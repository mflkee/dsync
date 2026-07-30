use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::PullRequest;

use super::connect::{connect, send_pull};

pub async fn pull(cfg: Config, _machine: Option<String>) -> Result<()> {
    info!("starting pull for {}", cfg.machine.name);

    let conn = connect(&cfg).await?;

    let req = PullRequest {
        machine: cfg.machine.name.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64,
        filter: None,
    };

    let resp = send_pull(&conn, &req).await?;
    info!("received state for {} machines", resp.machines.len());

    for (name, state) in &resp.machines {
        println!(
            "  {name}: {} projects, zen={}",
            state.projects.len(),
            state.zen.is_some(),
        );
    }

    // TODO: Фаза 2 — Zen import
    // TODO: Фаза 3 — git pull projects

    Ok(())
}
