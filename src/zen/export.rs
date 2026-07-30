use anyhow::{Context, Result};
use serde_json::Value;
use tracing::info;

use super::lz4::read_mozlz4;
use super::profile::find_profile;
use super::types::ZenExport;
use crate::config::Config;

fn strip_tab(tab: &Value) -> Value {
    let mut clean = tab.clone();
    for key in &["image", "storage", "formdata", "_zenPinnedInitialState"] {
        clean.as_object_mut().map(|m| m.remove(*key));
    }

    if let Some(entries) = clean.get("entries").and_then(|e| e.as_array()) {
        if let Some(last) = entries.last() {
            let url = last.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());
            let title = last.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
            let mut clean_entry = serde_json::Map::new();
            if let Some(u) = url {
                clean_entry.insert("url".into(), Value::String(u));
            }
            if let Some(t) = title {
                clean_entry.insert("title".into(), Value::String(t));
            }
            clean.as_object_mut()
                .map(|m| m.insert("entries".into(), vec![Value::Object(clean_entry)].into()));
        }
    }

    clean
}

pub fn export(cfg: &Config) -> Result<Vec<u8>> {
    let profile = find_profile(&cfg.zen).context("Zen profile not found")?;
    info!("exporting Zen from {}", profile.display());

    let data = ZenExport {
        _source: profile.to_string_lossy().to_string(),
        containers: None,
        themes: None,
        spaces: None,
        groups: None,
        folders: None,
        pinned_tabs: None,
        space_routing: None,
        live_folders: None,
    };

    let mut export = serde_json::to_value(&data)?;
    let obj = export.as_object_mut().unwrap();

    let containers_path = profile.join("containers.json");
    if containers_path.exists() {
        let val: Value = serde_json::from_str(
            &std::fs::read_to_string(&containers_path).context("reading containers.json")?,
        )?;
        obj.insert("containers".into(), val);
    }

    let themes_path = profile.join("zen-themes.json");
    if themes_path.exists() {
        let val: Value = serde_json::from_str(
            &std::fs::read_to_string(&themes_path).context("reading zen-themes.json")?,
        )?;
        obj.insert("themes".into(), val);
    }

    let session_path = profile.join("zen-sessions.jsonlz4");
    if session_path.exists() {
        let sess = read_mozlz4(&session_path)?;

        if let Some(spaces) = sess.get("spaces") {
            obj.insert("spaces".into(), spaces.clone());
        }
        if let Some(groups) = sess.get("groups") {
            obj.insert("groups".into(), groups.clone());
        }
        if let Some(folders) = sess.get("folders") {
            obj.insert("folders".into(), folders.clone());
        }

        if let Some(tabs) = sess.get("tabs").and_then(|t| t.as_array()) {
            let pinned: Vec<Value> = tabs
                .iter()
                .filter(|t| t.get("pinned").and_then(|p| p.as_bool()).unwrap_or(false))
                .map(strip_tab)
                .collect();
            obj.insert("pinned_tabs".into(), pinned.into());
        }
    }

    let routing_path = profile.join("zen-space-routing.jsonlz4");
    if routing_path.exists() {
        obj.insert("space_routing".into(), read_mozlz4(&routing_path)?);
    }

    let live_folders_path = profile.join("zen-live-folders.jsonlz4");
    if live_folders_path.exists() {
        obj.insert("live_folders".into(), read_mozlz4(&live_folders_path)?);
    }

    let json_bytes = serde_json::to_vec_pretty(&export).context("serializing zen export")?;
    info!("Zen export complete ({} bytes)", json_bytes.len());
    Ok(json_bytes)
}
