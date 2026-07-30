use std::path::PathBuf;

use anyhow::Result;
use regex::Regex;

use crate::config::ZenConfig;

pub fn find_profile(cfg: &Option<ZenConfig>) -> Result<PathBuf> {
    let zen_config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/home"))
        .join("zen");

    // If config explicitly provides a profile path, use it
    if let Some(zen_cfg) = cfg {
        let p = &zen_cfg.profile_path;
        if p.is_dir() {
            return Ok(p.canonicalize()?);
        }
    }

    // Otherwise, discover the default profile from profiles.ini
    let profiles_ini = zen_config_dir.join("profiles.ini");
    if !profiles_ini.exists() {
        anyhow::bail!("Zen config not found at {}", zen_config_dir.display());
    }

    let text = std::fs::read_to_string(&profiles_ini)?;

    let re_install = Regex::new(r"(?m)^\[Install.+?\][\s\S]*?^Default\s*=\s*(.+)$")?;
    if let Some(caps) = re_install.captures(&text) {
        let rel = caps[1].trim();
        let p = zen_config_dir.join(rel);
        if p.is_dir() {
            return Ok(p.canonicalize()?);
        }
    }

    let re_fallback = Regex::new(r"(?m)^Default\s*=\s*1$[\s\S]*?^Path\s*=\s*(.+)$")?;
    if let Some(caps) = re_fallback.captures(&text) {
        let rel = caps[1].trim();
        let p = zen_config_dir.join(rel);
        if p.is_dir() {
            return Ok(p.canonicalize()?);
        }
    }

    anyhow::bail!("no active Zen profile found in {}", profiles_ini.display());
}
