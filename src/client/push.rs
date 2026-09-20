use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::PushRequest;

use super::connect::{close_conn, connect_with_retry, send_push};

pub async fn push(cfg: Config, machine: Option<String>) -> Result<Vec<String>> {
    info!("starting push from {}", cfg.machine.name);
    if let Some(m) = &machine {
        info!("targeting SSH pull to machine {m}");
    }

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
    })
}

async fn collect_projects(cfg: &Config) -> Result<Vec<crate::protocol::ProjectState>> {
    if let Some(projects) = &cfg.projects {
        for (name, config) in projects {
            let path = crate::projects::status::expand_user_path(&config.path);
            if let Err(e) = crate::projects::sync::commit_and_push(name, &path) {
                tracing::warn!("{e}");
            }
        }
        crate::projects::status::scan(projects)
    } else {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HubConnectConfig, MachineConfig};

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
            remote: None,
            capture: None,
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
        let req = build_push_request(&cfg_with_token(""), None, Vec::new()).unwrap();
        let val = serde_json::to_value(&req).unwrap();
        assert_eq!(val["token"], "");
    }
}
