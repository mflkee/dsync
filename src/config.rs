use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub machine: MachineConfig,
    pub hub: Option<HubConfig>,
    pub hub_connect: Option<HubConnectConfig>,
    pub zen: Option<ZenConfig>,
    pub projects: Option<HashMap<String, ProjectConfig>>,
    pub remote: Option<HashMap<String, RemoteMachine>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MachineConfig {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HubConfig {
    pub bind: String,
    pub cert: Option<String>,
    pub key: Option<String>,
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HubConnectConfig {
    pub address: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ZenConfig {
    pub profile_path: PathBuf,
    pub export_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProjectConfig {
    pub path: PathBuf,
    pub remote: Option<String>,
    pub branch: Option<String>,
    pub machines: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RemoteMachine {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let paths = vec![
            PathBuf::from("/etc/dsync/config.toml"),
            directories::ProjectDirs::from("com", "mflkee", "dsync")
                .map(|d| d.config_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("~/.config/dsync"))
                .join("dsync/config.toml"),
            PathBuf::from("dsync.toml"),
        ];

        for path in &paths {
            if path.exists() {
                let content = std::fs::read_to_string(path)?;
                return Ok(toml::from_str(&content)?);
            }
        }

        anyhow::bail!("no config found at {:?}", paths);
    }
}
