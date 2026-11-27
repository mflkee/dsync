//! Accept-loop QUIC и проверка hub-токена.

use super::dispatch::{
    handle_pull, handle_push, handle_state_pull, handle_state_push, handle_state_status,
    handle_status,
};
use super::*;

pub(super) async fn serve_loop(
    endpoint: Endpoint,
    state: Arc<HubState>,
    cfg: Arc<Config>,
    max_message_size: u64,
    semaphore: Arc<Semaphore>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        let cfg = cfg.clone();
        let sem = semaphore.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(incoming, state, cfg, max_message_size, sem).await {
                error!("connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    incoming: Incoming,
    state: Arc<HubState>,
    cfg: Arc<Config>,
    max_message_size: u64,
    semaphore: Arc<Semaphore>,
) -> Result<()> {
    let connection = incoming.await?;
    let remote = connection.remote_address();
    info!("new connection from {remote}");

    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                // Ограничение параллельности: 5с на слот, иначе — явная ошибка
                // «hub busy», а не молчаливая потеря запроса.
                let permit = match tokio::time::timeout(
                    Duration::from_secs(5),
                    semaphore.clone().acquire_owned(),
                )
                .await
                {
                    Ok(Ok(p)) => p,
                    Ok(Err(_)) => {
                        error!("semaphore closed");
                        break;
                    }
                    Err(_) => {
                        let err =
                            error_envelope("hub busy — retry later (max_concurrency reached)");
                        let _ = send.write_all(&serde_json::to_vec(&err)?).await;
                        continue;
                    }
                };

                // Ограничение размера сообщения: читаем не более limit+1 байт.
                let mut buf = Vec::new();
                {
                    let mut limited = (&mut recv).take(max_message_size + 1);
                    let _ = limited.read_to_end(&mut buf).await;
                }
                if buf.len() as u64 > max_message_size {
                    let err = error_envelope(format!(
                        "request exceeds max_message_size ({max_message_size} bytes)"
                    ));
                    let _ = send.write_all(&serde_json::to_vec(&err)?).await;
                    continue;
                }

                let msg_str = String::from_utf8_lossy(&buf);
                match serde_json::from_str::<serde_json::Value>(&msg_str) {
                    Ok(val) => {
                        let kind = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let resp = match kind {
                            "push" | "pull" | "status" | "state_push" | "state_pull"
                            | "state_status"
                                if authorized(&val, cfg.hub.as_ref()) =>
                            {
                                match kind {
                                    "push" => handle_push(val, &state, &cfg).await,
                                    "pull" => handle_pull(val, &state).await,
                                    "state_push" => handle_state_push(val, &state).await,
                                    "state_pull" => handle_state_pull(val, &state, &cfg).await,
                                    "state_status" => handle_state_status(val, &state).await,
                                    _ => handle_status(&state).await,
                                }
                            }
                            "push" | "pull" | "status" | "state_push" | "state_pull"
                            | "state_status" => {
                                let machine =
                                    val.get("machine").and_then(|v| v.as_str()).unwrap_or("?");
                                warn!("authentication failed for machine '{machine}'");
                                error_envelope(format!(
                                    "authentication failed for machine '{machine}': check \
                                     [hub_connect] token and [hub] tokens"
                                ))
                            }
                            _ => {
                                error!("unknown message type: {kind}");
                                let err = error_envelope(format!("unknown message type: {kind}"));
                                let _ = send.write_all(&serde_json::to_vec(&err)?).await;
                                continue;
                            }
                        };

                        let data = serde_json::to_vec(&resp)?;
                        send.write_all(&data).await?;
                    }
                    Err(e) => {
                        error!("invalid JSON: {e}");
                    }
                }
                drop(permit);
            }
            Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                info!("connection from {remote} closed");
                break;
            }
            Err(e) => {
                error!("connection error from {remote}: {e}");
                break;
            }
        }
    }

    Ok(())
}

/// Проверяет, что запрос несёт валидный токен известной машины флота.
fn authorized(val: &serde_json::Value, hub: Option<&crate::config::HubConfig>) -> bool {
    let Some(hub) = hub else {
        return false;
    };
    let Some(machine) = val.get("machine").and_then(|v| v.as_str()) else {
        return false;
    };
    let token = val.get("token").and_then(|v| v.as_str()).unwrap_or("");
    match hub.tokens.get(machine) {
        Some(expected) => token_matches(expected, token),
        None => false,
    }
}
