use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::PushRequest;

use super::connect::{connect, send_push};

pub async fn push(cfg: Config, _machine: Option<String>) -> Result<()> {
    info!("starting push from {}", cfg.machine.name);

    let conn = connect(&cfg).await?;

    let projects = collect_projects(&cfg).await?;
    let zen = collect_zen(&cfg).await?;

    let req = PushRequest {
        machine: cfg.machine.name.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64,
        zen,
        projects,
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

async fn collect_zen(_cfg: &Config) -> Result<Option<crate::protocol::ZenState>> {
    // TODO: Фаза 2 — Zen export на Rust
    Ok(None)
}

async fn collect_projects(_cfg: &Config) -> Result<Vec<crate::protocol::ProjectState>> {
    // TODO: Фаза 3 — git status scanner на Rust
    Ok(Vec::new())
}
