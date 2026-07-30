use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde_json::Value;
use tracing::{info, warn};

use super::lz4::{read_mozlz4, write_mozlz4};
use super::profile::find_profile;
use crate::config::Config;

fn merge_groups(
    local: &mut Vec<Value>,
    exported: &[Value],
) -> HashMap<String, String> {
    let local_by_name: HashMap<String, usize> = local
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            g.get("name")
                .and_then(|n| n.as_str())
                .map(|n| (n.to_string(), i))
        })
        .collect();

    let mut id_map: HashMap<String, String> = HashMap::new();

    for eg in exported {
        let name = match eg.get("name").and_then(|n| n.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let old_id = eg.get("id").and_then(|i| i.as_str()).unwrap_or_default();

        if let Some(&idx) = local_by_name.get(name) {
            if let Some(lg) = local.get_mut(idx).and_then(|v| v.as_object_mut()) {
                for key in &["color", "pinned", "collapsed", "saveOnWindowClose"] {
                    if let Some(val) = eg.get(*key) {
                        lg.insert(key.to_string(), val.clone());
                    }
                }
                if !old_id.is_empty() {
                    if let Some(lid) = lg.get("id").and_then(|i| i.as_str()) {
                        id_map.insert(old_id.to_string(), lid.to_string());
                    }
                }
            }
        } else {
            let new_id = generate_id();
            let mut new_group = serde_json::Map::new();
            new_group.insert("id".into(), new_id.clone().into());
            new_group.insert("name".into(), name.into());
            new_group.insert(
                "color".into(),
                eg.get("color")
                    .cloned()
                    .unwrap_or(Value::String("zen-workspace-color".into())),
            );
            new_group.insert(
                "pinned".into(),
                eg.get("pinned").cloned().unwrap_or(Value::Bool(true)),
            );
            new_group.insert(
                "collapsed".into(),
                eg.get("collapsed").cloned().unwrap_or(Value::Bool(false)),
            );
            new_group.insert(
                "splitView".into(),
                eg.get("splitView").cloned().unwrap_or(Value::Bool(false)),
            );
            new_group.insert(
                "saveOnWindowClose".into(),
                eg.get("saveOnWindowClose").cloned().unwrap_or(Value::Bool(true)),
            );
            local.push(Value::Object(new_group));
            if !old_id.is_empty() {
                id_map.insert(old_id.to_string(), new_id);
            }
        }
    }

    id_map
}

