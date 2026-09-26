//! Синхронизация не-git состояния флота: раскладка Zellij и сессии opencode.
//!
//! Транспорт — хаб (`protocol::StateItem`, `hub::state`). Каналы:
//! * `zellij` — сериализованная раскладка сессии Zellij (один элемент
//!   `latest`): набор файлов `~/.cache/zellij/.../session_info/main/`
//!   (metadata + session-layout + содержимое панелей), упакованный в JSON;
//! * `opencode` — экспортированные сессии (`opencode session export`), по
//!   элементу на сессию (ключ — id сессии, `meta` — каталог проекта).
//!
//! Локальный индекс `~/.local/share/dsync/state.json` хранит:
//! * `seq` — последний известный сквозной seq хаба (забираем только новее);
//! * `items` — `{ "channel:key": updated }` уже отправленных/применённых
//!   элементов, чтобы не гонять их повторно.
//!
//! Всё best-effort: отсутствие Zellij/opencode или ошибка применения не должны
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

const ZELLIJ_CHANNEL: &str = "zellij";
const OPENCODE_CHANNEL: &str = "opencode";
/// Не приближаемся к hub `max_message_size` (8 MB по умолчанию).
const MAX_BATCH_BYTES: usize = 6 * 1024 * 1024;
/// Таймаут одной state-операции: загрузка/выгрузка может быть многомегабайтной.
const STATE_TIMEOUT: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------------------
// Локальный индекс
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateIndex {
    #[serde(default)]
    seq: i64,
    #[serde(default)]
    items: HashMap<String, i64>,
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
    if st.zellij {
        collect_zellij(&idx, &mut out);
    }
    if let Some(oc) = &st.opencode {
        collect_opencode(cfg, oc, &idx, &mut out);
    }
    out
}

/// Снимок раскладки Zellij. Zellij сам сериализует сессию в
/// `~/.cache/zellij/<contract>/session_info/<session>/` (metadata + layout +
/// содержимое панелей). Просим Zellij сохранить актуальное состояние
/// (`zellij action save-session`), затем упаковываем содержимое каталога в
/// один JSON-payload (`{ "session": "main", "files": { "<имя>": "<текст>" } }`).
fn collect_zellij(idx: &StateIndex, out: &mut Vec<StateItem>) {
    let Some(session) = zellij_session_name() else {
        return;
    };
    // Best-effort: попросить Zellij сохранить текущую раскладку.
    trigger_zellij_save(&session);

    let Some(dir) = zellij_session_dir(&session) else {
        return;
    };
    let Some(data) = pack_session_dir(&session, &dir) else {
        return;
    };
    let item = StateItem {
        channel: ZELLIJ_CHANNEL.into(),
        key: "latest".into(),
        updated: content_hash(&data),
        meta: Some(session),
        data,
        ..Default::default()
    };
    if !idx.has(&item) {
        out.push(item);
    }
}

/// Каталог сериализованной сессии Zellij, подходящий по имени сессии.
/// Zellij держит файлы под подкаталогом с contract-версией, которая может
/// меняться между релизами, поэтому ищем `*/session_info/<session>`.
fn zellij_session_dir(session: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let root = home.join(".cache/zellij");
    if !root.is_dir() {
        return None;
    }
    let mut found: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("session_info").join(session);
            if candidate.is_dir() {
                // На случай нескольких contract-версий берём самый свежий.
                let newer = found
                    .as_ref()
                    .and_then(|p| dir_mtime(p))
                    .zip(dir_mtime(&candidate))
                    .map(|(a, b)| b > a)
                    .unwrap_or(found.is_none());
                if newer {
                    found = Some(candidate);
                }
            }
        }
    }
    found
}

fn dir_mtime(dir: &Path) -> Option<i64> {
    let md = std::fs::metadata(dir).ok()?;
    let since = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(since.as_secs() as i64)
}

