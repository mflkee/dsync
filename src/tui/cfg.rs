//! Редактирование конфига dsync из TUI: добавление/удаление проектов и
//! машин ([remote]), сохранение в TOML. Живой файл может быть под
//! chezmoi — тогда в UI выводится предупреждение.

use std::path::{Path, PathBuf};

use anyhow::Result;

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

/// Редактор конфига: живёт в backend-потоке, мутирует `cfg` и пишет файл.
#[derive(Debug)]
pub struct ConfigEditor {
    pub live_path: PathBuf,
    pub chezmoi_managed: bool,
    pub cfg: Config,
}

impl ConfigEditor {
    pub fn load() -> Result<Self> {
        let live_path = config::find_config_path()
            .ok_or_else(|| anyhow::anyhow!("config not found"))?;
        let chezmoi_managed = is_chezmoi_managed(&live_path);
        let cfg = Config::load()?;
        Ok(Self {
            live_path,
            chezmoi_managed,
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

        CfgSummary {
            machine: self.cfg.machine.name.clone(),
            hub_connect: self.cfg.hub_connect.as_ref().map(|h| h.address.clone()),
            has_hub_section: self.cfg.hub.is_some(),
            projects,
            remotes,
            config_path: self.live_path.display().to_string(),
            chezmoi_managed: self.chezmoi_managed,
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
    ) -> Result<()> {
        if name.trim().is_empty() || path.trim().is_empty() {
            anyhow::bail!("name and path are required");
        }
        let p = ProjectConfig {
            path: PathBuf::from(path.trim()),
            branch: branch.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            machines: Some(machines.to_vec()).filter(|v| !v.is_empty()),
            post_pull: post_pull
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        };
        let map = self.cfg.projects.get_or_insert_with(Default::default);
        map.insert(name.trim().to_string(), p);
        self.save()
    }

    pub fn remove_project(&mut self, name: &str) -> Result<()> {
        let Some(map) = self.cfg.projects.as_mut() else {
            return Ok(());
        };
        if map.remove(name).is_none() {
            anyhow::bail!("project {name:?} not in config");
        }
        if map.is_empty() {
            self.cfg.projects = None;
        }
        self.save()
    }

    pub fn add_remote(&mut self, name: &str, host: &str, port: u16, user: &str) -> Result<()> {
        if name.trim().is_empty() || host.trim().is_empty() || user.trim().is_empty() {
            anyhow::bail!("name, host and user are required");
        }
        if port == 0 {
            anyhow::bail!("port must be 1..65535");
        }
        let r = RemoteMachine {
            host: host.trim().to_string(),
            port,
            user: user.trim().to_string(),
        };
        let map = self.cfg.remote.get_or_insert_with(Default::default);
        map.insert(name.trim().to_string(), r);
        self.save()
    }

    pub fn remove_remote(&mut self, name: &str) -> Result<()> {
        let Some(map) = self.cfg.remote.as_mut() else {
            return Ok(());
        };
        if map.remove(name).is_none() {
            anyhow::bail!("remote machine {name:?} not in config");
        }
        if map.is_empty() {
            self.cfg.remote = None;
        }
        self.save()
    }

    /// Перезаписать конфиг из-под себя (свежий TOML без комментариев).
    pub fn save(&self) -> Result<()> {
        let text = toml::to_string_pretty(&self.cfg)?;
        std::fs::write(&self.live_path, text)?;
        Ok(())
    }
}

/// Живёт ли файл под управлением chezmoi (проверка через `chezmoi source-path`).
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