use super::*;
use toml_edit::{Item, Table, Value};

pub(crate) fn apply_doc_patch(doc: &mut DocumentMut, patch: &ConfigPatch) -> Result<Vec<String>> {
    let mut diff = Vec::new();
    match patch {
        ConfigPatch::UpsertProject { name, proj } => {
            let projects_item = doc.entry("projects").or_insert(Item::Table(Table::new()));
            let table = projects_item
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("[projects] is not a table"))?;
            let existed = table.get(name).is_some();
            let old = if existed {
                table
                    .get(name)
                    .and_then(|i| i.as_table())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Table::new()
            };
            table.insert(name, Item::Table(project_table(proj)));
            if existed {
                diff.extend(changed_keys(name, &old, proj));
            } else {
                diff.push(format!(
                    "+ projects.{name} added (path = {}, branch = {}, machines = [{}])",
                    proj.path.display(),
                    proj.branch.as_deref().unwrap_or("-"),
                    proj.machines
                        .as_deref()
                        .map(|m| m.join(", "))
                        .unwrap_or_default()
                ));
            }
        }
        ConfigPatch::RemoveProject { name } => {
            if let Some(projects) = doc.get_mut("projects").and_then(|i| i.as_table_mut()) {
                if projects.remove(name).is_some() {
                    if projects.is_empty() {
                        doc.as_table_mut().remove("projects");
                    }
                    diff.push(format!("- projects.{name} removed"));
                } else {
                    anyhow::bail!("project {name:?} not in config");
                }
            } else {
                anyhow::bail!("project {name:?} not in config");
            }
        }
        ConfigPatch::UpsertRemote { name, remote } => {
            let remotes_item = doc.entry("remote").or_insert(Item::Table(Table::new()));
            let table = remotes_item
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("[remote] is not a table"))?;
            let existed = table.get(name).is_some();
            table.insert(name, Item::Table(remote_table(remote)));
            if existed {
                diff.push(format!(
                    "~ remote.{name} updated (host={}:{}, user={})",
                    remote.host, remote.port, remote.user
                ));
            } else {
                diff.push(format!(
                    "+ remote.{name} added ({}@{}:{})",
                    remote.user, remote.host, remote.port
                ));
            }
        }
        ConfigPatch::RemoveRemote { name } => {
            if let Some(remotes) = doc.get_mut("remote").and_then(|i| i.as_table_mut()) {
                if remotes.remove(name).is_some() {
                    if remotes.is_empty() {
                        doc.as_table_mut().remove("remote");
                    }
                    diff.push(format!("- remote.{name} removed"));
                } else {
                    anyhow::bail!("remote machine {name:?} not in config");
                }
            } else {
                anyhow::bail!("remote machine {name:?} not in config");
            }
        }
        ConfigPatch::State {
            tmux,
            tmux_restore,
            opencode_projects,
        } => {
            if tmux.is_none() && tmux_restore.is_none() && opencode_projects.is_none() {
                return Ok(diff);
            }
            let state_item = doc.entry("state").or_insert(Item::Table(Table::new()));
            let state = state_item
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("[state] is not a table"))?;
            if let Some(v) = tmux {
                let old = state
                    .get("tmux")
                    .and_then(|i| i.as_value())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "(absent)".into());
                state.insert("tmux", Item::Value(Value::from(*v)));
                diff.push(format!("~ state.tmux: {old} → {v}"));
            }
            if let Some(v) = tmux_restore {
                let old = state
                    .get("tmux_restore")
                    .and_then(|i| i.as_value())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "(absent)".into());
                state.insert("tmux_restore", Item::Value(Value::from(*v)));
                diff.push(format!("~ state.tmux_restore: {old} → {v}"));
            }
            if let Some(list) = opencode_projects {
                let occ_item = state.entry("opencode").or_insert(Item::Table(Table::new()));
                let occ = occ_item
                    .as_table_mut()
                    .ok_or_else(|| anyhow::anyhow!("[state.opencode] is not a table"))?;
                if list.is_empty() {
                    if occ.remove("projects").is_some() {
                        diff.push("~ state.opencode.projects: cleared (sync all projects)".into());
                    }
                } else {
                    let mut arr = toml_edit::Array::new();
                    for p in list {
                        arr.push(Value::from(p.as_str()));
                    }
                    occ.insert("projects", Item::Value(Value::Array(arr)));
                    diff.push(format!(
                        "~ state.opencode.projects: set ({} path(s))",
                        list.len()
                    ));
                }
                if occ.is_empty() {
                    state.remove("opencode");
                }
            }
        }
    }
    Ok(diff)
}