fn merge_folders(
    local: &mut Vec<Value>,
    exported: &[Value],
    workspace_id_map: &HashMap<String, String>,
) -> HashMap<String, String> {
    fn folder_key(f: &Value) -> (String, String) {
        (
            f.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
            f.get("workspaceId").and_then(|w| w.as_str()).unwrap_or_default().to_string(),
        )
    }

    let local_idx: HashMap<(String, String), usize> = local
        .iter()
        .enumerate()
        .map(|(i, f)| (folder_key(f), i))
        .collect();

    let mut id_map: HashMap<String, String> = HashMap::new();

    for ef in exported {
        let name = match ef.get("name").and_then(|n| n.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let old_id = ef.get("id").and_then(|i| i.as_str()).unwrap_or_default();
        let ws_id = ef
            .get("workspaceId")
            .and_then(|w| w.as_str())
            .unwrap_or_default()
            .to_string();
        let ws_id = workspace_id_map.get(&ws_id).cloned().unwrap_or(ws_id);
        let key = (name.to_string(), ws_id.clone());

        if let Some(&idx) = local_idx.get(&key) {
            if let Some(lf) = local.get_mut(idx).and_then(|v| v.as_object_mut()) {
                for k in &["collapsed", "pinned", "userIcon", "saveOnWindowClose"] {
                    if let Some(val) = ef.get(*k) {
                        lf.insert(k.to_string(), val.clone());
                    }
                }
                if !old_id.is_empty() {
                    if let Some(fid) = lf.get("id").and_then(|i| i.as_str()) {
                        id_map.insert(old_id.to_string(), fid.to_string());
                    }
                }
            }
        } else {
            let new_id = generate_id();
            let mut nf = serde_json::Map::new();
            nf.insert("id".into(), new_id.clone().into());
            nf.insert("name".into(), name.into());
            nf.insert("workspaceId".into(), ws_id.clone().into());
            nf.insert("pinned".into(), ef.get("pinned").cloned().unwrap_or(Value::Bool(true)));
            nf.insert("collapsed".into(), ef.get("collapsed").cloned().unwrap_or(Value::Bool(false)));
            nf.insert("splitViewGroup".into(), ef.get("splitViewGroup").cloned().unwrap_or(Value::Bool(false)));
            nf.insert("saveOnWindowClose".into(), ef.get("saveOnWindowClose").cloned().unwrap_or(Value::Bool(true)));
            nf.insert("emptyTabIds".into(), Value::Array(vec![]));

            for key in &["prevSiblingInfo", "userIcon"] {
                if let Some(val) = ef.get(*key) {
                    nf.insert(key.to_string(), val.clone());
                }
            }

            if let Some(pid) = ef.get("parentId").and_then(|p| p.as_str()) {
                nf.insert("parentId".into(), id_map.get(pid).cloned().unwrap_or_else(|| pid.to_string()).into());
            }

            local.push(Value::Object(nf));
            if !old_id.is_empty() {
                id_map.insert(old_id.to_string(), new_id);
            }
        }
    }

    id_map
}

fn merge_spaces(
    local: &mut Vec<Value>,
    exported: &[Value],
) -> HashMap<String, String> {
    let local_by_name: HashMap<String, usize> = local
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            s.get("name")
                .and_then(|n| n.as_str())
                .map(|n| (n.to_string(), i))
        })
        .collect();

    let mut uuid_map: HashMap<String, String> = HashMap::new();

    for es in exported {
        let name = match es.get("name").and_then(|n| n.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let old_uuid = es.get("uuid").and_then(|u| u.as_str()).unwrap_or_default();

        if let Some(&idx) = local_by_name.get(name) {
            if let Some(ls) = local.get_mut(idx).and_then(|v| v.as_object_mut()) {
                for key in &["icon", "theme", "hasCollapsedPinnedTabs"] {
                    if let Some(val) = es.get(*key) {
                        ls.insert(key.to_string(), val.clone());
                    }
                }
                if !old_uuid.is_empty() {
                    if let Some(luuid) = ls.get("uuid").and_then(|u| u.as_str()) {
                        uuid_map.insert(old_uuid.to_string(), luuid.to_string());
                    }
                }
            }
        } else {
            let new_uuid = generate_uuid();
            let mut ns = serde_json::Map::new();
            ns.insert("uuid".into(), new_uuid.clone().into());
            ns.insert("name".into(), name.into());

            let default_icon = Value::String(
                "chrome://browser/skin/zen-icons/selectable/circle.svg".into()
            );
            ns.insert("icon".into(), es.get("icon").cloned().unwrap_or(default_icon));

            let default_theme = serde_json::json!({
                "type": "gradient",
                "gradientColors": [],
                "opacity": 0.5,
                "texture": 0,
            });
            ns.insert("theme".into(), es.get("theme").cloned().unwrap_or(default_theme));
            ns.insert("hasCollapsedPinnedTabs".into(), Value::Bool(false));

            if let Some(ct) = es.get("containerTabId") {
                ns.insert("containerTabId".into(), ct.clone());
            }

            local.push(Value::Object(ns));
            if !old_uuid.is_empty() {
                uuid_map.insert(old_uuid.to_string(), new_uuid);
            }
        }
    }

    uuid_map
}

fn reconcile_group_folder_ids(
    groups: &[Value],
    folders: &mut Vec<Value>,
    folder_id_map: &mut HashMap<String, String>,
) {
    let groups_by_name: HashMap<&str, &Value> = groups
        .iter()
        .filter_map(|g| {
            g.get("name")
                .and_then(|n| n.as_str())
                .map(|n| (n, g))
        })
        .collect();

    let mut old_to_new: HashMap<String, String> = HashMap::new();

    for f in folders.iter_mut() {
        let name = match f.get("name").and_then(|n| n.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let g = match groups_by_name.get(name) {
            Some(g) => g,
            None => continue,
        };

        let gid = g.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
        let fid = f.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();

        if !gid.is_empty() && !fid.is_empty() && gid != fid {
            old_to_new.insert(fid.clone(), gid.clone());
            if let Some(fobj) = f.as_object_mut() {
                fobj.insert("id".into(), gid.clone().into());
                if let Some(gws) = g.get("workspaceId").and_then(|w| w.as_str()) {
                    fobj.insert("workspaceId".into(), gws.into());
                }
            }
        }
    }

    for (old_id, new_id) in &old_to_new {
        folder_id_map.insert(old_id.clone(), new_id.clone());
    }

    for f in folders.iter_mut() {
        if let Some(pid) = f.get("parentId").and_then(|p| p.as_str()) {
            if let Some(mapped) = old_to_new.get(pid) {
                f.as_object_mut()
                    .map(|o| o.insert("parentId".into(), mapped.clone().into()));
            }
        }
    }
}

fn clean_sessionstore(profile: &std::path::Path) {
    let sessionstore = profile.join("sessionstore.jsonlz4");
    if sessionstore.exists() {
        std::fs::remove_file(&sessionstore).ok();
        info!("sessionstore.jsonlz4 deleted");
    }

    let backups = profile.join("sessionstore-backups");
    if backups.is_dir() {
        std::fs::remove_dir_all(&backups).ok();
        info!("sessionstore-backups/ deleted");
    }
}

fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (nanos % 10_000_000_000_000_000_000).to_string()
}

