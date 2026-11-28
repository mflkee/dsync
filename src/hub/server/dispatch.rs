//! Обработчики запросов клиентов: push/pull/state/status.

use super::pull::{deploy_all, deploy_named, trigger_remote_pulls};
use super::*;

pub(super) async fn handle_push(
    val: serde_json::Value,
    state: &Arc<HubState>,
    cfg: &Config,
) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<PushRequest>(val) {
        let machine = req.machine.clone();
        state
            .update_machine(crate::protocol::MachineState {
                name: machine.clone(),
                // Время ставит хаб своими часами: req.timestamp — это часы
                // клиента, и при их уходе вперёд/назад машина навсегда
                // выпадает из online (online = last_push <= 35 мин назад).
                last_push: now_secs(),
                projects: req.projects.clone(),
                pulls: std::collections::HashMap::new(),
                version: req.version.clone(),
            })
            .await;
        info!("push from {machine} accepted");

        trigger_remote_pulls(&req, cfg, state).await;

        serde_json::to_value(PushResponse {
            ok: true,
            error: None,
        })
        .unwrap_or_default()
    } else {
        error_envelope("invalid push request")
    }
}

pub(crate) async fn handle_deploy(
    val: serde_json::Value,
    state: &Arc<HubState>,
    cfg: &Config,
) -> serde_json::Value {
    let req = match serde_json::from_value::<DeployRequest>(val) {
        Ok(req) => req,
        Err(_) => return error_envelope("invalid deploy request"),
    };
    if req.target_machine == req.machine {
        return error_envelope("target machine is the requesting machine");
    }
    let Some(remotes) = cfg.remote.as_ref() else {
        return error_envelope("no [remote.*] config on hub");
    };
    if !remotes.contains_key(&req.target_machine) {
        return error_envelope(format!(
            "unknown target machine {} (known: {})",
            req.target_machine,
            remotes.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    let results = match req.project.as_deref() {
        Some(project) => match deploy_named(
            cfg,
            state,
            &req.machine,
            &req.target_machine,
            project,
            req.force,
        )
        .await
        {
            Some(r) => vec![r],
            None => {
                return error_envelope(format!("unknown project {project}"));
            }
        },
        None => deploy_all(cfg, state, &req.machine, &req.target_machine, req.force).await,
    };
    if results.is_empty() {
        return error_envelope(format!(
            "no projects configured for machine {}",
            req.target_machine
        ));
    }
    let ok = results.iter().all(|r| r.ok);
    serde_json::to_value(DeployResponse {
        ok,
        results,
        error: if ok {
            None
        } else {
            Some("one or more deploys failed".into())
        },
    })
    .unwrap_or_default()
}

pub(super) async fn handle_pull(val: serde_json::Value, state: &HubState) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<PullRequest>(val) {
        state.prune_stale().await;
        let mut machines = state.all_machines().await;
        if let Some(machine) = req.filter.as_ref().and_then(|f| f.machine.as_ref()) {
            machines.retain(|name, _| name == machine);
        }
        info!("pull from {}: {} machines", req.machine, machines.len());
        serde_json::to_value(PullResponse { machines }).unwrap_or_default()
    } else {
        error_envelope("invalid pull request")
    }
}

pub(super) async fn handle_state_push(
    val: serde_json::Value,
    state: &Arc<HubState>,
) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<StatePushRequest>(val) {
        let n = req.items.len();
        let seq = state.merge_state(req.items, &req.machine).await;
        info!("state push from {}: {n} item(s), seq={seq}", req.machine);
        serde_json::to_value(StatePushResponse {
            ok: true,
            error: None,
            state_seq: seq,
        })
        .unwrap_or_default()
    } else {
        error_envelope("invalid state_push request")
    }
}

pub(super) async fn handle_state_pull(
    val: serde_json::Value,
    state: &HubState,
    cfg: &Config,
) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<StatePullRequest>(val) {
        // Старый клиент не шлёт max_bytes → 0 (без лимита): иначе он получил бы
        // одну страницу и, проигнорировав курсор, потерял остальное.
        // Новому клиенту дополнительно ограничиваем страницу конфигом хаба.
        let hub_page = cfg
            .hub
            .as_ref()
            .map(|h| h.state_pull_page_bytes)
            .unwrap_or(0);
        let max_bytes = if req.max_bytes == 0 {
            0
        } else if hub_page == 0 {
            req.max_bytes
        } else {
            req.max_bytes.min(hub_page)
        };
        let (items, seq, next_seq, has_more) = state
            .state_since_page(
                req.state_seq,
                &req.machine,
                max_bytes,
                req.channels.as_deref(),
            )
            .await;
        info!(
            "state pull from {}: {} item(s){}",
            req.machine,
            items.len(),
            if has_more { ", more pages" } else { "" }
        );
        serde_json::to_value(StatePullResponse {
            items,
            state_seq: seq,
            next_seq,
            has_more,
        })
        .unwrap_or_default()
    } else {
        error_envelope("invalid state_pull request")
    }
}

/// Read-only сводка state store (вкладка State в TUI). Старый хаб (без
/// `state_status`) отвечает error-обёрткой, клиент показывает "unavailable".
pub(super) async fn handle_state_status(
    val: serde_json::Value,
    state: &HubState,
) -> serde_json::Value {
    if serde_json::from_value::<StateStatusRequest>(val).is_ok() {
        match state.state_status().await {
            Ok(resp) => serde_json::to_value(resp).unwrap_or_default(),
            Err(e) => error_envelope(format!("state_status: {e}")),
        }
    } else {
        error_envelope("invalid state_status request")
    }
}

pub(super) async fn handle_status(state: &HubState) -> serde_json::Value {
    state.prune_stale().await;
    match state.status().await {
        Ok(mut resp) => {
            resp.hub_version = Some(crate::protocol::version_string());
            serde_json::to_value(resp).unwrap_or_default()
        }
        Err(_) => serde_json::Value::Null,
    }
}
