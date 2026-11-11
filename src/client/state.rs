//! Синхронизация не-git состояния флота: tmux-раскладка и сессии opencode.
//!
//! Транспорт — хаб (`protocol::StateItem`, `hub::state`). Каналы:
//! * `tmux` — последний снапшот `tmux-resurrect` (один элемент `latest`);
//! * `opencode` — экспортированные сессии (`opencode session export`), по
//!   элементу на сессию (ключ — id сессии, `meta` — каталог проекта).
//!
//! Локальный индекс `~/.local/share/dsync/state.json` хранит:
//! * `seq` — последний известный сквозной seq хаба (забираем только новее);
//! * `items` — `{ "channel:key": updated }` уже отправленных/применённых
//!   элементов, чтобы не гонять их повторно.
//!
//! Всё best-effort: отсутствие tmux/opencode или ошибка применения не должны
//! валить общий `dsync push`/`pull`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::{Config, OpencodeStateConfig};
use crate::protocol::{StateItem, StatePullRequest, StatePushRequest};

const TMUX_CHANNEL: &str = "tmux";
const OPENCODE_CHANNEL: &str = "opencode";
/// Не приближаемся к hub `max_message_size` (8 MB по умолчанию).
const MAX_BATCH_BYTES: usize = 6 * 1024 * 1024;
/// Размер страницы при pull состояния: хаб отдаёт стор порциями.
const PAGE_BYTES: u64 = 6 * 1024 * 1024;
/// Предохранитель от бесконечной пагинации (баг/старый хаб).
const MAX_PAGES: usize = 64;
/// Таймаут одной state-операции: загрузка/выгрузка может быть многомегабайтной.
const STATE_TIMEOUT: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------------------
// Локальный индекс
// ---------------------------------------------------------------------------

/// Счётчик неудачных применений элемента (для retry/backoff).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub attempts: u32,
    pub updated: i64,
    /// hub-seq элемента — чтобы вернуться к нему на следующем pull.
    pub seq: i64,
}

/// Элемент, который не применился `retry_limit` раз. Больше не блокирует
/// канал: следующие элементы применяются, а этот ждёт нового `updated`
/// (тогда origin пришлёт новую версию) или `dsync state retry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadItem {
    pub updated: i64,
    pub seq: i64,
    pub error: String,
    /// Когда элемент попал в dead-letter (сек. эпохи).
    pub at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateIndex {
    #[serde(default)]
    seq: i64,
    #[serde(default)]
    items: HashMap<String, i64>,
    /// Ключ `channel:key` → счётчик неудач.
    #[serde(default)]
    failures: HashMap<String, Failure>,
    /// Ключ `channel:key` → окончательно сбойный элемент.
    #[serde(default)]
    dead: HashMap<String, DeadItem>,
}