fn generate_uuid() -> String {
    let u = uuid::Uuid::new_v4();
    format!("{{{}}}", u.to_string().to_uppercase())
}

pub fn import(cfg: &Config, data: &[u8]) -> Result<()> {
    let export: Value = serde_json::from_slice(data).context("parsing zen export json")?;
    let profile = find_profile(&cfg.zen).context("Zen profile not found")?;

    info!("importing Zen to {}", profile.display());

    if let Some(containers) = export.get("containers") {
        let path = profile.join("containers.json");
        std::fs::write(&path, serde_json::to_vec_pretty(containers)?)
            .context("writing containers.json")?;
        info!("containers.json updated");
    }

    if let Some(themes) = export.get("themes") {
        let path = profile.join("zen-themes.json");
        std::fs::write(&path, serde_json::to_vec_pretty(themes)?)
            .context("writing zen-themes.json")?;
        info!("zen-themes.json updated");
    }

    let session_path = profile.join("zen-sessions.jsonlz4");
    let exported_spaces = export.get("spaces").and_then(|v| v.as_array());
    let exported_groups = export.get("groups").and_then(|v| v.as_array());
    let exported_folders = export.get("folders").and_then(|v| v.as_array());
    let exported_pinned = export.get("pinned_tabs").and_then(|v| v.as_array());
    let exported_live_folders = export.get("live_folders").and_then(|v| v.as_array());

    if session_path.exists() {
        let mut local: Value = read_mozlz4(&session_path)?;

        let old_spaces = local.get("spaces").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        let old_groups = local.get("groups").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        let old_folders = local.get("folders").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

        let mut workspace_uuid_map: HashMap<String, String> = HashMap::new();
        if let Some(espaces) = exported_spaces {
            let mut spaces = local.get("spaces").cloned().unwrap_or(Value::Array(vec![]));
            if let Some(arr) = spaces.as_array_mut() {
                workspace_uuid_map = merge_spaces(arr, espaces);
            }
            local.as_object_mut().map(|o| o.insert("spaces".into(), spaces));
        }

        let mut group_id_map: HashMap<String, String> = HashMap::new();
        if let Some(egroups) = exported_groups {
            let mut groups = local.get("groups").cloned().unwrap_or(Value::Array(vec![]));
            if let Some(arr) = groups.as_array_mut() {
                group_id_map = merge_groups(arr, egroups);
            }
            local.as_object_mut().map(|o| o.insert("groups".into(), groups));
        }

        let mut folder_id_map: HashMap<String, String> = HashMap::new();
        if let Some(efolders) = exported_folders {
            let mut folders = local.get("folders").cloned().unwrap_or(Value::Array(vec![]));
            if let Some(arr) = folders.as_array_mut() {
                folder_id_map = merge_folders(arr, efolders, &workspace_uuid_map);
            }
            local.as_object_mut().map(|o| o.insert("folders".into(), folders));
        }

        let groups = local.get("groups").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut folders = local.get("folders").cloned().unwrap_or(Value::Array(vec![]));
        reconcile_group_folder_ids(&groups, folders.as_array_mut().unwrap(), &mut folder_id_map);
        local.as_object_mut().map(|o| o.insert("folders".into(), folders));

        if let Some(pinned) = exported_pinned {
            let local_tabs = local.get("tabs").cloned().unwrap_or(Value::Array(vec![]));

            let new_pinned: Vec<Value> = pinned
                .iter()
                .filter_map(|pt| {
                    let entries = pt.get("entries").and_then(|e| e.as_array())?;
                    let entry = entries.first()?;
                    let url = entry.get("url").and_then(|u| u.as_str())?;
                    if url.is_empty() || url == "about:blank" {
                        return None;
                    }

                    let group_id = pt.get("groupId").and_then(|g| g.as_str())
                        .and_then(|g| group_id_map.get(g)).cloned();
                    let zen_workspace = pt.get("zenWorkspace").and_then(|w| w.as_str())
                        .and_then(|w| workspace_uuid_map.get(w)).cloned();
                    let folder_id = pt.get("zenLiveFolderItemId").and_then(|f| f.as_str())
                        .and_then(|f| folder_id_map.get(f)).cloned();
                    let title = entry.get("title").and_then(|t| t.as_str()).unwrap_or_default();

                    Some(serde_json::json!({
                        "entries": [{
                            "url": url,
                            "title": title,
                            "triggeringPrincipal_base64": "{}"
                        }],
                        "lastAccessed": 0,
                        "pinned": true,
                        "hidden": false,
                        "index": null,
                        "groupId": group_id,
                        "zenWorkspace": zen_workspace,
                        "zenLiveFolderItemId": folder_id,
                        "zenSyncId": pt.get("zenSyncId").and_then(|s| s.as_str()),
                        "userContextId": pt.get("userContextId").and_then(|c| c.as_i64()).unwrap_or(0),
                        "attributes": {}
                    }))
                })
                .enumerate()
                .map(|(i, mut t)| {
                    t.as_object_mut().map(|o| o.insert("index".into(), Value::Number((i + 1).into())));
                    t
                })
                .collect();

            let non_pinned: Vec<Value> = local_tabs
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter(|t| !t.get("pinned").and_then(|p| p.as_bool()).unwrap_or(false))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();

            let mut all_tabs = non_pinned;
            all_tabs.extend(new_pinned);
            local.as_object_mut().map(|o| o.insert("tabs".into(), all_tabs.into()));
        }

        if let Some(lf) = exported_live_folders {
            let mut live_folders = local.get("liveFolders").cloned().unwrap_or(Value::Array(vec![]));
            let existing_ids: HashSet<String> = live_folders
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|f| f.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            if let Some(arr) = live_folders.as_array_mut() {
                for lf_item in lf {
                    if let Some(id) = lf_item.get("id").and_then(|i| i.as_str()) {
                        if !existing_ids.contains(id) {
                            arr.push(lf_item.clone());
                        }
                    }
                }
            }
            local.as_object_mut().map(|o| o.insert("liveFolders".into(), live_folders));
        }

        let new_spaces = local.get("spaces").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0) - old_spaces;
        let new_groups = local.get("groups").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0) - old_groups;
        let new_folders = local.get("folders").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0) - old_folders;

        write_mozlz4(&session_path, &local)?;
        info!("zen-sessions.jsonlz4 updated");

        if new_spaces > 0 { info!("  new spaces: {new_spaces}"); }
        if new_groups > 0 { info!("  new groups: {new_groups}"); }
        if new_folders > 0 { info!("  new folders: {new_folders}"); }

        let pinned_count = exported_pinned.map(|a| a.len()).unwrap_or(0);
        if pinned_count > 0 { info!("  pinned tabs: {pinned_count}"); }
        let total = local.get("tabs").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        info!("  total tabs: {total}");
    } else {
        warn!("zen-sessions.jsonlz4 not found, creating new");
        let mut new_sess = serde_json::json!({
            "lastCollected": 0,
            "tabs": [],
            "folders": [],
            "groups": [],
            "spaces": [],
        });

        if let Some(ss) = exported_spaces {
            new_sess
                .as_object_mut()
                .map(|o| o.insert("spaces".into(), ss.to_vec().into()));
        }
        if let Some(gs) = exported_groups {
            new_sess
                .as_object_mut()
                .map(|o| o.insert("groups".into(), gs.to_vec().into()));
        }
        if let Some(fs) = exported_folders {
            new_sess
                .as_object_mut()
                .map(|o| o.insert("folders".into(), fs.to_vec().into()));
        }

        write_mozlz4(&session_path, &new_sess)?;
        info!("zen-sessions.jsonlz4 created");
    }

    if let Some(routing) = export.get("space_routing") {
        write_mozlz4(&profile.join("zen-space-routing.jsonlz4"), routing)?;
        info!("zen-space-routing.jsonlz4 updated");
    }

    if let Some(lf) = export.get("live_folders") {
        write_mozlz4(&profile.join("zen-live-folders.jsonlz4"), lf)?;
        info!("zen-live-folders.jsonlz4 updated");
    }

    clean_sessionstore(&profile);
    info!("Zen import complete");

    Ok(())
}