/// Собирает файлы каталога сессии в JSON-payload.
fn pack_session_dir(session: &str, dir: &Path) -> Option<String> {
    use serde_json::json;
    let mut files = serde_json::Map::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        files.insert(name, serde_json::Value::String(content));
    }
    if files.is_empty() {
        return None;
    }
    serde_json::to_string(&json!({ "session": session, "files": files })).ok()
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

/// Применяет полученные от хаба элементы. Возвращает (строки, минимальный
/// `seq` неудачного элемента) — второй нужен, чтобы не терять сбойные элементы
/// при продвижении локального seq.
pub fn apply(cfg: &Config, items: Vec<StateItem>) -> (Vec<String>, Option<i64>) {
    let Some(st) = cfg.state.as_ref() else {
        return (Vec::new(), None);
    };
    let mut messages = Vec::new();
    let mut applied: Vec<StateItem> = Vec::new();
    let mut failed_min: Option<i64> = None;
    let mut refresh_projects: Vec<PathBuf> = Vec::new();
    for item in items {
        let wanted = match item.channel.as_str() {
            ZELLIJ_CHANNEL => st.zellij,
            OPENCODE_CHANNEL => st.opencode.is_some(),
            other => {
                warn!("state: неизвестный канал {other}");
                false
            }
        };
        if !wanted {
            continue;
        }
        match apply_one(cfg, &item) {
            Ok(msg) => {
                messages.push(msg);
                if item.channel == OPENCODE_CHANNEL {
                    if let Some(m) = &item.meta {
                        refresh_projects.push(PathBuf::from(m));
                    }
                }
                applied.push(item);
            }
            Err(e) => {
                warn!(
                    "state: не удалось применить {}/{}: {e:#}",
                    item.channel, item.key
                );
                if item.seq > 0 {
                    failed_min = Some(failed_min.map_or(item.seq, |m| m.min(item.seq)));
                }
            }
        }
    }
    if !applied.is_empty() {
        mark_synced(&applied);
    }
    // Импорт opencode бампает локальный `updated` сессии, из-за чего её
    // тут же захотелось бы отправить обратно (эхо-петля). Освежаем индекс
    // по фактическим локальным updated для уже известных сессий.
    if !refresh_projects.is_empty() {
        refresh_projects.sort();
        refresh_projects.dedup();
        refresh_opencode_index(&refresh_projects);
    }
    (messages, failed_min)
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
        ZELLIJ_CHANNEL => apply_zellij(cfg, item),
        OPENCODE_CHANNEL => apply_opencode(item),
        other => bail!("unknown channel {other}"),
    }
}

