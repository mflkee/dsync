use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    /// Версия схемы конфига. Сейчас поддерживается только 1; при появлении
    /// v2, Настраиваемый мигратор будет в `/migrate` или в `init`.
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    pub machine: MachineConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub: Option<HubConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_connect: Option<HubConnectConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<HashMap<String, ProjectConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<HashMap<String, RemoteMachine>>,
}

fn default_config_version() -> u32 {
    1
}

/// Текущая версия схемы конфига, поддерживаемая бинарём.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MachineConfig {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HubConfig {
    pub bind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HubConnectConfig {
    pub address: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProjectConfig {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machines: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_pull: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RemoteMachine {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = find_config_path()
            .ok_or_else(|| anyhow::anyhow!("no config found (expected /etc/dsync/config.toml, ~/.config/dsync/dsync/config.toml or ./dsync.toml)"))?;
        let content = std::fs::read_to_string(&path)?;
        let cfg: Config = toml::from_str(&content)?;
        if cfg.config_version > CONFIG_VERSION {
            anyhow::bail!(
                "config version {} is newer than supported {CONFIG_VERSION}; update dsync",
                cfg.config_version
            );
        }
        Ok(cfg)
    }
}

/// Путь к конфигу в том же порядке, что и `Config::load`.
pub fn find_config_path() -> Option<PathBuf> {
    config_paths().into_iter().find(|p| p.exists())
}

pub fn config_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/etc/dsync/config.toml"),
        directories::ProjectDirs::from("com", "mflkee", "dsync")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("~/.config/dsync"))
            .join("dsync/config.toml"),
        PathBuf::from("dsync.toml"),
    ]
}
