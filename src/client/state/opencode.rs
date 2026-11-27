//! Канал opencode: экспорт/импорт сессий по проектам.

use serde::Deserialize;

use super::*;

pub(crate) struct OpencodeChannel;

impl StateChannel for OpencodeChannel {
    fn name(&self) -> &'static str {
        OPENCODE_CHANNEL
    }

    fn wanted(&self, cfg: &Config) -> bool {
        cfg.state
            .as_ref()
            .and_then(|s| s.opencode.as_ref())
            .map(|oc| oc.enabled())
            .unwrap_or(false)
    }

    fn accepts(&self, cfg: &Config, _item: &StateItem) -> bool {
        self.wanted(cfg)
    }

    fn collect(&self, cfg: &Config, idx: &StateIndex) -> Vec<StateItem> {
        let Some(oc) = cfg.state.as_ref().and_then(|s| s.opencode.as_ref()) else {
            return Vec::new();
        };
        let Some(bin) = opencode_bin() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for proj in opencode_projects(cfg, oc) {
            for s in list_sessions(&bin, &proj) {
                let item = StateItem {
                    channel: OPENCODE_CHANNEL.into(),
                    key: s.id.clone(),
                    updated: s.updated,
                    meta: Some(proj.display().to_string()),
                    data: String::new(),
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
        out
    }

    fn apply(&self, _cfg: &Config, item: &StateItem) -> Result<String> {
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
}

/// Обновляет `updated` уже известных (синхронизированных) сессий до
/// локального значения, чтобы не пере-экспортировать только что импортированное.
pub(crate) fn refresh_opencode_index(projects: &[PathBuf]) {
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
    if let Some(home) = crate::infra::paths::home_dir() {
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
