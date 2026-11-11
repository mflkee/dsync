use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::PushRequest;

use super::connect::{close_conn, connect_with_retry, send_push};

pub async fn push(cfg: Config, machine: Option<String>) -> Result<Vec<String>> {
    // Захват live-правок dotfiles в репы до коммита: приходит сюда любое
    // изменение живых файлов (~/.zshrc, ~/.config/...), откуда бы оно ни было
    // сделано (nvim, bash, sed, скрипты) — см. capture::capture_changed.
    let captured = super::capture::capture_changed(&cfg);

    let mut out = push_core(cfg, machine).await?;
    out.splice(0..0, captured);
    Ok(out)
}

/// Ядро push без периодического захвата: коммит репозиториев, пуш в origin,
/// уведомление хаба. Используется явным `dsync capture`, который уже сам
/// re-add-нул нужные файлы и не должен гонять повторный скан.
pub(super) async fn push_core(cfg: Config, machine: Option<String>) -> Result<Vec<String>> {
    info!("starting push from {}", cfg.machine.name);
    if let Some(m) = &machine {
        info!("targeting SSH pull to machine {m}");
    }

    // Сначала локальная работа с репозиториями, потом — короткая сессия hub.
    // Иначе, пока мы 30-60с ходим по git (медленный origin), QUIC-соединение
    // умирает по idle-таймауту и `send_push` падает с «timed out». Плюс
    // локальные коммиты сохраняются, даже если hub сейчас недоступен.
    let projects = collect_projects(&cfg).await?;

    let conn = connect_with_retry(&cfg).await?;

    let req = build_push_request(&cfg, machine, projects)?;

    let resp = send_push(&conn, &req).await?;
    close_conn(&conn);
    let mut out = Vec::new();
    if resp.ok {
        info!("push successful");
        out.push("✓ pushed to hub".to_string());
    } else {
        anyhow::bail!("push failed: {}", resp.error.unwrap_or_default());
    }

    // Не-git состояние флота (tmux-снапшот, сессии opencode) — best-effort:
    // ошибки логируются и не валят общий push. Сбор делается здесь (capture=true),
    // pull в том же цикле `dsync-run` только применяет чужие изменения.
    out.extend(crate::client::state::sync_state(&cfg, true).await);

    Ok(out)
}

/// Собирает `PushRequest` от имени конфига: несёт machine name и токен из
/// `[hub_connect] token` (пустой токен хаб отвергнет — см. hub-auth).
fn build_push_request(
    cfg: &Config,
    machine: Option<String>,
    projects: Vec<crate::protocol::ProjectState>,
) -> Result<PushRequest> {
    Ok(PushRequest {
        machine: cfg.machine.name.clone(),
        token: super::connect::hub_token(cfg),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64,
        projects,
        target: machine,
        version: Some(crate::protocol::version_string()),
    })
}

async fn collect_projects(cfg: &Config) -> Result<Vec<crate::protocol::ProjectState>> {
    let mut static_names = std::collections::HashSet::new();
    let mut states = Vec::new();

    if let Some(projects) = &cfg.projects {
        for (name, config) in projects {
            static_names.insert(name.clone());
            let path = crate::projects::status::expand_user_path(&config.path);
            if let Err(e) = crate::projects::sync::commit_and_push(name, &path) {
                tracing::warn!("{e}");
            }
        }
        states = crate::projects::status::scan(projects)?;
    }

    // Авто-обнаружение новых git-проектов в общем каталоге ([auto_projects]).
    // Анонсируем (хаб развернёт на флоте), но не коммитим за репозиторий —
    // авто-уход («project sync: …») остаётся за явными [projects.*].
    if let Some(ap) = &cfg.auto_projects {
        let root = crate::projects::status::expand_user_path(
            ap.root
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("~/projects")),
        );
        if root.is_dir() {
            let excluded: std::collections::HashSet<String> =
                ap.exclude.iter().flatten().cloned().collect();
            match crate::projects::status::discover_entries(&root, &static_names, &excluded) {
                Ok(entries) => {
                    let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
                    log_new_auto_projects(&names);
                    // Скан в несколько потоков: 29 репо × ~5 git-вызовов
                    // последовательно — это 8–11 с на каждый push.
                    states.extend(crate::projects::status::scan_parallel(entries));
                }
                Err(e) => tracing::warn!("auto_projects scan of {} failed: {e:#}", root.display()),
            }
        }
    }

    Ok(states)
}

