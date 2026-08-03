use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::protocol::PushRequest;

use super::connect::{connect_with_retry, send_push};

pub async fn push(cfg: Config, _machine: Option<String>) -> Result<()> {
    info!("starting push from {}", cfg.machine.name);

    let conn = connect_with_retry(&cfg).await?;

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

async fn collect_zen(cfg: &Config) -> Result<Option<crate::protocol::ZenState>> {
    use sha2::Digest;

    match crate::zen::export::export(cfg) {
        Ok(data) => {
            let mut hasher = sha2::Sha256::new();
            hasher.update(&data);
            let checksum = hex::encode(hasher.finalize());
            info!(
                "zen export ready ({} bytes, checksum {})",
                data.len(),
                &checksum[..12]
            );
            Ok(Some(crate::protocol::ZenState { data, checksum }))
        }
        Err(e) => {
            tracing::warn!("zen export failed: {e}");
            Ok(None)
        }
    }
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
