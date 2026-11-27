//! Канал tmux: снапшот tmux-resurrect этой машины.

use super::*;

pub(crate) struct TmuxChannel;

impl StateChannel for TmuxChannel {
    fn name(&self) -> &'static str {
        TMUX_CHANNEL
    }

    fn wanted(&self, cfg: &Config) -> bool {
        cfg.state.as_ref().map(|s| s.tmux).unwrap_or(false)
    }

    fn accepts(&self, cfg: &Config, item: &StateItem) -> bool {
        self.wanted(cfg) && item.key == cfg.machine.name
    }

    fn collect(&self, cfg: &Config, idx: &StateIndex) -> Vec<StateItem> {
        let Some(dir) = tmux_resurrect_dir() else {
            return Vec::new();
        };
        if !dir.is_dir() {
            let _ = std::fs::create_dir_all(&dir);
        }
        let before = newest_resurrect_file(&dir).map(|(_, m)| m);
        trigger_tmux_save(&dir, before);

        let Some((path, _)) = newest_resurrect_file(&dir) else {
            return Vec::new();
        };
        let Ok(data) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let item = StateItem {
            channel: TMUX_CHANNEL.into(),
            key: cfg.machine.name.clone(),
            updated: content_hash(&data),
            meta: path.file_name().map(|s| s.to_string_lossy().to_string()),
            data,
            ..Default::default()
        };
        if idx.has_exact(&item) {
            Vec::new()
        } else {
            vec![item]
        }
    }

    fn apply(&self, cfg: &Config, item: &StateItem) -> Result<String> {
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
}

/// Стабильный положительный i64-хеш содержимого (для `updated`).
pub(crate) fn content_hash(data: &str) -> i64 {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    i64::from_be_bytes(b) & i64::MAX
}

fn tmux_resurrect_dir() -> Option<PathBuf> {
    let home = crate::infra::paths::home_dir()?;
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

/// Самый свежий снапшот resurrect: (путь, время модификации).
///
/// Сравнение идёт по `SystemTime` (наносекунды), а не по секундам эпохи:
/// при секундной точности новый снапшот, записанный в ту же секунду, что и
/// предыдущий, не считался новее — и `trigger_tmux_save` каждый раз ждал
/// полный таймаут (3 с) на каждом push.
fn newest_resurrect_file(dir: &Path) -> Option<(PathBuf, std::time::SystemTime)> {
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("tmux_resurrect_") || !name.ends_with(".txt") {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        let Ok(modified) = md.modified() else {
            continue;
        };
        if best.as_ref().map(|(_, m)| modified > *m).unwrap_or(true) {
            best = Some((entry.path(), modified));
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

fn trigger_tmux_save(dir: &Path, prev_mtime: Option<std::time::SystemTime>) {
    if !tmux_running() {
        return;
    }
    let Some(script) = tmux_resurrect_script("save.sh") else {
        return;
    };
    let _ = Command::new("tmux")
        .args(["run-shell", &script.display().to_string()])
        .output();
    // Сохранение из run-shell асинхронное — недолго ждём новый файл
    // (обычно один-два тика по 100 мс).
    for _ in 0..12 {
        if newest_resurrect_file(dir)
            .map(|(_, m)| prev_mtime.map(|p| m > p).unwrap_or(true))
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
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
    let home = crate::infra::paths::home_dir()?;
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