/// Применяет чужую раскладку Zellij: распаковывает файлы сессии в каталог
/// `~/.cache/zellij/<contract>/session_info/<session>/`. Если сессия с таким
/// именем уже запущена — в неё можно «войти» восстановлением только при
/// следующем старте; сам живой сервер не трогаем.
fn apply_zellij(cfg: &Config, item: &StateItem) -> Result<String> {
    let session = item
        .meta
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "main".to_string());
    let session = sanitize_filename(&session);
    let home = dirs::home_dir().context("no home dir")?;
    let root = home.join(".cache/zellij");

    // Куда писать: contract-каталог уже существующей сессии, иначе создаём
    // новую contract-папку (`contract_version_1` — текущая в Zellij 0.45).
    let target_root = zellij_session_dir(&session)
        .and_then(|p| p.parent().and_then(|p| p.parent()).map(Path::to_path_buf))
        .or_else(|| {
            std::fs::read_dir(&root)
                .ok()
                .and_then(|it| {
                    it.flatten()
                        .map(|e| e.path())
                        .find(|p| p.join("session_info").is_dir())
                })
                .or_else(|| Some(root.join("contract_version_1")))
        })
        .context("no zellij cache dir")?;
    let dir = target_root.join("session_info").join(&session);
    std::fs::create_dir_all(&dir)?;

    let parsed: serde_json::Value =
        serde_json::from_str(&item.data).context("zellij payload невалиден/обрезан")?;
    let files = parsed
        .get("files")
        .and_then(|f| f.as_object())
        .context("zellij payload без поля files")?;
    let mut written = 0usize;
    for (name, content) in files {
        let Some(text) = content.as_str() else {
            continue;
        };
        // Имя файла от чужой машины — не доверяем путям.
        let name = sanitize_filename(name);
        std::fs::write(dir.join(&name), text.as_bytes())?;
        written += 1;
    }

    let mut msg = format!(
        "zellij: раскладка от {} → сессия {session} ({written} файл(ов))",
        item.origin
    );
    if cfg
        .state
        .as_ref()
        .map(|s| s.zellij_restore)
        .unwrap_or(false)
        && trigger_zellij_restore(&session)
    {
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
/// `capture` = собирать ли локальные изменения (сейв Zellij, экспорт сессий).
/// Пуш — собирает; pull — только применяет чужие, чтобы не дёргать
/// сериализацию Zellij повторно в одном цикле `dsync-run` (push+pull подряд).
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

    // 2. Загрузка чужих изменений.
    let req = StatePullRequest {
        machine: cfg.machine.name.clone(),
        token: super::connect::hub_token(cfg),
        state_seq: get_seq(),
    };
    match tokio::time::timeout(STATE_TIMEOUT, super::connect::send_state_pull(&conn, &req)).await {
        Ok(Ok(resp)) => {
            let (msgs, failed_min) = apply(cfg, resp.items);
            out.extend(msgs);
            // Не теряем сбойные элементы: если что-то не применилось, не
            // продвигаем seq за них — на следующем pull повторим.
            match failed_min {
                Some(min) => set_seq_force(min - 1),
                None => set_seq(resp.state_seq),
            }
        }
        Ok(Err(e)) => warn!("state pull failed: {e:#}"),
        Err(_) => warn!("state pull timed out (hub too old?)"),
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
// Хелперы: Zellij
// ---------------------------------------------------------------------------

/// Имя сессии Zellij, чью раскладку синхронизируем. Берём из переменной
/// окружения (если `dsync` запущен внутри Zellij) или последнюю активную.
fn zellij_session_name() -> Option<String> {
    if let Ok(name) = std::env::var("ZELLIJ_SESSION_NAME") {
        if !name.is_empty() {
            return Some(name);
        }
    }
    // Иначе — самая свежая сессия (первая строка `zellij list-sessions -s -n`).
    let out = Command::new("zellij")
        .args(["list-sessions", "-s", "-n"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let name = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.contains("[Created"))?;
    Some(name.to_string())
}

fn zellij_running() -> bool {
    Command::new("zellij")
        .args(["list-sessions", "-s", "-n"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn trigger_zellij_save(session: &str) {
    if !zellij_running() {
        return;
    }
    let _ = Command::new("zellij")
        .args(["action", "save-session"])
        .env("ZELLIJ_SESSION_NAME", session)
        .output();
}

/// Восстанавливает раскладку в живой сессии: пересоздаёт её из сохранённых
/// файлов. Best-effort: если сессия занята клиентом — пропускаем.
fn trigger_zellij_restore(session: &str) -> bool {
    if !zellij_running() {
        return false;
    }
    // Только если сессия не запущена (иначе attach оживит уже живой сервер).
    let attached = Command::new("zellij")
        .args(["list-sessions", "-s", "-n"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.trim() == session)
        })
        .unwrap_or(false);
    if attached {
        return false;
    }
    // Сессия не запущена, но её файлы на диске — `attach -c` поднимет её из
    // сериализованного состояния; сразу отсоединяемся, чтобы не висеть.
    Command::new("zellij")
        .args(["attach", "--create", session])
        .env("ZELLIJ_SESSION_NAME", session)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
        let b = batches(vec![item("zellij", "latest", 1, 10)]);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn content_hash_stable_and_positive() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
        assert!(content_hash("abc") >= 0);
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
        assert!(apply(&cfg, vec![item("zellij", "latest", 1, 3)]).0.is_empty());
    }

    #[test]
    fn index_roundtrip_marks_and_skips() {
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
