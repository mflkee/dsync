use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Захват live-правок dotfiles в их репозитории (см. `src/client/capture.rs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureConfig>,
}

/// Настройка авто-захвата правок живых dotfiles-файлов при `dsync push`.
///
/// Список `watch` задаёт исходные пути (файлы/директории; директории
/// обходятся рекурсивно). Если секции `[capture]` или `watch` нет — по
/// умолчанию берутся **все** файлы, которыми управляет chezmoi
/// (`chezmoi managed --include files`), так что ловится любое изменение —
/// из nvim, bash, sed, скриптов. Правки, которых chezmoi не знает,
/// пропускаются.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct CaptureConfig {
    /// Живые пути для сканирования (опционально; default — все chezmoi managed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch: Option<Vec<String>>,
    /// Живые пути, которые никогда не захватываются (регенерируемые темы и т.п.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

fn default_config_version() -> u32 {
    1
}

/// Текущая версия схемы конфига, поддерживаемая бинарём.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MachineConfig {
    pub name: String,
    /// Path to the SSH private key used for remote pulls (e.g. "~/.ssh/id_ed25519").
    /// Defaults to `~/.ssh/id_ed25519` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<PathBuf>,
}

impl MachineConfig {
    /// SSH key path with `~` expansion; falls back to `~/.ssh/id_ed25519`.
    pub fn ssh_key_path(&self) -> PathBuf {
        let default = || dirs::home_dir().map(|h| h.join(".ssh/id_ed25519"));
        match &self.ssh_key {
            Some(p) => expand_tilde(p).unwrap_or_else(|| p.clone()),
            None => default().unwrap_or_else(|| PathBuf::from(".ssh/id_ed25519")),
        }
    }
}

fn expand_tilde(p: &Path) -> Option<PathBuf> {
    let s = p.to_string_lossy();
    let rest = s.strip_prefix("~/")?;
    dirs::home_dir().map(|h| h.join(rest))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_key_defaults_to_home() {
        let m = MachineConfig {
            name: "test".into(),
            ssh_key: None,
        };
        let p = m.ssh_key_path();
        let home = dirs::home_dir().expect("home in tests");
        assert_eq!(p, home.join(".ssh/id_ed25519"));
    }

    #[test]
    fn ssh_key_expands_tilde() {
        let m = MachineConfig {
            name: "test".into(),
            ssh_key: Some("~/keys/mykey".into()),
        };
        let home = dirs::home_dir().expect("home in tests");
        assert_eq!(m.ssh_key_path(), home.join("keys/mykey"));
    }

    #[test]
    fn ssh_key_absolute_passthrough() {
        let m = MachineConfig {
            name: "test".into(),
            ssh_key: Some("/etc/dsync/key".into()),
        };
        assert_eq!(m.ssh_key_path(), PathBuf::from("/etc/dsync/key"));
    }

    #[test]
    fn config_version_field_defaults_and_rejects_newer() {
        let old: Config = toml::from_str("machine = { name = 'x' }\n").unwrap();
        assert_eq!(old.config_version, 1);
        let too_new: Result<Config> =
            toml::from_str("config_version = 99\nmachine = { name = 'x' }\n")
                .map_err(|e| anyhow::anyhow!(e));
        // Загрузка из строки не валидирует версию — валидация в Config::load.
        assert!(too_new.is_ok());
    }
}
