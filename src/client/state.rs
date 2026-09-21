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
    if st.tmux {
        collect_tmux(&idx, &mut out);
    }
    if let Some(oc) = &st.opencode {
        collect_opencode(cfg, oc, &idx, &mut out);
    }
    out
}

fn collect_tmux(idx: &StateIndex, out: &mut Vec<StateItem>) {
    let Some(dir) = tmux_resurrect_dir() else {
        return;
    };
    if !dir.is_dir() {
        return;
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
    if !idx.has(&item) {
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
    for item in items {
        let wanted = match item.channel.as_str() {
            TMUX_CHANNEL => st.tmux,
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
    (messages, failed_min)
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
/// Ошибки логируются, но не прерывают общий push/pull.
pub async fn sync_state(cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.state.is_none() {
        return out;
    }
    let conn = match super::connect::connect_with_retry(cfg).await {
        Ok(c) => c,
        Err(e) => {
            warn!("state sync skipped (hub unavailable): {e:#}");
            return out;
        }
    };

    // 1. Загрузка локальных изменений (батчами под лимит сообщения).
    let changed = collect(cfg);
    if !changed.is_empty() {
        info!("state: {} item(s) to upload", changed.len());
    }
    for batch in batches(changed) {
        let req = StatePushRequest {
            machine: cfg.machine.name.clone(),
            token: super::connect::hub_token(cfg),
            timestamp: now_secs(),
            items: batch.clone(),
        };
        match tokio::time::timeout(
            Duration::from_secs(30),
            super::connect::send_state_push(&conn, &req),
        )
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
    match tokio::time::timeout(
        Duration::from_secs(30),
        super::connect::send_state_pull(&conn, &req),
    )
    .await
    {
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
    // Экспорт opencode V2 бывает недетерминированно обрезан (баг самого CLI):
    // валидируем JSON и повторяем, пока не получим целый документ.
    for attempt in 1..=5 {
        let out = match Command::new(bin)
            .args(["session", "export", id])
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
        let b = batches(vec![item("tmux", "latest", 1, 10)]);
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
            remote: None,
            capture: None,
            state: None,
        };
        assert!(collect(&cfg).is_empty());
        assert!(apply(&cfg, vec![item("tmux", "latest", 1, 3)]).0.is_empty());
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
