use std::path::PathBuf;

use anyhow::Result;
use regex::Regex;

use crate::config::ZenConfig;

fn zen_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("zen")
}

pub fn find_profile(cfg: &Option<ZenConfig>) -> Result<PathBuf> {
    if let Some(zen_cfg) = cfg {
        let p = &zen_cfg.profile_path;
        if p.exists() && p.is_dir() {
            return Ok(p.canonicalize()?);
        }
    }

    let zen_dir = zen_config_dir();
    let profiles_ini = zen_dir.join("profiles.ini");
    if !profiles_ini.exists() {
        anyhow::bail!("Zen config not found at {}", zen_dir.display());
    }

    let text = std::fs::read_to_string(&profiles_ini)?;

    let re_install = Regex::new(r"(?m)^\[Install.+?\][\s\S]*?^Default\s*=\s*(.+)$")?;
    if let Some(caps) = re_install.captures(&text) {
        let rel = caps[1].trim();
        let p = zen_dir.join(rel);
        if p.is_dir() {
            return Ok(p.canonicalize()?);
        }
    }

    let re_fallback = Regex::new(r"(?m)^Default\s*=\s*1$[\s\S]*?^Path\s*=\s*(.+)$")?;
    if let Some(caps) = re_fallback.captures(&text) {
        let rel = caps[1].trim();
        let p = zen_dir.join(rel);
        if p.is_dir() {
            return Ok(p.canonicalize()?);
        }
    }

    anyhow::bail!("no active Zen profile found in {}", profiles_ini.display());
}
