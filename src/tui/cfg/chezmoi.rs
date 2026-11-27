use super::*;

/// Живёт ли файл под управлением chezmoi.
pub(crate) fn is_chezmoi_managed(path: &Path) -> bool {
    std::process::Command::new("chezmoi")
        .args(["source-path"])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Path к source-файлу chezmoi для живого файла (`chezmoi source-path <target>`).
pub(crate) fn chezmoi_source_path(path: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("chezmoi")
        .args(["source-path"])
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

/// `<target>` для `chezmoi apply`: живой путь относительно home (`.config/...`).
pub(crate) fn live_target(live: &Path) -> Option<String> {
    let home = crate::infra::paths::home_dir()?;
    live.strip_prefix(&home)
        .ok()
        .map(|r| r.display().to_string())
}
