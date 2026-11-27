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
use toml_edit::DocumentMut;

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
            chezmoi_template: self
                .chezmoi_source
                .as_ref()
                .map(|p| p.display().to_string()),
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
            let content = std::fs::read_to_string(&self.live_path)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", self.live_path.display()))?;
            let mut doc = content.parse::<DocumentMut>().map_err(|e| {
                anyhow::anyhow!("config parse error ({}): {e}", self.live_path.display())
            })?;
            let diff = apply_doc_patch(&mut doc, patch)?;
            std::fs::write(&self.live_path, doc.to_string())
                .map_err(|e| anyhow::anyhow!("write {}: {e}", self.live_path.display()))?;
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
                        state.opencode.get_or_insert_with(Default::default).projects = Some(
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
}

/// Применить патч к TOML-документу, вернуть diff-строки (точные по ключам).
mod chezmoi;
mod patch;

#[cfg(test)]
mod tests;

pub(crate) use chezmoi::*;
pub(crate) use patch::*;