/// Путь к списку уже анонсированных авто-проектов (только для честного лога).
fn auto_projects_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("DSYNC_AUTO_PROJECTS_PATH") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    dirs::data_dir()
        .unwrap_or_default()
        .join("dsync/auto-projects.json")
}

/// Логирует только реально новые проекты. Раньше в лог каждый цикл шло
/// `auto-discovered 29 new project(s)` — это было ложью (29 = все найденные)
/// и мешало заметить по-настоящему новое.
fn log_new_auto_projects(names: &[String]) {
    let path = auto_projects_path();
    let mut known: std::collections::BTreeSet<String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let fresh: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| !known.contains(*n))
        .collect();
    if fresh.is_empty() {
        return;
    }
    info!(
        "auto-discovered {} new project(s): {}",
        fresh.len(),
        fresh.join(", ")
    );
    known.extend(names.iter().cloned());
    // Забываем исчезнувшие каталоги, чтобы файл не рос бесконечно.
    known.retain(|n| names.iter().any(|x| x == n));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(
        &path,
        serde_json::to_string_pretty(&known).unwrap_or_default(),
    ) {
        tracing::warn!("can't save auto-projects list: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HubConnectConfig, MachineConfig};

    /// env-переопределение sidecar-пути — общий мьютекс, чтобы тесты не
    /// гонялись за переменную окружения параллельно.
    static TOKENS_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn cfg_with_token(token: &str) -> Config {
        Config {
            config_version: 1,
            machine: MachineConfig {
                name: "desktop".into(),
                ssh_key: None,
            },
            hub: None,
            hub_connect: Some(HubConnectConfig {
                address: "10.0.0.1:42069".into(),
                token: token.into(),
            }),
            projects: None,
            auto_projects: None,
            remote: None,
            capture: None,
            state: None,
        }
    }

    #[test]
    fn built_request_carries_configured_token_on_the_wire() {
        let req = build_push_request(&cfg_with_token("t0k3n"), None, Vec::new()).unwrap();
        let val = serde_json::to_value(&req).unwrap();
        assert_eq!(val["machine"], "desktop");
        assert_eq!(val["token"], "t0k3n");
    }

    #[test]
    fn missing_token_serializes_as_empty_string() {
        // Пин sidecar на несуществующий путь: реальный tokens.toml тестовой
        // машины не должен влиять на тест.
        let _g = TOKENS_ENV.lock().unwrap();
        std::env::set_var("DSYNC_TOKENS_PATH", "/nonexistent/dsync-test-tokens.toml");
        let req = build_push_request(&cfg_with_token(""), None, Vec::new()).unwrap();
        let val = serde_json::to_value(&req).unwrap();
        assert_eq!(val["token"], "");
        std::env::remove_var("DSYNC_TOKENS_PATH");
    }

    #[test]
    fn empty_config_token_falls_back_to_sidecar() {
        // Конфиг без токена, но с sidecar-файлом: на проводе — токен sidecar.
        let _g = TOKENS_ENV.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("dsync-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tokens.toml");
        std::fs::write(&path, "[hub_connect]\ntoken = \"sidecar-tok\"\n").unwrap();
        std::env::set_var("DSYNC_TOKENS_PATH", &path);
        let req = build_push_request(&cfg_with_token(""), None, Vec::new()).unwrap();
        let val = serde_json::to_value(&req).unwrap();
        assert_eq!(val["token"], "sidecar-tok");
        std::env::remove_var("DSYNC_TOKENS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
