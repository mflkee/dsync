use anyhow::{bail, Result};
use tracing::info;

use crate::config::Config;
use crate::protocol::DeployRequest;

use super::connect::{close_conn, connect_with_retry, send_deploy};

/// Целевой деплой проекта на машину через хаб. Возвращает строки результата и
/// общий признак успеха (для кода возврата CLI).
pub async fn deploy(
    cfg: Config,
    machine: String,
    project: Option<String>,
    force: bool,
) -> Result<(Vec<String>, bool)> {
    // На клиенте [remote.*] может отсутствовать: список машин и адреса знает
    // хаб. Локально проверяем только если секция есть (понятная ошибка на
    // опечатку), иначе окончательную проверку делает хаб.
    if let Some(remotes) = cfg.remote.as_ref() {
        if !remotes.contains_key(&machine) {
            let mut known: Vec<&String> = remotes.keys().collect();
            known.sort();
            bail!(
                "unknown machine '{machine}' (known: {})",
                known
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    if let Some(name) = &project {
        let explicit = cfg
            .projects
            .as_ref()
            .map(|p| p.contains_key(name))
            .unwrap_or(false);
        if !explicit && cfg.auto_projects.is_none() {
            bail!("unknown project '{name}' (not in [projects.*])");
        }
    }

    info!(
        "deploying {} on {machine} (force={force})",
        project.as_deref().unwrap_or("all projects")
    );
    let conn = connect_with_retry(&cfg).await?;
    let req = DeployRequest {
        machine: cfg.machine.name.clone(),
        token: super::connect::hub_token(&cfg),
        target_machine: machine.clone(),
        project: project.clone(),
        force,
    };
    let resp = send_deploy(&conn, &req).await?;
    close_conn(&conn);

    let mut out = Vec::new();
    for r in &resp.results {
        if r.ok {
            let msg = r
                .message
                .as_deref()
                .map(|m| format!(" ({m})"))
                .unwrap_or_default();
            out.push(format!(
                "  ✓ {}/{}: ok, {} attempt(s), {}s{msg}",
                r.machine,
                r.project,
                r.attempts,
                r.finished_at.saturating_sub(r.started_at)
            ));
        } else {
            out.push(format!(
                "  ✗ {}/{}: {}",
                r.machine,
                r.project,
                r.error.as_deref().unwrap_or("failed")
            ));
        }
    }
    if let Some(err) = &resp.error {
        out.push(format!("deploy: {err}"));
    }
    Ok((out, resp.ok))
}
