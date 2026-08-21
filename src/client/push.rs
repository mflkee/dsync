use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::PushRequest;

use super::connect::{connect_with_retry, send_push};

pub async fn push(cfg: Config, machine: Option<String>) -> Result<()> {
    info!("starting push from {}", cfg.machine.name);
    if let Some(m) = &machine {
        info!("targeting SSH pull to machine {m}");
    }

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
    if resp.ok {
        info!("push successful");
        println!("✓ pushed to hub");
    } else {
        anyhow::bail!("push failed: {}", resp.error.unwrap_or_default());
    }

    Ok(())
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
