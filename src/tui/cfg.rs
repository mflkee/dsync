//! Редактирование конфига dsync из TUI: добавление/удаление проектов и
//! машин ([remote]), переключение [state], сохранение в TOML **без потери
//! комментариев и неизвестных секций** (точечные патчи через `toml_edit`).
//!
//! Живой файл может быть под chezmoi: тогда правки уходят в source-шаблон
//! (`chezmoi source-path`) и применяются через `chezmoi apply <target>`.
//! Шаблоны с Go-template синтаксисом (`{{ }}` вне строк) правятся текстовым
//! движком по регионам; обычные TOML-шаблоны — полным `toml_edit`-патчем.

use std::path::{Path, PathBuf};

use anyhow::Result;
use toml_edit::{DocumentMut, Item, Table, Value};

use crate::config::{self, Config, ProjectConfig, RemoteMachine};

/// Снимок конфига для отображения в UI (не мутируется на UI-потоке).
#[derive(Debug, Clone)]
pub struct CfgSummary {
    pub machine: String,
    pub hub_connect: Option<String>,
    pub has_hub_section: bool,
    pub projects: Vec<CfgProject>,
    pub remotes: Vec<CfgRemote>,
    pub config_path: String,
    pub chezmoi_managed: bool,
    /// Резолвленный путь к source-шаблону chezmoi (None = не managed/нет шаблона).
    pub chezmoi_template: Option<String>,
    /// Эффективная конфигурация [state] (с учётом дефолтов).
    pub state: CfgState,
}

