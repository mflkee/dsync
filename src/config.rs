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
    /// Авто-обнаружение новых git-проектов в общем каталоге (обычно
    /// `~/projects`): любая новая папка с `.git` попадает в флот без ручной
    /// правки конфига. См. `AutoProjectsConfig`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_projects: Option<AutoProjectsConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<HashMap<String, RemoteMachine>>,
    /// Захват live-правок dotfiles в их репозитории (см. `src/client/capture.rs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureConfig>,
    /// Синхронизация не-git состояния флота: tmux-раскладка и сессии opencode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<StateConfig>,
}

/// Синхронизация не-git состояния флота через хаб (см. `src/client/state.rs`).
///
/// ```toml
/// [state]
/// tmux = true            # синхронизировать tmux-resurrect снапшот
/// tmux_restore = false   # после применения — запускать resurrect restore
///
/// [state.opencode]
/// # каталоги проектов, чьи сессии синхронизируем (по умолчанию — все [projects.*])
/// projects = ["~/projects/mushroomwars"]
/// ```
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct StateConfig {
    /// Синхронизировать снапшот tmux-resurrect.
    #[serde(default)]
    pub tmux: bool,
    /// После применения чужого снапшота запускать `tmux-resurrect` restore
    /// (в уже запущенном tmux). По умолчанию выключено: восстановление
    /// в живую сессию может насаждать окна, если делать это слишком часто.
    #[serde(default)]
    pub tmux_restore: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode: Option<OpencodeStateConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct OpencodeStateConfig {
    /// Каталоги проектов, сессии которых синхронизируем. Если не задано —
    /// берутся пути всех `[projects.*]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<PathBuf>>,
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
    /// Per-machine auth tokens: `{machine name → token}`. The hub refuses to
    /// start without at least one entry (fail-secure, see Proposal).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tokens: HashMap<String, String>,
    /// Maximum serialized request payload accepted (bytes).
    #[serde(default = "default_max_message_size")]
    pub max_message_size: u64,
    /// Maximum concurrently processed requests.
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: u32,
    /// Machines not seen for this many days are pruned from hub state.
    #[serde(default = "default_retention_days")]
    pub retention_days: u64,
    /// Additional SSH-pull attempts after the first failure (bounded retry).
    #[serde(default = "default_pull_retries")]
    pub pull_retries: u32,
    /// SSH exec timeout for pulls (seconds). The pull runs `git stash && git
    /// pull && post_pull`, and post_pull can be a long build (`cargo build`),
    /// so the exec window must be generous — connect stage keeps the short
    /// DEFAULT_SSH_TIMEOUT (30s), only the exec stage uses this.
    #[serde(default = "default_pull_timeout_secs")]
    pub pull_timeout_secs: u64,
}

pub(crate) fn default_max_message_size() -> u64 {
    8 * 1024 * 1024
}

pub(crate) fn default_max_concurrency() -> u32 {
    32
}

pub(crate) fn default_retention_days() -> u64 {
    30
}

pub(crate) fn default_pull_retries() -> u32 {
    2
}

pub(crate) fn default_pull_timeout_secs() -> u64 {
    300
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HubConnectConfig {
    pub address: String,
    /// Client token for hub auth (see `[hub] tokens` on the hub side).
    #[serde(default)]
    pub token: String,
}

/// Секретные токены в отдельном sidecar-файле `~/.config/dsync/dsync/tokens.toml`
/// (0600, игнорируется chezmoi-apply и dsync-capture). В главном `config.toml`
/// их держать нельзя: файл управляется chezmoi и перегенерируется при каждом
/// `chezmoi apply` (post_pull dotfiles), стирая ручные правки. Формат:
///
/// ```toml
/// # у каждого клиента
/// [hub_connect]
/// token = "…"
///
/// # только у хаба
/// [hub]
/// tokens = { "desktop" = "…", "notebook" = "…" }
/// ```
#[derive(Debug, Default, Deserialize, Clone)]
pub struct SecretTokens {
    #[serde(default)]
    pub hub_connect: Option<SecretHubConnect>,
    #[serde(default)]
    pub hub: Option<SecretHub>,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct SecretHubConnect {
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct SecretHub {
    #[serde(default)]
    pub tokens: HashMap<String, String>,
}

/// Путь к sidecar-файлу токенов — рядом с главным конфигом.
/// Возможно переопределение через `DSYNC_TOKENS_PATH` (используется тестами
/// и позволяет разместить секреты вне конфиг-каталога).
pub fn tokens_path() -> PathBuf {
    if let Ok(p) = std::env::var("DSYNC_TOKENS_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    directories::ProjectDirs::from("com", "mflkee", "dsync")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("~/.config/dsync"))
        .join("dsync/tokens.toml")
}

/// Читает токены из sidecar-файла. Отсутствие файла или битый TOML — не
/// ошибка (просто пустой результат): пустоту обрабатывает вызывающий код
/// (fail-secure хаба / пустой токен клиента).
pub fn read_secret_tokens() -> SecretTokens {
    read_secret_tokens_from(&tokens_path())
}

pub fn read_secret_tokens_from(path: &std::path::Path) -> SecretTokens {
    let Ok(content) = std::fs::read_to_string(path) else {
        return SecretTokens::default();
    };
    match toml::from_str(&content) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("ignoring malformed tokens file {}: {e:#}", path.display());
            SecretTokens::default()
        }
    }
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

/// Авто-обнаружение новых git-проектов флота.
///
/// Клиент при каждом `dsync push` обходит `root` на глубину 1 и включает в
/// push-сообщение все каталоги с `.git`, которых нет в явном `[projects.*]`
/// (имя проекта — имя каталога). Хаб, получив такой проект, разворачивает его
/// на машинах из `machines`: клонирует из origin (`url` приходит в состоянии
/// проекта), если каталога на машине ещё нет, а дальше — обычный
/// `git pull --rebase --autostash`.
///
/// Семантика: авто-проекты **анонсируются** (разворачиваются на флоте), но
/// dsync не делает за них `git add/commit/push` — репозитории, за которыми
/// dsync «ухаживает» (автокоммит «project sync: …»), остаются в `[projects.*]`.
///
/// ```toml
/// [auto_projects]
/// root = "~/projects"
/// branch = "main"
/// machines = ["notebook", "desktop", "archlinux-mkair", "archlinux-server"]
/// exclude = ["rustlings", "git-tutorial"]   # имена, НЕ разворачиваемые на флоте
/// ```
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct AutoProjectsConfig {
    /// Корень сканирования (по умолчанию `~/projects`), глубина 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// Ветка по умолчанию для новых проектов без неё (по умолчанию `main`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Машины флота, на которые разворачиваются новые проекты. Если не задано —
    /// берутся все машины из `[remote.*]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machines: Option<Vec<String>>,
    /// Имена проектов (каталогов), которые авто-анонс НЕ трогает: учёба,
    /// скретч, чужые форки, дубли. Не влияет на явные `[projects.*]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
    /// Контроль-режим: `sync = false` → авто-проекты только **анонсируются**
    /// (состояния видны в `dsync status` / TUI Projects), но НЕ разворачиваются
    /// на флоте: hub не делает git clone/pull. По умолчанию `true` — обычный
    /// разворот (`git pull --rebase --autostash` / bootstrap clone).
    /// Явные `[projects.*]` пулятся в любом случае.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<bool>,
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

    /// Директория данных хаба: `[hub] data_dir` или дефолт `~/.local/share/dsync`.
    /// Здесь живут `machines.json`, TLS-сертификат и `ssh_known_hosts.toml`.
    pub fn hub_data_dir(&self) -> PathBuf {
        self.hub
            .as_ref()
            .and_then(|h| h.data_dir.clone())
            .unwrap_or_else(|| dirs::data_dir().unwrap_or_default().join("dsync"))
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

    #[test]
    fn hub_defaults_limits() {
        let cfg: Config = toml::from_str(
            "machine = { name = 'x' }\n\
             [hub]\n\
             bind = '0.0.0.0:42069'\n",
        )
        .unwrap();
        let hub = cfg.hub.expect("hub present");
        assert_eq!(hub.max_message_size, 8 * 1024 * 1024);
        assert_eq!(hub.max_concurrency, 32);
        assert_eq!(hub.retention_days, 30);
        assert_eq!(hub.pull_retries, 2);
        assert!(hub.tokens.is_empty(), "no tokens configured");
    }

    #[test]
    fn hub_fields_roundtrip_and_old_config_parses() {
        let cfg: Config = toml::from_str(
            "machine = { name = 'desktop' }\n\
             [hub]\n\
             bind = '0.0.0.0:42069'\n\
             max_message_size = 4096\n\
             max_concurrency = 4\n\
             retention_days = 7\n\
             pull_retries = 1\n\
             [hub.tokens]\n\
             desktop = 'tok-a'\n\
             notebook = 'tok-b'\n\
             [hub_connect]\n\
             address = '127.0.0.1:42069'\n\
             token = 'tok-a'\n",
        )
        .unwrap();
        let hub = cfg.hub.as_ref().unwrap();
        assert_eq!(hub.tokens["desktop"], "tok-a");
        assert_eq!(hub.tokens["notebook"], "tok-b");
        assert_eq!(hub.max_message_size, 4096);
        assert_eq!(hub.max_concurrency, 4);
        assert_eq!(hub.retention_days, 7);
        assert_eq!(hub.pull_retries, 1);
        assert_eq!(
            cfg.hub_connect.as_ref().unwrap().token,
            "tok-a",
            "client token parsed"
        );

        // Round-trip через toml сохраняет всё.
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.hub.as_ref().unwrap().tokens["notebook"], "tok-b");
        assert_eq!(back.hub_connect.as_ref().unwrap().token, "tok-a");

        // Старый конфиг без новых полей и без [hub] также парсится.
        let old: Config = toml::from_str("machine = { name = 'x' }\n").unwrap();
        assert!(old.hub.is_none());
        assert!(old.hub_connect.is_none());
    }

    fn tmp_tokens_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dsync-secrets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn secret_tokens_missing_file_is_empty() {
        let dir = tmp_tokens_dir("missing");
        let t = read_secret_tokens_from(&dir.join("tokens.toml"));
        assert!(t.hub_connect.is_none());
        assert!(t.hub.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_tokens_parse_hub_and_client() {
        let dir = tmp_tokens_dir("parse");
        std::fs::write(
            dir.join("tokens.toml"),
            "[hub_connect]\ntoken = \"tok-m\"\n\n[hub]\ntokens = { \"desktop\" = \"tok-d\", \"notebook\" = \"tok-n\" }\n",
        )
        .unwrap();
        let t = read_secret_tokens_from(&dir.join("tokens.toml"));
        assert_eq!(t.hub_connect.unwrap().token.unwrap(), "tok-m");
        let hub = t.hub.unwrap();
        assert_eq!(hub.tokens.get("desktop").unwrap(), "tok-d");
        assert_eq!(hub.tokens.get("notebook").unwrap(), "tok-n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_tokens_malformed_is_empty_not_error() {
        let dir = tmp_tokens_dir("malformed");
        std::fs::write(dir.join("tokens.toml"), "not = [valid toml").unwrap();
        let t = read_secret_tokens_from(&dir.join("tokens.toml"));
        assert!(t.hub_connect.is_none());
        assert!(t.hub.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_section_parses() {
        let cfg: Config = toml::from_str(
            "machine = { name = 'x' }\n\
             [state]\n\
             tmux = true\n\
             tmux_restore = true\n\
             [state.opencode]\n\
             projects = ['~/projects/mushroomwars']\n",
        )
        .unwrap();
        let st = cfg.state.expect("state present");
        assert!(st.tmux);
        assert!(st.tmux_restore);
        assert_eq!(
            st.opencode.unwrap().projects.unwrap()[0],
            PathBuf::from("~/projects/mushroomwars")
        );
    }

    #[test]
    fn auto_projects_section_parses_and_defaults() {
        let cfg: Config = toml::from_str(
            "machine = { name = 'x' }\n\
             [auto_projects]\n\
             root = '~/projects'\n\
             branch = 'main'\n\
             machines = ['notebook', 'desktop']\n\
             exclude = ['rustlings', 'study']\n\
             sync = false\n",
        )
        .unwrap();
        let ap = cfg.auto_projects.expect("auto_projects present");
        assert_eq!(ap.root.as_deref().unwrap(), Path::new("~/projects"));
        assert_eq!(ap.branch.as_deref(), Some("main"));
        assert_eq!(ap.machines.as_ref().unwrap().len(), 2);
        assert_eq!(ap.exclude.as_ref().unwrap().len(), 2);
        assert_eq!(ap.sync, Some(false));

        // Минимальная секция: всё по умолчанию (None).
        let cfg: Config = toml::from_str(
            "machine = { name = 'x' }\n[auto_projects]\n",
        )
        .unwrap();
        let ap = cfg.auto_projects.expect("present");
        assert!(ap.root.is_none() && ap.branch.is_none() && ap.machines.is_none() && ap.exclude.is_none());
        assert!(ap.sync.is_none(), "sync default = None (означает true)");

        // Отсутствует секция — None.
        let cfg: Config = toml::from_str("machine = { name = 'x' }\n").unwrap();
        assert!(cfg.auto_projects.is_none());
    }

    #[test]
    fn missing_state_section_is_none() {
        let cfg: Config = toml::from_str("machine = { name = 'x' }\n").unwrap();
        assert!(cfg.state.is_none());
    }
}
