use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use quinn::{Endpoint, Incoming, ServerConfig};
use tokio::signal;
use tracing::{error, info};

use crate::config::Config;
use crate::protocol::{PullRequest, PullResponse, PushRequest, PushResponse};

use super::state::HubState;

pub async fn run_server(cfg: Config) -> Result<()> {
    let bind: SocketAddr = cfg.hub.as_ref().map(|h| &h.bind).unwrap().parse()?;
    info!("starting dsync hub on {bind}");

    let data_dir = cfg
        .hub
        .as_ref()
        .and_then(|h| h.data_dir.clone())
        .unwrap_or_else(|| dirs::data_dir().unwrap_or_default().join("dsync"));

    let (cert, key) = load_or_generate_certs(&cfg)?;
    let server_config = make_server_config(cert, key)?;
    let endpoint = Endpoint::server(server_config, bind)?;
    let state = Arc::new(HubState::new(Some(data_dir)));

    info!("hub listening on {bind}");

    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                match incoming {
                    Some(incoming) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(incoming, state).await {
                                error!("connection error: {e}");
                            }
                        });
                    }
                    None => break,
                }
            }
            _ = signal::ctrl_c() => {
                info!("shutting down hub");
                endpoint.close(0u32.into(), b"shutdown");
                break;
            }
        }
    }

    info!("hub stopped");
    Ok(())
}

async fn handle_connection(
    incoming: Incoming,
    state: Arc<HubState>,
) -> Result<()> {
    let connection = incoming.await?;
    let remote = connection.remote_address();
    info!("new connection from {remote}");

    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let buf = recv.read_to_end(usize::MAX).await?;
                let msg_str = String::from_utf8_lossy(&buf);
                match serde_json::from_str::<serde_json::Value>(&msg_str) {
                    Ok(val) => {
                        let kind = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let resp = match kind {
                            "push" => handle_push(val, &state).await,
                            "pull" => handle_pull(val, &state).await,
                            "status" => handle_status(&state).await,
                            _ => {
                                error!("unknown message type: {kind}");
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

async fn handle_push(val: serde_json::Value, state: &HubState) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<PushRequest>(val) {
        let machine = req.machine.clone();
        state
            .update_machine(crate::protocol::MachineState {
                name: machine.clone(),
                last_push: req.timestamp,
                zen: req.zen,
                projects: req.projects,
            })
            .await;
        state.set_online(&machine, true).await;
        info!("push from {machine} accepted");
        serde_json::to_value(PushResponse { ok: true, error: None }).unwrap_or_default()
    } else {
        serde_json::to_value(PushResponse {
            ok: false,
            error: Some("invalid push request".into()),
        })
        .unwrap_or_default()
    }
}

async fn handle_pull(val: serde_json::Value, state: &HubState) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<PullRequest>(val) {
        let machines = state.all_machines().await;
        info!("pull from {}: {} machines", req.machine, machines.len());
        serde_json::to_value(PullResponse { machines }).unwrap_or_default()
    } else {
        serde_json::Value::Null
    }
}

async fn handle_status(state: &HubState) -> serde_json::Value {
    match state.status().await {
        Ok(resp) => serde_json::to_value(resp).unwrap_or_default(),
        Err(_) => serde_json::Value::Null,
    }
}

fn make_server_config(
    cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    priv_key: rustls::pki_types::PrivateKeyDer<'static>,
) -> Result<ServerConfig> {
    let mut config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, priv_key)?,
        )?,
    ));
    config.transport = Arc::new(quinn::TransportConfig::default());
    Ok(config)
}

fn load_or_generate_certs(
    cfg: &Config,
) -> Result<(
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
)> {
    if let Some(hub) = &cfg.hub {
        if let (Some(cert_path), Some(key_path)) = (&hub.cert, &hub.key) {
            let cert = std::fs::read(cert_path)?;
            let key = std::fs::read(key_path)?;
            let certs = rustls_pemfile::certs(&mut cert.as_slice())
                .collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile::private_key(&mut key.as_slice())?.unwrap();
            return Ok((certs, key));
        }
    }

    info!("no certs found, generating self-signed");
    let cert = rcgen::generate_simple_self_signed(vec!["dsync.local".into()])?;
    let cert_der = cert.cert.into();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        cert.key_pair.serialize_der().into(),
    );
    Ok((vec![cert_der], key_der))
}