/// Эффективное состояние секции [state] для вкладки State.
#[derive(Debug, Clone)]
pub struct CfgState {
    /// Была ли секция [state] в конфиге вовсе.
    pub configured: bool,
    pub tmux: bool,
    pub tmux_restore: bool,
    /// Список путей проектов opencode; None = синхронизируются все.
    pub opencode_projects: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct CfgProject {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub machines: Vec<String>,
    pub post_pull: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CfgRemote {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
}

/// Точечное изменение конфига: применяется и к TOML-документу, и к
/// in-memory `Config`. Никогда не переписывает файл целиком.
#[derive(Debug, Clone)]
pub enum ConfigPatch {
    UpsertProject {
        name: String,
        proj: ProjectConfig,
    },
    RemoveProject {
        name: String,
    },
    UpsertRemote {
        name: String,
        remote: RemoteMachine,
    },
    RemoveRemote {
        name: String,
    },
    State {
        tmux: Option<bool>,
        tmux_restore: Option<bool>,
        opencode_projects: Option<Vec<String>>,
    },
}

/// Редактор конфига: живёт в backend-потоке, мутирует `cfg` и пишет файл.
#[derive(Debug)]
pub struct ConfigEditor {
    pub live_path: PathBuf,
    pub chezmoi_managed: bool,
    /// Путь к source-шаблону chezmoi (resolved), если managed.
    pub chezmoi_source: Option<PathBuf>,
    pub cfg: Config,
}

impl ConfigEditor {
    pub fn load() -> Result<Self> {
        let live_path =
            config::find_config_path().ok_or_else(|| anyhow::anyhow!("config not found"))?;
        let chezmoi_managed = is_chezmoi_managed(&live_path);
        let chezmoi_source = if chezmoi_managed {
            chezmoi_source_path(&live_path)
        } else {
            None
        };
        let cfg = Config::load()?;
        Ok(Self {
            live_path,
            chezmoi_managed,
            chezmoi_source,
            cfg,
        })
    }

    pub fn summary(&self) -> CfgSummary {
        let mut projects: Vec<CfgProject> = self
            .cfg
            .projects
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(name, p)| CfgProject {
                        name: name.clone(),
                        path: p.path.display().to_string(),
                        branch: p.branch.clone(),
                        machines: p.machines.clone().unwrap_or_default(),
                        post_pull: p.post_pull.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        projects.sort_by(|a, b| a.name.cmp(&b.name));

        let mut remotes: Vec<CfgRemote> = self
            .cfg
            .remote
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(name, r)| CfgRemote {
                        name: name.clone(),
                        host: r.host.clone(),
                        port: r.port,
                        user: r.user.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        remotes.sort_by(|a, b| a.name.cmp(&b.name));

        let configured = self.cfg.state.is_some();
        let st = self.cfg.state.as_ref();
        let state = CfgState {
            configured,
            tmux: st.map(|s| s.tmux).unwrap_or(true),
            tmux_restore: st.map(|s| s.tmux_restore).unwrap_or(false),
            opencode_projects: st
                .and_then(|s| s.opencode.as_ref())
                .and_then(|o| o.projects.as_ref())
                .map(|ps| {
                    ps.iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                }),
        };

        CfgSummary {
            machine: self.cfg.machine.name.clone(),
            hub_connect: self.cfg.hub_connect.as_ref().map(|h| h.address.clone()),
            has_hub_section: self.cfg.hub.is_some(),
            projects,
            remotes,
            config_path: self.live_path.display().to_string(),
            chezmoi_managed: self.chezmoi_managed,
            chezmoi_template: self.chezmoi_source.as_ref().map(|p| p.display().to_string()),
            state,
        }
    }

    // --- мутации ---

    pub fn add_project(
        &mut self,
        name: &str,
        path: &str,
        branch: Option<&str>,
        machines: &[String],
        post_pull: Option<&str>,
        use_template: bool,
    ) -> Result<Vec<String>> {
        if name.trim().is_empty() || path.trim().is_empty() {
            anyhow::bail!("name and path are required");
        }
        let proj = ProjectConfig {
            path: PathBuf::from(path.trim()),
            branch: branch
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            machines: Some(machines.to_vec()).filter(|v| !v.is_empty()),
            post_pull: post_pull
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        };
        self.apply_patch(
            &ConfigPatch::UpsertProject {
                name: name.trim().to_string(),
                proj,
            },
            use_template,
        )
    }

    pub fn remove_project(&mut self, name: &str, use_template: bool) -> Result<Vec<String>> {
        self.apply_patch(
            &ConfigPatch::RemoveProject {
                name: name.to_string(),
            },
            use_template,
        )
    }

    pub fn add_remote(
        &mut self,
        name: &str,
        host: &str,
        port: u16,
        user: &str,
        use_template: bool,
    ) -> Result<Vec<String>> {
        if name.trim().is_empty() || host.trim().is_empty() || user.trim().is_empty() {
            anyhow::bail!("name, host and user are required");
        }
        if port == 0 {
            anyhow::bail!("port must be 1..65535");
        }
        let remote = RemoteMachine {
            host: host.trim().to_string(),
            port,
            user: user.trim().to_string(),
        };
        self.apply_patch(
            &ConfigPatch::UpsertRemote {
                name: name.trim().to_string(),
                remote,
            },
            use_template,
        )
    }

    pub fn remove_remote(&mut self, name: &str, use_template: bool) -> Result<Vec<String>> {
        self.apply_patch(
            &ConfigPatch::RemoveRemote {
                name: name.to_string(),
            },
            use_template,
        )
    }

    /// Применить изменение секции [state] (tmux/tmux_restore/opencode-список).
    pub fn apply_state(
        &mut self,
        tmux: Option<bool>,
        tmux_restore: Option<bool>,
        opencode_projects: Option<Vec<String>>,
        use_template: bool,
    ) -> Result<Vec<String>> {
        self.apply_patch(
            &ConfigPatch::State {
                tmux,
                tmux_restore,
                opencode_projects,
            },
            use_template,
        )
    }

    /// Основа всех мутаций: точечный патч файла (живого или source-шаблона)
    /// + обновление in-memory cfg. Возвращает строки diff-сводки.
    pub fn apply_patch(&mut self, patch: &ConfigPatch, use_template: bool) -> Result<Vec<String>> {
        let diff: Vec<String> = if use_template {
            let src = self.chezmoi_source.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "chezmoi-managed config: source template not found — edit the template directly"
                )
            })?;
            if !src.exists() {
                anyhow::bail!(
                    "chezmoi source template {} does not exist — refusing to write the live config",
                    src.display()
                );
            }
            let content = std::fs::read_to_string(&src)?;
            let diff = match content.parse::<DocumentMut>() {
                Ok(mut doc) => {
                    let diff = apply_doc_patch(&mut doc, patch)?;
                    std::fs::write(&src, doc.to_string())?;
                    diff
                }
                Err(_) => {
                    // Шаблон с Go-template синтаксисом (неполный TOML) —
                    // правка текстом по регионам (см. patch_template_text).
                    let (out, diff) = patch_template_text(&content, patch)?;
                    std::fs::write(&src, out)?;
                    diff
                }
            };
            self.apply_chezmoi_apply()?;
            diff
        } else {
            let content = std::fs::read_to_string(&self.live_path).map_err(|e| {
                anyhow::anyhow!("read {}: {e}", self.live_path.display())
            })?;
            let mut doc = content.parse::<DocumentMut>().map_err(|e| {
                anyhow::anyhow!("config parse error ({}): {e}", self.live_path.display())
            })?;
            let diff = apply_doc_patch(&mut doc, patch)?;
            std::fs::write(&self.live_path, doc.to_string()).map_err(|e| {
                anyhow::anyhow!("write {}: {e}", self.live_path.display())
            })?;
            diff
        };
        self.apply_in_memory(patch);
        Ok(diff)
    }

    /// Применить патч к in-memory `Config` (чтобы summary был консистентен).
    fn apply_in_memory(&mut self, patch: &ConfigPatch) {
        match patch {
            ConfigPatch::UpsertProject { name, proj } => {
                let map = self.cfg.projects.get_or_insert_with(Default::default);
                map.insert(name.clone(), proj.clone());
            }
            ConfigPatch::RemoveProject { name } => {
                if let Some(map) = self.cfg.projects.as_mut() {
                    map.remove(name);
                    if map.is_empty() {
                        self.cfg.projects = None;
                    }
                }
            }
            ConfigPatch::UpsertRemote { name, remote } => {
                let map = self.cfg.remote.get_or_insert_with(Default::default);
                map.insert(name.clone(), remote.clone());
            }
            ConfigPatch::RemoveRemote { name } => {
                if let Some(map) = self.cfg.remote.as_mut() {
                    map.remove(name);
                    if map.is_empty() {
                        self.cfg.remote = None;
                    }
                }
            }
            ConfigPatch::State {
                tmux,
                tmux_restore,
                opencode_projects,
            } => {
                if tmux.is_none() && tmux_restore.is_none() && opencode_projects.is_none() {
                    return;
                }
                let state = self.cfg.state.get_or_insert_with(Default::default);
                if let Some(v) = tmux {
                    state.tmux = *v;
                }
                if let Some(v) = tmux_restore {
                    state.tmux_restore = *v;
                }
                if let Some(list) = opencode_projects {
                    if list.is_empty() {
                        if let Some(o) = state.opencode.as_mut() {
                            o.projects = None;
                        }
                    } else {
                        state
                            .opencode
                            .get_or_insert_with(Default::default)
                            .projects = Some(
                            list.iter()
                                .map(|s| PathBuf::from(s.trim()))
                                .filter(|p| !p.as_os_str().is_empty())
                                .collect(),
                        );
                    }
                }
            }
        }
    }

    /// `chezmoi apply <target>` — регенерирует живой файл из шаблона.
    fn apply_chezmoi_apply(&self) -> Result<()> {
        let target = live_target(&self.live_path).ok_or_else(|| {
            anyhow::anyhow!(
                "cannot compute chezmoi target from {}",
                self.live_path.display()
            )
        })?;
        let status = std::process::Command::new("chezmoi")
            .args(["apply", &target])
            .status()
            .map_err(|e| anyhow::anyhow!("chezmoi apply failed to start: {e}"))?;
        if !status.success() {
            anyhow::bail!("chezmoi apply {target} failed (exit {status})");
        }
        Ok(())
    }

    /// Есть ли у нас управляемый шаблон для прямой (не-template) записи.
    pub fn template_available(&self) -> bool {
        self.chezmoi_source.as_ref().map(|p| p.exists()).unwrap_or(false)
    }
}

/// Применить патч к TOML-документу, вернуть diff-строки (точные по ключам).
fn apply_doc_patch(doc: &mut DocumentMut, patch: &ConfigPatch) -> Result<Vec<String>> {
    let mut diff = Vec::new();
    match patch {
        ConfigPatch::UpsertProject { name, proj } => {
            let projects_item = doc
                .entry("projects")
                .or_insert(Item::Table(Table::new()));
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

fn project_table(proj: &ProjectConfig) -> Table {
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

fn remote_table(r: &RemoteMachine) -> Table {
    let mut t = Table::new();
    t.insert("host", Item::Value(Value::from(r.host.as_str())));
    t.insert("port", Item::Value(Value::from(i64::from(r.port))));
    t.insert("user", Item::Value(Value::from(r.user.as_str())));
    t
}

/// Строки "~ key: old → new" для изменившихся полей существующего проекта.
fn changed_keys(name: &str, old: &Table, proj: &ProjectConfig) -> Vec<String> {
    let mut out = Vec::new();
    let field = |t: &Table, k: &str| -> Option<String> {
        t.get(k).and_then(|i| i.as_value()).map(|v| v.to_string())
    };
    let new_path = Some(proj.path.display().to_string());
    if field(old, "path") != new_path {
        out.push(format!(
            "~ projects.{name}.path: {} → {}",
            field(old, "path").unwrap_or_else(|| "(absent)".into()),
            new_path.unwrap_or_default()
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
fn patch_template_text(content: &str, patch: &ConfigPatch) -> Result<(String, Vec<String>)> {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut diff = Vec::new();
    match patch {
        ConfigPatch::UpsertProject { name, proj } => {
            let block = format!("[projects.{name}]\n{}", project_table(proj));
            let added = upsert_table(&mut lines, &format!("[projects.{name}]"), &block, "projects.");
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
                set_key_in_block(&mut lines, "[state]", "tmux", Some(v.to_string()), &mut diff);
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
                set_key_in_block(
                    &mut lines,
                    "[state.opencode]",
                    "projects",
                    value,
                    &mut diff,
                );
            }
        }
    }
    Ok((lines.join("\n"), diff))
}

fn is_header(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with('[') && t.ends_with(']')
}

fn is_template_marker(l: &str) -> bool {
    l.trim_start().starts_with("{{")
}

/// Находит индекс конца блока таблицы: следующая строка-заголовок `[`,
/// маркер `{{` или конец файла.
fn block_end(lines: &[String], from: usize) -> usize {
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
fn upsert_table(lines: &mut Vec<String>, header: &str, block: &str, prefix: &str) -> bool {
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
fn remove_table(lines: &mut Vec<String>, prefix: &str, name: &str) -> bool {
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
fn set_key_in_block(
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
            if rest.starts_with('=') || (rest.starts_with(' ') && rest.trim_start().starts_with('=')) {
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

/// Живёт ли файл под управлением chezmoi.
fn is_chezmoi_managed(path: &Path) -> bool {
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
fn chezmoi_source_path(path: &Path) -> Option<PathBuf> {
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
fn live_target(live: &Path) -> Option<String> {
    let home = dirs::home_dir()?;
    live.strip_prefix(&home)
        .ok()
        .map(|r| r.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(tag: &str, content: &str) -> (PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("dsync-cfg2-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, content).unwrap();
        (path, dir)
    }

    fn base_editor(path: PathBuf) -> ConfigEditor {
        let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        ConfigEditor {
            live_path: path.clone(),
            chezmoi_managed: false,
            chezmoi_source: None,
            cfg,
        }
    }

    const WITH_COMMENTS: &str = r#"# main comment
config_version = 1

[machine]
name = "testbox"

# hub stuff
[hub_connect]
address = "127.0.0.1:42069"

[unknown_section]
keep = "me"

[state]
tmux = true
tmux_restore = false
"#;

    #[test]
    fn add_project_preserves_comments_and_unknown_sections() {
        let (path, dir) = tmp_file("preserve", WITH_COMMENTS);
        let content = std::fs::read_to_string(&path).unwrap();
        let mut ed = base_editor(path.clone());
        let diff = ed
            .add_project("proj-a", "/tmp/proj-a", Some("main"), &["desktop".to_string()], Some("echo hi"), false)
            .unwrap();
        assert!(diff.iter().any(|l| l.contains("+ projects.proj-a added")));

        let out = std::fs::read_to_string(&path).unwrap();
        // Комментарии и неизвестная секция на месте.
        assert!(out.contains("# main comment"));
        assert!(out.contains("# hub stuff"));
        assert!(out.contains("[unknown_section]"));
        assert!(out.contains("keep = \"me\""));
        assert!(out.contains("[state]"));
        assert!(out.contains("[projects.proj-a]"));
        assert!(out.contains("path = \"/tmp/proj-a\""));

        // Весь документ остаётся валидным TOML.
        let back: Config = toml::from_str(&out).unwrap();
        assert!(back.projects.as_ref().unwrap().contains_key("proj-a"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = content;
    }

    #[test]
    fn add_remove_roundtrip_keeps_unknown_content() {
        let (path, dir) = tmp_file("roundtrip", WITH_COMMENTS);
        let mut ed = base_editor(path.clone());
        ed.add_project("p1", "/tmp/p1", None, &[], None, false).unwrap();
        ed.add_remote("m1", "100.89.0.1", 22, "user", false).unwrap();
        ed.remove_project("p1", false).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[unknown_section]"));
        assert!(!out.contains("[projects.p1]"));
        assert!(out.contains("[remote.m1]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_field_survives_remove_remote() {
        let (path, dir) = tmp_file("unknown-sect", WITH_COMMENTS);
        let cfg: Config = toml::from_str(
            "[machine]\nname = \"x\"\n[remote.r1]\nhost = \"h\"\nport = 22\nuser = \"u\"\n[v2_new_section]\nx = 1\n",
        )
        .unwrap();
        let mut ed = ConfigEditor {
            live_path: path.clone(),
            chezmoi_managed: false,
            chezmoi_source: None,
            cfg,
        };
        ed.remove_remote("r1", false).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[v2_new_section]"), "unknown section survives: {out}");
        assert!(!out.contains("[remote.r1]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_keys_diff_reports_exact_keys() {
        let (path, dir) = tmp_file("diff", WITH_COMMENTS);
        let mut ed = base_editor(path.clone());
        ed.add_project("p1", "/tmp/p1", Some("main"), &["a".into()], None, false)
            .unwrap();
        let diff = ed
            .add_project("p1", "/tmp/p2", Some("dev"), &["a".into(), "b".into()], Some("x"), false)
            .unwrap();
        assert!(diff.iter().any(|l| l.contains("projects.p1.path")));
        assert!(diff.iter().any(|l| l.contains("projects.p1.branch")));
        assert!(diff.iter().any(|l| l.contains("projects.p1.machines")));
        assert!(diff.iter().any(|l| l.contains("projects.p1.post_pull")));
        assert!(!diff.iter().any(|l| l.contains("projects.other")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_patch_toggles_and_keeps_comments() {
        let (path, dir) = tmp_file("state", WITH_COMMENTS);
        let mut ed = base_editor(path.clone());
        let diff = ed
            .apply_state(Some(false), Some(true), Some(vec![], ), false)
            .unwrap();
        assert!(diff.iter().any(|l| l.contains("state.tmux")));
        assert!(diff.iter().any(|l| l.contains("state.tmux_restore")));
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# main comment"));
        assert!(out.contains("tmux = false"));
        assert!(out.contains("tmux_restore = true"));
        let back: Config = toml::from_str(&out).unwrap();
        let st = back.state.unwrap();
        assert!(!st.tmux);
        assert!(st.tmux_restore);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_opencode_list_sets_and_clears() {
        let (path, dir) = tmp_file("oc", WITH_COMMENTS);
        let mut ed = base_editor(path.clone());
        ed.apply_state(
            None,
            None,
            Some(vec!["~/projects/a".into(), "~/projects/b".into()]),
            false,
        )
        .unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("projects = [\"~/projects/a\", \"~/projects/b\"]"));
        // Очистка → ключ уходит (означает «все проекты»).
        ed.apply_state(None, None, Some(vec![]), false).unwrap();
        let out2 = std::fs::read_to_string(&path).unwrap();
        assert!(!out2.contains("projects = ["));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- текстовый движок (Go-template шаблоны) ---

    const GO_TMPL: &str = r#"# dsync
[machine]
name = "{{ if eq .chezmoi.hostname \"a\" }}desktop{{ else }}unknown{{ end }}"

[projects.dsync]
path = "/home/mflkee/projects/dsync"
branch = "main"
machines = ["notebook", "desktop"]

[state]
tmux = true
tmux_restore = false

{{ if eq .chezmoi.hostname "server" }}
[hub]
bind = "0.0.0.0:42069"
{{ end }}
"#;

    #[test]
    fn go_template_project_add_and_remove_preserves_blocks() {
        let (out, diff) = patch_template_text(GO_TMPL, &ConfigPatch::UpsertProject {
            name: "newp".into(),
            proj: ProjectConfig {
                path: "/tmp/newp".into(),
                branch: Some("main".into()),
                machines: Some(vec!["notebook".into()]),
                post_pull: None,
            },
        })
        .unwrap();
        assert!(diff.iter().any(|l| l.contains("+ projects.newp added")));
        assert!(out.contains("[projects.newp]"));
        assert!(out.contains("path = \"/tmp/newp\""));
        // Go-блоки не тронуты
        assert!(out.contains("{{ if eq .chezmoi.hostname \"server\" }}"));
        assert!(out.contains("{{ end }}"));
        // Существующие проекты на месте
        assert!(out.contains("[projects.dsync]"));

        let (out2, diff2) = patch_template_text(&out, &ConfigPatch::RemoveProject {
            name: "newp".into(),
        })
        .unwrap();
        assert!(diff2.iter().any(|l| l.contains("- projects.newp removed")));
        assert!(!out2.contains("[projects.newp]"));
        assert!(out2.contains("[projects.dsync]"));
        assert!(out2.contains("{{ end }}"));
    }

    #[test]
    fn go_template_state_toggle() {
        let (out, diff) = patch_template_text(
            GO_TMPL,
            &ConfigPatch::State {
                tmux: Some(false),
                tmux_restore: None,
                opencode_projects: None,
            },
        )
        .unwrap();
        assert!(diff.iter().any(|l| l.contains("tmux")));
        assert!(out.contains("tmux = false"));
        assert!(out.contains("tmux_restore = false"));
        assert!(out.contains("{{ if eq .chezmoi.hostname \"server\" }}"));
    }

    #[test]
    fn validation_errors_still_apply() {
        let (path, dir) = tmp_file("valid", "[machine]\nname = \"x\"\n");
        let mut ed = base_editor(path.clone());
        assert!(ed.add_project("", "/tmp/x", None, &[], None, false).is_err());
        assert!(ed.add_remote("m2", "1.2.3.4", 0, "u", false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_target_strips_home() {
        let home = dirs::home_dir().unwrap();
        let live = home.join(".config/dsync/dsync/config.toml");
        let t = live_target(&live).unwrap();
        assert_eq!(t, ".config/dsync/dsync/config.toml");
    }
}