use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::{PullFilter, PullRequest};

use super::connect::{connect_with_retry, send_pull};

pub async fn pull(cfg: Config, machine: Option<String>) -> Result<Vec<String>> {
    info!("starting pull for {}", cfg.machine.name);
    if let Some(m) = &machine {
        info!("pulling state for machine {m} only");
    }

    let conn = connect_with_retry(&cfg).await?;

    let req = PullRequest {
        machine: cfg.machine.name.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64,
        filter: Some(PullFilter {
            projects: true,
            machine: machine.clone(),
        }),
    };

    let resp = send_pull(&conn, &req).await?;
    info!("received state for {} machines", resp.machines.len());

    let mut out = Vec::new();
    for (name, state) in &resp.machines {
        out.push(format!("  {name}: {} projects", state.projects.len()));
    }

    Ok(out)
}
