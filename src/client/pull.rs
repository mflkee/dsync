use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::{PullFilter, PullRequest};
use crate::zen::import;

use super::connect::{connect_with_retry, send_pull};

pub async fn pull(cfg: Config, machine: Option<String>) -> Result<()> {
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
            zen: true,
            projects: true,
            machine: machine.clone(),
        }),
    };

    let resp = send_pull(&conn, &req).await?;
    info!("received state for {} machines", resp.machines.len());

    let mut zen_source: Option<(&String, &crate::protocol::MachineState)> = None;
    for (name, state) in &resp.machines {
        println!(
            "  {name}: {} projects, zen={}",
            state.projects.len(),
            state.zen.is_some(),
        );

        if state.zen.is_none() || name == &cfg.machine.name {
            continue;
        }
        if machine.is_some() || zen_source.map_or(true, |(_, s)| state.last_push > s.last_push) {
            zen_source = Some((name, state));
        }
    }

    if let Some((name, state)) = zen_source {
        if let Some(zen) = &state.zen {
            info!("importing Zen from {name} (last_push={})", state.last_push);
            if let Err(e) = import::import(&cfg, &zen.data) {
                tracing::warn!("failed to import Zen from {name}: {e}");
            }
        }
    }

    Ok(())
}