fn index_path() -> PathBuf {
    if let Ok(p) = std::env::var("DSYNC_STATE_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::data_dir()
        .unwrap_or_default()
        .join("dsync/state.json")
}

impl StateIndex {
    fn load() -> Self {
        let p = index_path();
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<()> {
        let p = index_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    fn key_of(item: &StateItem) -> String {
        format!("{}:{}", item.channel, item.key)
    }

    /// Уже знаем это значение (не нужно повторно отправлять/применять)?
    fn has(&self, item: &StateItem) -> bool {
        self.items
            .get(&Self::key_of(item))
            .map(|u| *u >= item.updated)
            .unwrap_or(false)
    }

    /// Точное совпадение `updated` — для каналов, где `updated` является хешем
    /// содержимого (tmux-снапшот), а не монотонным счётчиком. Сравнение `>=`
    /// здесь ошибочно: новый хеш может оказаться численно МЕНЬШЕ прежнего, и
    /// тогда обновление молча теряется навсегда — в индексе остаётся большое
    /// «отравленное» значение, которое блокирует все последующие раскладки.
    fn has_exact(&self, item: &StateItem) -> bool {
        self.items
            .get(&Self::key_of(item))
            .map(|u| *u == item.updated)
            .unwrap_or(false)
    }

    /// Элемент уже применён (не нужно применять повторно)?
    ///
    /// Критично для ретраев: раньше seq откатывался назад при любой ошибке,
    /// и вместе с упавшим элементом заново применялись все успешные — включая
    /// tmux-снапшот, из-за чего `tmux-resurrect restore` дёргался каждые
    /// 15 минут. Проверка перед `apply_one` это исключает.
    fn is_known(&self, item: &StateItem) -> bool {
        if item.channel == TMUX_CHANNEL {
            self.has_exact(item)
        } else {
            self.has(item)
        }
    }
}

/// Публичная сводка локального state-индекса (для `doctor`/`state list`).
pub struct StateHealth {
    pub seq: i64,
    pub applied: usize,
    pub failures: Vec<(String, Failure)>,
    pub dead: Vec<(String, DeadItem)>,
}

pub fn health() -> StateHealth {
    let idx = StateIndex::load();
    let mut failures: Vec<_> = idx.failures.into_iter().collect();
    failures.sort_by(|a, b| a.0.cmp(&b.0));
    let mut dead: Vec<_> = idx.dead.into_iter().collect();
    dead.sort_by(|a, b| a.0.cmp(&b.0));
    StateHealth {
        seq: idx.seq,
        applied: idx.items.len(),
        failures,
        dead,
    }
}

/// Человекочитаемая сводка локального state-индекса (`dsync state list`).
pub fn describe() -> Vec<String> {
    let h = health();
    let mut out = vec![format!(
        "state index: seq={}, applied={}, failing={}, dead-letter={}",
        h.seq,
        h.applied,
        h.failures.len(),
        h.dead.len()
    )];
    for (key, f) in &h.failures {
        out.push(format!(
            "  failing  {key}: {} attempt(s), updated={}",
            f.attempts, f.updated
        ));
    }
    for (key, d) in &h.dead {
        out.push(format!(
            "  dead     {key}: {} (seq={}, updated={})",
            d.error, d.seq, d.updated
        ));
    }
    out
}

/// Убирает элементы из dead-letter (все или один `channel:key`) и откатывает
/// seq к самому раннему из них, чтобы хаб выдал их снова.
pub fn retry_dead(only: Option<&str>) -> usize {
    let mut idx = StateIndex::load();
    let keys: Vec<String> = match only {
        Some(want) => idx
            .dead
            .keys()
            .filter(|k| k.as_str() == want)
            .cloned()
            .collect(),
        None => idx.dead.keys().cloned().collect(),
    };
    if keys.is_empty() {
        return 0;
    }
    let mut min_seq: Option<i64> = None;
    for k in &keys {
        if let Some(d) = idx.dead.remove(k) {
            min_seq = Some(min_seq.map_or(d.seq, |m: i64| m.min(d.seq)));
            idx.failures.remove(k);
        }
    }
    if let Some(seq) = min_seq {
        idx.seq = (seq - 1).max(0);
    }
    if let Err(e) = idx.save() {
        warn!("can't save state index: {e:#}");
    }
    keys.len()
}

/// Последний известный seq хаба (для инкрементального pull).
pub fn get_seq() -> i64 {
    StateIndex::load().seq
}

/// Запоминает seq хаба (монотонно — не откатываем назад).
pub fn set_seq(seq: i64) {
    let mut idx = StateIndex::load();
    if seq > idx.seq {
        idx.seq = seq;
        if let Err(e) = idx.save() {
            warn!("can't save state index: {e:#}");
        }
    }
}

/// Принудительно ставит seq (в т.ч. назад) — чтобы на следующем pull повторить
/// элементы, которые не удалось применить.
fn set_seq_force(seq: i64) {
    let mut idx = StateIndex::load();
    idx.seq = seq.max(0);
    if let Err(e) = idx.save() {
        warn!("can't save state index: {e:#}");
    }
}

/// Помечает элементы отправленными/применёнными (после успешной синхронизации).
fn mark_synced(items: &[StateItem]) {
    let mut idx = StateIndex::load();
    for it in items {
        idx.items.insert(StateIndex::key_of(it), it.updated);
    }
    if let Err(e) = idx.save() {
        warn!("can't save state index: {e:#}");
    }
}

// ---------------------------------------------------------------------------
// Сбор локальных изменений
// ---------------------------------------------------------------------------

/// Собирает локальные элементы состояния, изменившиеся с прошлого раза.
pub fn collect(cfg: &Config) -> Vec<StateItem> {
    let Some(st) = cfg.state.as_ref() else {
        return Vec::new();
    };
    let idx = StateIndex::load();
    let mut out = Vec::new();
    if st.tmux {
        collect_tmux(&idx, &mut out);
    }
    if let Some(oc) = &st.opencode {
        if oc.enabled() {
            collect_opencode(cfg, oc, &idx, &mut out);
        }
    }
    out
}

/// Каналы, которые эта машина реально синхронизирует. Отправляется хабу,
/// чтобы он не присылал мегабайты чужих сессий машине с выключенным каналом.
fn wanted_channels(cfg: &Config) -> Vec<String> {
    let Some(st) = cfg.state.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if st.tmux {
        out.push(TMUX_CHANNEL.to_string());
    }
    if st.opencode.as_ref().map(|oc| oc.enabled()).unwrap_or(false) {
        out.push(OPENCODE_CHANNEL.to_string());
    }
    out
}

fn collect_tmux(idx: &StateIndex, out: &mut Vec<StateItem>) {
    let Some(dir) = tmux_resurrect_dir() else {
        return;
    };
    // Каталог снапшотов мог ещё не появиться (свежая машина, или continuum не
    // успел сохранить из-за порядка загрузки плагинов). Раньше здесь был ранний
    // `return`, из-за чего dsync молча не бэкапил tmux, пока кто-то другой не
    // создаст каталог. Теперь создаём его сами и всё равно просим tmux сохранить
    // текущую раскладку — бэкап не зависит от tmux-continuum.
    if !dir.is_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    // Best-effort: попросить tmux сохранить текущую раскладку.
    let before = newest_resurrect_file(&dir).map(|(_, m)| m).unwrap_or(0);
    trigger_tmux_save(&dir, before);

    let Some((path, mtime)) = newest_resurrect_file(&dir) else {
        return;
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return;
    };
    // `updated` = хеш содержимого: неизменённую раскладку повторно не гоним
    // (mtime меняется от каждого save, содержимое — нет).
    let item = StateItem {
        channel: TMUX_CHANNEL.into(),
        key: "latest".into(),
        updated: content_hash(&data),
        meta: path.file_name().map(|s| s.to_string_lossy().to_string()),
        data,
        ..Default::default()
    };
    let _ = mtime;
    if !idx.has_exact(&item) {
        out.push(item);
    }
}

/// Стабильный положительный i64-хеш содержимого (для `updated`).
fn content_hash(data: &str) -> i64 {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    i64::from_be_bytes(b) & i64::MAX
}

fn collect_opencode(
    cfg: &Config,
    oc: &OpencodeStateConfig,
    idx: &StateIndex,
    out: &mut Vec<StateItem>,
) {
    let Some(bin) = opencode_bin() else {
        return;
    };
    for proj in opencode_projects(cfg, oc) {
        for s in list_sessions(&bin, &proj) {
            let item = StateItem {
                channel: OPENCODE_CHANNEL.into(),
                key: s.id.clone(),
                updated: s.updated,
                meta: Some(proj.display().to_string()),
                data: String::new(), // заполним после проверки индекса
                ..Default::default()
            };
            if idx.has(&item) {
                continue;
            }
            match export_session(&bin, &proj, &s.id) {
                Some(data) => {
                    let mut it = item;
                    it.data = data;
                    out.push(it);
                }
                None => warn!("opencode: не удалось экспортировать сессию {}", s.id),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Применение чужих изменений
// ---------------------------------------------------------------------------

/// Результат применения чужих элементов: сообщения для пользователя,
/// успешно применённые и сбойные элементы (по ним ведётся retry/dead-letter).
#[derive(Default)]
pub struct ApplyReport {
    pub messages: Vec<String>,
    pub applied: Vec<StateItem>,
    pub failed: Vec<StateItem>,
    /// Пропущено: уже применено ранее / канал выключен / неизвестный канал.
    pub skipped: usize,
}

/// Применяет полученные от хаба элементы.
///
/// Уже применённые элементы (по локальному индексу) пропускаются — это делает
/// ретраи безопасными: повторная страница состояния не перезапускает
/// `tmux-resurrect restore` и не переимпортирует сессии.
pub fn apply(cfg: &Config, items: Vec<StateItem>) -> ApplyReport {
    let mut report = ApplyReport::default();
    let Some(st) = cfg.state.as_ref() else {
        return report;
    };
    let idx = StateIndex::load();
    let mut refresh_projects: Vec<PathBuf> = Vec::new();
    for item in items {
        let wanted = match item.channel.as_str() {
            TMUX_CHANNEL => st.tmux,
            OPENCODE_CHANNEL => st.opencode.as_ref().map(|oc| oc.enabled()).unwrap_or(false),
            other => {
                // Неизвестный канал (например, устаревший `zellij`) — не шумим
                // warn'ом на каждом цикле, просто не применяем.
                tracing::debug!("state: канал {other} не поддерживается — пропуск");
                false
            }
        };
        if !wanted {
            report.skipped += 1;
            continue;
        }
        if idx.is_known(&item) {
            report.skipped += 1;
            continue;
        }
        match apply_one(cfg, &item) {
            Ok(msg) => {
                report.messages.push(msg);
                if item.channel == OPENCODE_CHANNEL {
                    if let Some(m) = &item.meta {
                        refresh_projects.push(PathBuf::from(m));
                    }
                }
                report.applied.push(item);
            }
            Err(e) => {
                warn!(
                    "state: не удалось применить {}/{}: {e:#}",
                    item.channel, item.key
                );
                report.failed.push(item);
            }
        }
    }
    if !report.applied.is_empty() {
        mark_synced(&report.applied);
    }
    // Импорт opencode бампает локальный `updated` сессии, из-за чего её
    // тут же захотелось бы отправить обратно (эхо-петля). Освежаем индекс
    // по фактическим локальным updated для уже известных сессий.
    if !refresh_projects.is_empty() {
        refresh_projects.sort();
        refresh_projects.dedup();
        refresh_opencode_index(&refresh_projects);
    }
    report
}

/// Обновляет учёт неудач и возвращает seq самого раннего элемента, у которого
/// ещё остались попытки (курсор pull'а должен остаться перед ним), либо None.
///
/// После `retry_limit` неудач элемент уходит в dead-letter: канал больше не
/// блокируется, а элемент ждёт новой версии от origin или `dsync state retry`.
fn record_failures(
    idx: &mut StateIndex,
    applied: &[StateItem],
    failed: &[StateItem],
    retry_limit: u32,
) -> Option<i64> {
    for it in applied {
        let k = StateIndex::key_of(it);
        idx.failures.remove(&k);
        idx.dead.remove(&k);
    }
    let mut blocked: Option<i64> = None;
    for it in failed {
        let k = StateIndex::key_of(it);
        let f = idx.failures.entry(k.clone()).or_insert(Failure {
            attempts: 0,
            updated: it.updated,
            seq: it.seq,
        });
        if f.updated != it.updated {
            // Пришла новая версия сбойного элемента — считаем попытки заново.
            f.attempts = 0;
            f.updated = it.updated;
            idx.dead.remove(&k);
        }
        f.attempts += 1;
        f.seq = it.seq;
        if f.attempts >= retry_limit {
            let error = format!(
                "не удалось применить за {} попыт(ки) (канал {})",
                f.attempts, it.channel
            );
            warn!("state: {}/{} → dead-letter", it.channel, it.key);
            idx.dead.insert(
                k.clone(),
                DeadItem {
                    updated: it.updated,
                    seq: it.seq,
                    error,
                    at: now_secs(),
                },
            );
            idx.failures.remove(&k);
        } else if it.seq > 0 {
            blocked = Some(blocked.map_or(it.seq, |m: i64| m.min(it.seq)));
        }
    }
    blocked
}

/// Обновляет `updated` уже известных (синхронизированных) сессий до
/// локального значения, чтобы не пере-экспортировать только что импортированное.
fn refresh_opencode_index(projects: &[PathBuf]) {
    let Some(bin) = opencode_bin() else {
        return;
    };
    let mut idx = StateIndex::load();
    let mut changed = false;
    for proj in projects {
        for s in list_sessions(&bin, proj) {
            let key = format!("{OPENCODE_CHANNEL}:{}", s.id);
            if let Some(v) = idx.items.get_mut(&key) {
                if s.updated > *v {
                    *v = s.updated;
                    changed = true;
                }
            }
        }
    }
    if changed {
        if let Err(e) = idx.save() {
            warn!("can't save state index: {e:#}");
        }
    }
}

fn apply_one(cfg: &Config, item: &StateItem) -> Result<String> {
    match item.channel.as_str() {
        TMUX_CHANNEL => apply_tmux(cfg, item),
        OPENCODE_CHANNEL => apply_opencode(item),
        other => bail!("unknown channel {other}"),
    }
}

fn apply_tmux(cfg: &Config, item: &StateItem) -> Result<String> {
    let dir = tmux_resurrect_dir().context("no home dir")?;
    std::fs::create_dir_all(&dir)?;
    let fname = sanitize_filename(
        item.meta
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or("tmux_resurrect_synced.txt"),
    );
    if !fname.ends_with(".txt") {
        bail!("unexpected tmux snapshot name {fname}");
    }
    std::fs::write(dir.join(&fname), item.data.as_bytes())?;
    update_last_symlink(&dir, &fname)?;

    let mut msg = format!("tmux: снапшот от {} → {fname}", item.origin);
    if cfg.state.as_ref().map(|s| s.tmux_restore).unwrap_or(false) && trigger_tmux_restore() {
        msg.push_str(" (restore запущен)");
    }
    Ok(msg)
}

fn apply_opencode(item: &StateItem) -> Result<String> {
    let bin = opencode_bin().context("opencode binary not found")?;
    if serde_json::from_str::<serde_json::Value>(&item.data).is_err() {
        bail!("session JSON невалиден/обрезан ({} байт)", item.data.len());
    }
    let dir = item
        .meta
        .clone()
        .filter(|m| !m.is_empty())
        .context("opencode session without project dir")?;
    let dirp = PathBuf::from(&dir);
    if !dirp.is_dir() {
        warn!("opencode: проект {dir} отсутствует — импорт пропущен");
    }
    let tmp = std::env::temp_dir().join(format!(
        "dsync-session-{}.json",
        sanitize_filename(&item.key)
    ));
    std::fs::write(&tmp, item.data.as_bytes())?;

    let mut cmd = Command::new(&bin);
    cmd.arg("session").arg("import").arg(&tmp);
    if dirp.is_dir() {
        cmd.current_dir(&dirp);
    }
    let out = cmd.output().context("spawn opencode import")?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        bail!(
            "opencode import failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(format!(
        "opencode: импортирована сессия {} → {dir}",
        item.key
    ))
}

// ---------------------------------------------------------------------------
// Оркестрация (вызывается из push/pull)
// ---------------------------------------------------------------------------

/// Отправляет локальные изменения состояния и применяет чужие.
/// `capture` = собирать ли локальные изменения (tmux-сейв, экспорт сессий).
/// Пуш — собирает; pull — только применяет чужие, чтобы не дёргать
/// tmux-resurrect повторно в одном цикле `dsync-run` (push+pull подряд).
/// Ошибки логируются, но не прерывают общий push/pull.
pub async fn sync_state(cfg: &Config, capture: bool) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.state.is_none() {
        return out;
    }

    // Сначала (возможно, долгий) сбор — экспорт сессий opencode занимает
    // десятки секунд. Делаем это ДО подключения: иначе QUIC-соединение
    // простаивает и рвётся по idle-таймауту.
    let changed = if capture { collect(cfg) } else { Vec::new() };
    if !changed.is_empty() {
        info!("state: {} item(s) to upload", changed.len());
    }

    let conn = match super::connect::connect_with_retry(cfg).await {
        Ok(c) => c,
        Err(e) => {
            warn!("state sync skipped (hub unavailable): {e:#}");
            return out;
        }
    };
    for batch in batches(changed) {
        let req = StatePushRequest {
            machine: cfg.machine.name.clone(),
            token: super::connect::hub_token(cfg),
            timestamp: now_secs(),
            items: batch.clone(),
        };
        match tokio::time::timeout(STATE_TIMEOUT, super::connect::send_state_push(&conn, &req))
            .await
        {
            Ok(Ok(resp)) if resp.ok => mark_synced(&batch),
            Ok(Ok(resp)) => warn!("state push rejected: {}", resp.error.unwrap_or_default()),
            Ok(Err(e)) => {
                warn!("state push failed: {e:#}");
                break;
            }
            Err(_) => {
                warn!("state push timed out (hub too old?)");
                break;
            }
        }
    }

    // 2. Загрузка чужих изменений — страницами, чтобы не тянуть весь стор
    //    (сейчас ~66 МБ opencode-сессий) одним сообщением.
    let retry_limit = cfg
        .state
        .as_ref()
        .map(|s| s.retry_limit)
        .unwrap_or_else(crate::config::default_state_retry_limit)
        .max(1);
    let mut since = get_seq();
    for _page in 0..MAX_PAGES {
        let req = StatePullRequest {
            machine: cfg.machine.name.clone(),
            token: super::connect::hub_token(cfg),
            state_seq: since,
            max_bytes: PAGE_BYTES,
        };
        let resp = match tokio::time::timeout(STATE_TIMEOUT, super::connect::send_state_pull(
            &conn, &req,
        ))
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                warn!("state pull failed: {e:#}");
                break;
            }
            Err(_) => {
                warn!("state pull timed out (hub too old?)");
                break;
            }
        };

        let page_len = resp.items.len();
        let report = apply(cfg, resp.items);
        out.extend(report.messages);

        // Учёт неудач: после `retry_limit` попыток элемент уходит в dead-letter
        // и перестаёт блокировать канал (иначе один сломанный импорт заставлял
        // перекачивать и переприменять всё состояние каждый цикл — навсегда).
        let mut idx = StateIndex::load();
        let blocked = record_failures(&mut idx, &report.applied, &report.failed, retry_limit);
        if let Err(e) = idx.save() {
            warn!("can't save state index: {e:#}");
        }

        match blocked {
            // Есть сбойные элементы с остатком попыток: держим курсор перед
            // самым ранним из них и повторим на следующем цикле. Успешные
            // элементы повторно не применяются (см. `apply`).
            Some(min_seq) => {
                set_seq_force(min_seq - 1);
                break;
            }
            None => {
                // Старый хаб не умеет страницы (next_seq = 0) — он отдал всё,
                // дальше идти некуда.
                let next = if resp.next_seq > 0 {
                    resp.next_seq
                } else {
                    resp.state_seq
                };
                set_seq(next);
                since = next;
                if !resp.has_more || resp.next_seq <= 0 {
                    break;
                }
            }
        }
        if page_len == 0 && !resp.has_more {
            break;
        }
    }

    super::connect::close_conn(&conn);
    out
}

/// Разбивает элементы на батчи, не превышающие `MAX_BATCH_BYTES`.
fn batches(items: Vec<StateItem>) -> Vec<Vec<StateItem>> {
    let mut out = Vec::new();
    let mut cur: Vec<StateItem> = Vec::new();
    let mut bytes = 0usize;
    for it in items {
        let sz = it.data.len()
            + it.key.len()
            + it.channel.len()
            + it.meta.as_deref().map(str::len).unwrap_or(0)
            + 128;
        if bytes + sz > MAX_BATCH_BYTES && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        bytes += sz;
        cur.push(it);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// Хелперы: tmux
// ---------------------------------------------------------------------------

fn tmux_resurrect_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let xdg = home.join(".local/share/tmux/resurrect");
    let legacy = home.join(".tmux/resurrect");
    if xdg.is_dir() {
        Some(xdg)
    } else if legacy.is_dir() {
        Some(legacy)
    } else {
        Some(xdg)
    }
}

fn newest_resurrect_file(dir: &Path) -> Option<(PathBuf, i64)> {
    let mut best: Option<(PathBuf, i64)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("tmux_resurrect_") || !name.ends_with(".txt") {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        let Ok(modified) = md.modified() else {
            continue;
        };
        let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) else {
            continue;
        };
        let mtime = since.as_secs() as i64;
        if best.as_ref().map(|(_, m)| mtime > *m).unwrap_or(true) {
            best = Some((entry.path(), mtime));
        }
    }
    best
}

fn tmux_running() -> bool {
    Command::new("tmux")
        .args(["list-sessions"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn trigger_tmux_save(dir: &Path, prev_mtime: i64) {
    if !tmux_running() {
        return;
    }
    let Some(script) = tmux_resurrect_script("save.sh") else {
        return;
    };
    let _ = Command::new("tmux")
        .args(["run-shell", &script.display().to_string()])
        .output();
    // Сохранение из run-shell асинхронное — недолго ждём новый файл.
    for _ in 0..15 {
        if newest_resurrect_file(dir)
            .map(|(_, m)| m > prev_mtime)
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn trigger_tmux_restore() -> bool {
    if !tmux_running() {
        return false;
    }
    let Some(script) = tmux_resurrect_script("restore.sh") else {
        return false;
    };
    Command::new("tmux")
        .args(["run-shell", &script.display().to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmux_resurrect_script(name: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let p = home.join(".tmux/plugins/tmux-resurrect/scripts").join(name);
    p.is_file().then_some(p)
}

#[cfg(unix)]
fn update_last_symlink(dir: &Path, target: &str) -> Result<()> {
    let last = dir.join("last");
    let _ = std::fs::remove_file(&last);
    std::os::unix::fs::symlink(target, &last)?;
    Ok(())
}

#[cfg(not(unix))]
fn update_last_symlink(_dir: &Path, _target: &str) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Хелперы: opencode
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SessionRow {
    id: String,
    #[serde(default)]
    updated: i64,
}

/// Публичный доступ к найденному бинарю opencode (для `doctor`).
pub fn opencode_binary() -> Option<PathBuf> {
    opencode_bin()
}

fn opencode_bin() -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".opencode/bin/opencode");
        if p.is_file() {
            return Some(p);
        }
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        let c = dir.join("opencode");
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

fn opencode_projects(cfg: &Config, oc: &OpencodeStateConfig) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(list) = &oc.projects {
        for p in list {
            out.push(expand_user_path(p));
        }
    } else if let Some(projects) = &cfg.projects {
        for pc in projects.values() {
            out.push(crate::projects::status::expand_user_path(&pc.path));
        }
    }
    out.sort();
    out.dedup();
    out.retain(|p| p.is_dir());
    out
}

fn expand_user_path(p: &Path) -> PathBuf {
    crate::projects::status::expand_user_path(p)
}

fn list_sessions(bin: &Path, project: &Path) -> Vec<SessionRow> {
    let out = match Command::new(bin)
        .args(["session", "list", "--format", "json"])
        .current_dir(project)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    serde_json::from_slice::<Vec<SessionRow>>(&out.stdout).unwrap_or_default()
}

fn export_session(bin: &Path, project: &Path, id: &str) -> Option<String> {
    // Экспорт opencode V2 через общий фоновый сервис недетерминированно
    // обрезает вывод (баг CLI). `--standalone` (приватный сервер) даёт
    // стабильный полный JSON; на всякий случай всё равно валидируем.
    for attempt in 1..=5 {
        let out = match Command::new(bin)
            .args(["session", "export", "--standalone", id])
            .current_dir(project)
            .output()
        {
            Ok(o) => o,
            Err(_) => return None,
        };
        if !out.status.success() {
            continue;
        }
        let data = String::from_utf8_lossy(&out.stdout).to_string();
        if serde_json::from_str::<serde_json::Value>(&data).is_ok() {
            return Some(data);
        }
        warn!(
            "opencode: экспорт сессии {id} вернул неполный JSON ({} байт), попытка {attempt}/5",
            data.len()
        );
    }
    None
}

// ---------------------------------------------------------------------------
// Утилиты
// ---------------------------------------------------------------------------

fn sanitize_filename(s: &str) -> String {
    let base = Path::new(s)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| s.to_string());
    base.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тесты, трогающие `DSYNC_STATE_PATH`, идут последовательно: переменная
    /// окружения общая на процесс, а cargo гоняет тесты параллельно.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn item(channel: &str, key: &str, updated: i64, data_len: usize) -> StateItem {
        StateItem {
            channel: channel.into(),
            key: key.into(),
            updated,
            data: "x".repeat(data_len),
            ..Default::default()
        }
    }

    #[test]
    fn sanitize_strips_paths_and_specials() {
        assert_eq!(sanitize_filename("/etc/foo/bar.txt"), "bar.txt");
        assert_eq!(sanitize_filename("a/b:c"), "b_c");
        assert_eq!(sanitize_filename("ses_123-abc"), "ses_123-abc");
    }

    #[test]
    fn batches_split_by_size() {
        let items = vec![
            item("opencode", "a", 1, 10),
            item("opencode", "b", 1, MAX_BATCH_BYTES + 10),
            item("opencode", "c", 1, 10),
        ];
        let b = batches(items);
        assert_eq!(b.len(), 3, "oversized item gets its own batch");
        assert_eq!(b[0][0].key, "a");
        assert_eq!(b[1][0].key, "b");
        assert_eq!(b[2][0].key, "c");
    }

    #[test]
    fn batches_single_when_small() {
        let b = batches(vec![item("tmux", "latest", 1, 10)]);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn content_hash_stable_and_positive() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
        assert!(content_hash("abc") >= 0);
    }

    fn state_cfg() -> Config {
        Config {
            config_version: 1,
            machine: crate::config::MachineConfig {
                name: "t".into(),
                ssh_key: None,
            },
            hub: None,
            hub_connect: None,
            projects: None,
            auto_projects: None,
            remote: None,
            capture: None,
            state: Some(crate::config::StateConfig {
                tmux: true,
                tmux_restore: false,
                opencode: None,
                retry_limit: 3,
            }),
        }
    }

    #[test]
    fn collect_without_state_config_is_empty() {
        let cfg = Config {
            config_version: 1,
            machine: crate::config::MachineConfig {
                name: "t".into(),
                ssh_key: None,
            },
            hub: None,
            hub_connect: None,
            projects: None,
            auto_projects: None,
            remote: None,
            capture: None,
            state: None,
        };
        assert!(collect(&cfg).is_empty());
        assert!(apply(&cfg, vec![item("tmux", "latest", 1, 3)])
            .messages
            .is_empty());
    }

    #[test]
    fn apply_skips_already_applied_items() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ретрай страницы состояния не должен повторно писать tmux-снапшот
        // (и запускать restore): уже применённый элемент пропускается.
        let dir = std::env::temp_dir().join(format!("dsync-state-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));

        let cfg = state_cfg();
        let tmux = item("tmux", "latest", 42, 3);
        mark_synced(std::slice::from_ref(&tmux));

        let report = apply(&cfg, vec![tmux]);
        assert!(report.applied.is_empty(), "nothing re-applied");
        assert_eq!(report.skipped, 1);
        assert!(report.failed.is_empty());

        std::env::remove_var("DSYNC_STATE_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_failures_moves_item_to_dead_letter_after_limit() {
        let mut idx = StateIndex::default();
        let bad = StateItem {
            seq: 100,
            ..item("opencode", "ses_x", 100, 1)
        };
        assert_eq!(
            record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3),
            Some(100),
            "first failure keeps the cursor before the item"
        );
        assert_eq!(
            record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3),
            Some(100)
        );
        assert_eq!(
            record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3),
            None,
            "after the limit the channel is no longer blocked"
        );
        assert_eq!(idx.dead.len(), 1, "dead-letter recorded");
        assert!(idx.failures.is_empty(), "counter cleared after dead-letter");
    }

    #[test]
    fn record_failures_returns_earliest_pending_seq() {
        let mut idx = StateIndex::default();
        let a = StateItem {
            seq: 30,
            ..item("opencode", "ses_a", 1, 1)
        };
        let b = StateItem {
            seq: 10,
            ..item("opencode", "ses_b", 1, 1)
        };
        let blocked = record_failures(&mut idx, &[], &[a, b], 5);
        assert_eq!(blocked, Some(10), "cursor must stop before the earliest");
        assert_eq!(idx.failures.len(), 2);
    }

    #[test]
    fn new_version_resets_failure_counter() {
        let mut idx = StateIndex::default();
        let old = item("opencode", "ses_x", 100, 1);
        for _ in 0..2 {
            record_failures(&mut idx, &[], std::slice::from_ref(&old), 3);
        }
        assert_eq!(idx.failures["opencode:ses_x"].attempts, 2);
        let new = item("opencode", "ses_x", 200, 1);
        record_failures(&mut idx, &[], std::slice::from_ref(&new), 3);
        assert_eq!(
            idx.failures["opencode:ses_x"].attempts, 1,
            "new updated — fresh budget"
        );
    }

    #[test]
    fn retry_dead_rolls_seq_back_and_clears() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dsync-state-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));

        let mut idx = StateIndex::default();
        let bad = StateItem {
            seq: 77,
            ..item("opencode", "ses_x", 100, 1)
        };
        for _ in 0..3 {
            record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3);
        }
        idx.seq = 120;
        idx.save().unwrap();
        assert_eq!(health().dead.len(), 1);

        assert_eq!(retry_dead(None), 1);
        let h = health();
        assert!(h.dead.is_empty());
        assert_eq!(h.seq, 76, "seq rolled back before the failed item");

        std::env::remove_var("DSYNC_STATE_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_roundtrip_marks_and_skips() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dsync-state-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));
        let it = item("opencode", "ses_a", 100, 1);
        assert!(!StateIndex::load().has(&it));
        mark_synced(std::slice::from_ref(&it));
        assert!(StateIndex::load().has(&it));
        let older = item("opencode", "ses_a", 50, 1);
        assert!(StateIndex::load().has(&older), "older counts as known");
        let newer = item("opencode", "ses_a", 200, 1);
        assert!(!StateIndex::load().has(&newer));
        set_seq(7);
        assert_eq!(get_seq(), 7);
        set_seq(3); // не откатываем
        assert_eq!(get_seq(), 7);
        std::env::remove_var("DSYNC_STATE_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