pub(crate) fn project_table(proj: &ProjectConfig) -> Table {
    let mut t = Table::new();
    t.insert(
        "path",
        Item::Value(Value::from(proj.path.display().to_string())),
    );
    if let Some(b) = &proj.branch {
        t.insert("branch", Item::Value(Value::from(b.clone())));
    }
    if let Some(m) = &proj.machines {
        let mut arr = toml_edit::Array::new();
        for s in m {
            arr.push(Value::from(s.as_str()));
        }
        t.insert("machines", Item::Value(Value::Array(arr)));
    }
    if let Some(p) = &proj.post_pull {
        t.insert("post_pull", Item::Value(Value::from(p.clone())));
    }
    t
}

pub(crate) fn remote_table(r: &RemoteMachine) -> Table {
    let mut t = Table::new();
    t.insert("host", Item::Value(Value::from(r.host.as_str())));
    t.insert("port", Item::Value(Value::from(i64::from(r.port))));
    t.insert("user", Item::Value(Value::from(r.user.as_str())));
    t
}

/// Строки "~ key: old → new" для изменившихся полей существующего проекта.
pub(crate) fn changed_keys(name: &str, old: &Table, proj: &ProjectConfig) -> Vec<String> {
    let mut out = Vec::new();
    let field = |t: &Table, k: &str| -> Option<String> {
        t.get(k).and_then(|i| i.as_value()).map(|v| v.to_string())
    };
    let new_path = proj.path.display().to_string();
    if field(old, "path") != Some(new_path.clone()) {
        out.push(format!(
            "~ projects.{name}.path: {} → {}",
            field(old, "path").unwrap_or_else(|| "(absent)".into()),
            new_path
        ));
    }
    let new_branch = proj.branch.clone();
    if field(old, "branch") != new_branch {
        out.push(format!(
            "~ projects.{name}.branch: {} → {}",
            field(old, "branch").unwrap_or_else(|| "(absent)".into()),
            new_branch.unwrap_or_else(|| "(absent)".into())
        ));
    }
    // machines: сравниваем списком
    let old_machines: Vec<String> = old
        .get("machines")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let new_machines = proj.machines.clone().unwrap_or_default();
    if old_machines != new_machines {
        out.push(format!(
            "~ projects.{name}.machines: [{}] → [{}]",
            old_machines.join(", "),
            new_machines.join(", ")
        ));
    }
    let new_post = proj.post_pull.clone();
    if field(old, "post_pull") != new_post {
        out.push(format!(
            "~ projects.{name}.post_pull: {} → {}",
            field(old, "post_pull").unwrap_or_else(|| "(absent)".into()),
            new_post.unwrap_or_else(|| "(absent)".into())
        ));
    }
    out
}

