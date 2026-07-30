use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::PushRequest;

use super::connect::{connect_with_retry, send_push};

pub async fn push(cfg: Config, _machine: Option<String>) -> Result<()> {
    info!("starting push from {}", cfg.machine.name);

    let conn = connect_with_retry(&cfg).await?;

    info!("collecting projects...");
    let projects = collect_projects(&cfg).await?;
    info!("collecting zen...");
    let zen = collect_zen(&cfg).await?;

    let req = PushRequest {
        machine: cfg.machine.name.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64,
        zen,
        projects,
    };

    info!("sending push request...");
    let resp = send_push(&conn, &req).await?;
    if resp.ok {
        info!("push successful");
        println!("✓ pushed to hub");
    } else {
        anyhow::bail!("push failed: {}", resp.error.unwrap_or_default());
    }

    Ok(())
}

async fn collect_zen(_cfg: &Config) -> Result<Option<crate::protocol::ZenState>> {
    // TODO: Фаза 2 — Zen export на Rust
    Ok(None)
}

async fn collect_projects(cfg: &Config) -> Result<Vec<crate::protocol::ProjectState>> {
    if let Some(projects) = &cfg.projects {
        crate::projects::status::scan(projects)
    } else {
        Ok(Vec::new())
    }
}
