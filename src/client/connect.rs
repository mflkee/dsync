use std::sync::Arc;

use anyhow::Result;
use quinn::{ClientConfig, Connection, Endpoint};
use rustls::ClientConfig as TlsClientConfig;
use tracing::info;

use crate::config::Config;
use crate::protocol::{PullRequest, PullResponse, PushRequest, PushResponse, StatusRequest, StatusResponse};

#[derive(Debug)]
struct SkipVerification;

impl rustls::client::danger::ServerCertVerifier for SkipVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

fn make_client_config() -> Result<ClientConfig> {
    let crypto = TlsClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerification))
        .with_no_client_auth();

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?;
    let mut config = ClientConfig::new(Arc::new(quic_config));
    config.transport_config(Arc::new(quinn::TransportConfig::default()));
    Ok(config)
}

pub async fn connect_with_retry(cfg: &Config) -> Result<Connection> {
    let addr = cfg
        .hub_connect
        .as_ref()
        .map(|h| &h.address)
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:42069".into());

    let endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;

    let mut last_connect_err = String::new();
    for attempt in 1..=4 {
        let config = make_client_config()?;
        match endpoint.connect_with(config, addr.parse()?, "dsync.local") {
            Ok(connecting) => match connecting.await {
                Ok(conn) => {
                    info!("connected to hub at {addr}");
                    return Ok(conn);
                }
                Err(e) => {
                    last_connect_err = format!("{e}");
                    info!("connect attempt {attempt}/4 failed: {e}");
                }
            },
            Err(e) => {
                last_connect_err = format!("{e}");
                info!("connect attempt {attempt}/4 failed: {e}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
    }

    anyhow::bail!("failed to connect to hub after 4 attempts: {last_connect_err}")
}

async fn send_recv(
    conn: &Connection,
    msg: &serde_json::Value,
) -> Result<Vec<u8>> {
    let (mut send, mut recv) = conn.open_bi().await?;
    let data = serde_json::to_vec(msg)?;
    send.write_all(&data).await?;
    send.finish()?;

    let buf = recv.read_to_end(usize::MAX).await?;
    Ok(buf)
}

pub async fn send_push(conn: &Connection, req: &PushRequest) -> Result<PushResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("push");
    let buf = send_recv(conn, &msg).await?;
    Ok(serde_json::from_slice(&buf)?)
}

pub async fn send_pull(conn: &Connection, req: &PullRequest) -> Result<PullResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("pull");
    let buf = send_recv(conn, &msg).await?;
    Ok(serde_json::from_slice(&buf)?)
}

pub async fn send_status(conn: &Connection, req: &StatusRequest) -> Result<StatusResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("status");
    let buf = send_recv(conn, &msg).await?;
    Ok(serde_json::from_slice(&buf)?)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn fmt_relative(ts: i64) -> String {
    let diff = unix_now() - ts;
    if diff < 0 {
        return "in the future".into();
    }
    let days = diff / 86400;
    let hours = diff / 3600;
    let mins = diff / 60;
    if days > 0 {
        format!("{days}d ago")
    } else if hours > 0 {
        format!("{hours}h ago")
    } else if mins > 0 {
        format!("{mins}m ago")
    } else {
        format!("{diff}s ago")
    }
}

fn fmt_civil(ts: i64) -> String {
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(dt) => dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
        None => format!("{ts}"),
    }
}

pub async fn status(cfg: Config) -> Result<()> {
    let conn = connect_with_retry(&cfg).await?;
    let req = StatusRequest {
        machine: cfg.machine.name.clone(),
    };
    let resp = send_status(&conn, &req).await?;

    println!("Sync Status:");
    for (name, status) in &resp.machines {
        println!(
            "  {}: online={}, last_push={} ({})",
            name,
            status.online,
            fmt_civil(status.last_push),
            fmt_relative(status.last_push),
        );
    }

    Ok(())
}