/// Текстовый движок для шаблонов с Go-template синтаксисом (не TOML целиком):
/// правка только регионов `[projects.X]` / `[remote.X]` / `[state]`,
/// всё остальное (включая `{{ }}`-блоки) остаётся нетронутым.
pub(crate) fn patch_template_text(
    content: &str,
    patch: &ConfigPatch,
) -> Result<(String, Vec<String>)> {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut diff = Vec::new();
    match patch {
        ConfigPatch::UpsertProject { name, proj } => {
            let block = format!("[projects.{name}]\n{}", project_table(proj));
            let added = upsert_table(
                &mut lines,
                &format!("[projects.{name}]"),
                &block,
                "projects.",
            );
            if added {
                diff.push(format!(
                    "+ projects.{name} added (path = {})",
                    proj.path.display()
                ));
            } else {
                diff.push(format!("~ projects.{name} updated"));
            }
        }
        ConfigPatch::RemoveProject { name } => {
            if remove_table(&mut lines, "projects.", name) {
                diff.push(format!("- projects.{name} removed"));
            } else {
                anyhow::bail!("project {name:?} not in config");
            }
        }
        ConfigPatch::UpsertRemote { name, remote } => {
            let block = format!("[remote.{name}]\n{}", remote_table(remote));
            let added = upsert_table(&mut lines, &format!("[remote.{name}]"), &block, "remote.");
            if added {
                diff.push(format!("+ remote.{name} added"));
            } else {
                diff.push(format!("~ remote.{name} updated"));
            }
        }
        ConfigPatch::RemoveRemote { name } => {
            if remove_table(&mut lines, "remote.", name) {
                diff.push(format!("- remote.{name} removed"));
            } else {
                anyhow::bail!("remote machine {name:?} not in config");
            }
        }
        ConfigPatch::State {
            tmux,
            tmux_restore,
            opencode_projects,
        } => {
            if let Some(v) = tmux {
                set_key_in_block(
                    &mut lines,
                    "[state]",
                    "tmux",
                    Some(v.to_string()),
                    &mut diff,
                );
            }
            if let Some(v) = tmux_restore {
                set_key_in_block(
                    &mut lines,
                    "[state]",
                    "tmux_restore",
                    Some(v.to_string()),
                    &mut diff,
                );
            }
            if let Some(list) = opencode_projects {
                let value = if list.is_empty() {
                    None
                } else {
                    Some(format!(
                        "[{}]",
                        list.iter()
                            .map(|p| format!("\"{p}\""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                };
                set_key_in_block(&mut lines, "[state.opencode]", "projects", value, &mut diff);
            }
        }
    }
    Ok((lines.join("\n"), diff))
}

pub(crate) fn is_header(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with('[') && t.ends_with(']')
}

pub(crate) fn is_template_marker(l: &str) -> bool {
    l.trim_start().starts_with("{{")
}

/// Находит индекс конца блока таблицы: следующая строка-заголовок `[`,
/// маркер `{{` или конец файла.
pub(crate) fn block_end(lines: &[String], from: usize) -> usize {
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(from + 1) {
        if is_header(l) || is_template_marker(l) {
            end = i;
            break;
        }
    }
    end
}

/// Вставит блок на место существующей таблицы или добавит новую рядом с
/// другими таблицами того же префикса. Возвращает true, если таблицы не было.
pub(crate) fn upsert_table(
    lines: &mut Vec<String>,
    header: &str,
    block: &str,
    prefix: &str,
) -> bool {
    let pos = lines.iter().position(|l| l.trim() == header);
    let block_lines: Vec<String> = block.lines().map(|l| l.to_string()).collect();
    if let Some(p) = pos {
        let end = block_end(lines, p);
        lines.splice(p..end, block_lines);
        return false;
    }
    // Новый блок: после последней таблицы с таким префиксом
    let last = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| l.trim().starts_with(&format!("[{prefix}")))
        .map(|(i, _)| i);
    let insert_at = match last {
        Some(i) => block_end(lines, i),
        None => lines
            .iter()
            .position(|l| is_header(l) || is_template_marker(l))
            .unwrap_or(lines.len()),
    };
    let mut out = lines[..insert_at].to_vec();
    out.extend(block_lines);
    out.extend(lines[insert_at..].iter().cloned());
    *lines = out;
    true
}

/// Удаляет блок таблицы (заголовок + тело до следующего заголовка/`{{`).
pub(crate) fn remove_table(lines: &mut Vec<String>, prefix: &str, name: &str) -> bool {
    let header = format!("[{prefix}{name}]");
    let pos = lines.iter().position(|l| l.trim() == header);
    let Some(p) = pos else {
        return false;
    };
    let end = block_end(lines, p);
    lines.drain(p..end);
    true
}

/// Меняет/добавляет/удаляет ключ внутри блока таблицы (сохраняет комментарии).
pub(crate) fn set_key_in_block(
    lines: &mut Vec<String>,
    header: &str,
    key: &str,
    value: Option<String>,
    diff: &mut Vec<String>,
) {
    let Some(p) = lines.iter().position(|l| l.trim() == header) else {
        return;
    };
    let end = block_end(lines, p);
    let mut replaced = false;
    for i in p..end {
        let l = &lines[i];
        let trimmed = l.trim_start();
        // Строка вида `key = ...` (сохраняем возможные inline-комментарии? нет)
        if let Some(rest) = trimmed.strip_prefix(key) {
            if rest.starts_with('=')
                || (rest.starts_with(' ') && rest.trim_start().starts_with('='))
            {
                match &value {
                    Some(v) => {
                        let indent = &l[..l.len() - l.trim_start().len()];
                        lines[i] = format!("{indent}{key} = {v}");
                        diff.push(format!("~ {header} {key}: set to {v}"));
                    }
                    None => {
                        lines.remove(i);
                        diff.push(format!("~ {header} {key}: removed"));
                    }
                }
                replaced = true;
                break;
            }
        }
    }
    if !replaced {
        if let Some(v) = value {
            lines.insert(p + 1, format!("{key} = {v}"));
            diff.push(format!("~ {header} {key}: set to {v}"));
        }
    }
}
