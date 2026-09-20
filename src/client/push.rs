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

    let conn = connect_with_retry(&cfg).await?;

    let projects = collect_projects(&cfg).await?;

    let req = PushRequest {
        machine: cfg.machine.name.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64,
        projects,
        target: machine,
    };

    let resp = send_push(&conn, &req).await?;
    close_conn(&conn);
    let mut out = Vec::new();
    out.extend(captured);
    if resp.ok {
        info!("push successful");
        out.push("✓ pushed to hub".to_string());
    } else {
        anyhow::bail!("push failed: {}", resp.error.unwrap_or_default());
    }

    Ok(out)
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
