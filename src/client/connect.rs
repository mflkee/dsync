use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use quinn::{ClientConfig, Connection, Endpoint};
use rustls::ClientConfig as TlsClientConfig;
use tracing::{info, warn};

use crate::config::Config;
use crate::protocol::{
    MachineStatus, PullRequest, PullResponse, PushRequest, PushResponse, StatePullRequest,
    StatePullResponse, StatePushRequest, StatePushResponse, StateStatusRequest,
    StateStatusResponse, StatusRequest, StatusResponse,
};
use crate::trust::{fingerprint, TrustStore};

/// TOFU-верификатор: при первом подключении к хабу запоминает отпечаток
/// сертификата (trust on first use), при последующих — сверяет с known_hosts.
/// Изменение отпечатка = возможный MITM или переустановка хаба — отказ.
#[derive(Debug)]
struct TofuVerifier {
    /// Ожидаемый отпечаток из known_hosts (None = первый контакт).
    expected: Option<String>,
    /// Сюда кладётся отпечаток, который сертификат предъявил на самом деле.
    observed: Arc<Mutex<Option<String>>>,
}

impl rustls::client::danger::ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        if let Ok(mut obs) = self.observed.lock() {
            *obs = Some(fp.clone());
        }
        if let Some(expected) = &self.expected {
            if expected != &fp {
                return Err(rustls::Error::General(format!(
                    "hub fingerprint mismatch: server presented {fp}, known_hosts has {expected}. \
                     Possible MITM or hub reinstall — run `dsync trust rm <addr>` to accept the new one"
                )));
            }
        }
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

fn make_client_config(
    expected: Option<String>,
    observed: Arc<Mutex<Option<String>>>,
) -> Result<ClientConfig> {
    let verifier = TofuVerifier { expected, observed };
    let crypto = TlsClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
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

    // TOFU: ожидаемый отпечаток из known_hosts; наблюдаемый — из handshake.
    let mut store = TrustStore::load();
    let expected = store.get(&addr).map(str::to_string);
    let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;

    let mut last_connect_err = String::new();
    for attempt in 1..=4 {
        let config = make_client_config(expected.clone(), observed.clone())?;
        match endpoint.connect_with(config, addr.parse()?, "dsync.local") {
            Ok(connecting) => {
                // Жёсткий таймаут на рукопожатие: к мёртвому хабу quinn сам
                // висит 20-30+ с на ретраях PTO, и весь цикл из 4 попыток
                // растягивался бы на минуты (TUI при этом висел на
                // «⟳ refresh…» без ошибки). 5с на попытку достаточно даже
                // для медленного разогрева хаба (спящий/загрузка).
                match tokio::time::timeout(Duration::from_secs(5), connecting).await {
                    Ok(Ok(conn)) => {
                        // Первый контакт: запоминаем отпечаток (TOFU).
                        if let Ok(obs) = observed.lock() {
                            if let Some(fp) = obs.as_ref() {
                                if store.get(&addr).is_none() {
                                    warn!(
                                        "first connection to hub {addr}: trusting \
                                         fingerprint {fp} (add `dsync trust list` to verify)"
                                    );
                                    store.insert(&addr, fp);
                                    if let Err(e) = store.save() {
                                        warn!(
                                            "can't persist known_hosts ({}): \
                                             future connections will re-trust",
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        info!("connected to hub at {addr}");
                        return Ok(conn);
                    }
                    Ok(Err(e)) => {
                        last_connect_err = format!("{e}");
                        info!("connect attempt {attempt}/4 failed: {e}");
                    }
                    Err(_) => {
                        last_connect_err = "handshake timed out after 5s".into();
                        info!("connect attempt {attempt}/4 failed: timed out after 5s");
                    }
                }
            }
            Err(e) => {
                last_connect_err = format!("{e}");
                info!("connect attempt {attempt}/4 failed: {e}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
    }

    anyhow::bail!("failed to connect to hub after 4 attempts: {last_connect_err}")
}

async fn send_recv(conn: &Connection, msg: &serde_json::Value) -> Result<Vec<u8>> {
    let (mut send, mut recv) = conn.open_bi().await?;
    let data = serde_json::to_vec(msg)?;
    send.write_all(&data).await?;
    send.finish()?;

    let buf = recv.read_to_end(usize::MAX).await?;
    Ok(buf)
}

/// Вежливо закрыть соединение после выполненного запроса. Без этого хаб
/// держит связь до idle-таймаута и пишет в лог «connection error… timed out»
/// на каждый оставленный клиентом «висяк».
pub fn close_conn(conn: &Connection) {
    conn.close(0u32.into(), b"done");
}

pub async fn send_push(conn: &Connection, req: &PushRequest) -> Result<PushResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("push");
    let buf = send_recv(conn, &msg).await?;
    crate::protocol::parse_response(&buf)
}

pub async fn send_pull(conn: &Connection, req: &PullRequest) -> Result<PullResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("pull");
    let buf = send_recv(conn, &msg).await?;
    crate::protocol::parse_response(&buf)
}

pub async fn send_state_push(
    conn: &Connection,
    req: &StatePushRequest,
) -> Result<StatePushResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("state_push");
    let buf = send_recv(conn, &msg).await?;
    crate::protocol::parse_response(&buf)
}

pub async fn send_state_pull(
    conn: &Connection,
    req: &StatePullRequest,
) -> Result<StatePullResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("state_pull");
    let buf = send_recv(conn, &msg).await?;
    crate::protocol::parse_response(&buf)
}

pub async fn send_status(conn: &Connection, req: &StatusRequest) -> Result<StatusResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("status");
    let buf = send_recv(conn, &msg).await?;
    crate::protocol::parse_response(&buf)
}

pub async fn send_state_status(
    conn: &Connection,
    req: &StateStatusRequest,
) -> Result<StateStatusResponse> {
    let mut msg = serde_json::to_value(req)?;
    msg["type"] = serde_json::json!("state_status");
    let buf = send_recv(conn, &msg).await?;
    crate::protocol::parse_response(&buf)
}

/// Hub auth token from `[hub_connect]`, empty when unset (hub will reject).
pub fn hub_token(cfg: &Config) -> String {
    // Токен из `[hub_connect]`, иначе — sidecar `tokens.toml` (главный конфиг
    // перегенерируется chezmoi-apply, секреты в нём не живут).
    if let Some(t) = cfg
        .hub_connect
        .as_ref()
        .map(|h| h.token.trim())
        .filter(|t| !t.is_empty())
    {
        return t.to_string();
    }
    crate::config::read_secret_tokens()
        .hub_connect
        .and_then(|h| h.token)
        .unwrap_or_default()
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
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => format!("{ts}"),
    }
}

pub async fn status(cfg: Config) -> Result<Vec<String>> {
    let conn = connect_with_retry(&cfg).await?;
    let req = StatusRequest {
        machine: cfg.machine.name.clone(),
        token: hub_token(&cfg),
    };
    let resp = send_status(&conn, &req).await?;
    close_conn(&conn);

    let mut out = vec!["Sync Status:".to_string()];
    for (name, status) in &resp.machines {
        out.extend(machine_status_lines(name, status));
        if status.pulls.is_empty() && resp.machines.len() > 1 {
            out.push("      (no pulls yet)".to_string());
        }
    }

    Ok(out)
}

/// Строки статуса одной машины, включая исходы последних SSH-пуллов хаба
/// (pull-orchestration) — вынесено в чистую функцию ради юнит-тестов 4.3.
fn machine_status_lines(name: &str, status: &MachineStatus) -> Vec<String> {
    let mut lines = vec![format!(
        "  {name}: online={}, last_push={} ({})",
        status.online,
        fmt_civil(status.last_push),
        fmt_relative(status.last_push),
    )];
    let mut pulls: Vec<_> = status.pulls.iter().collect();
    pulls.sort_by_key(|(p, _)| (*p).clone());
    for (project, outcome) in pulls {
        let res = if outcome.ok {
            "ok".to_string()
        } else {
            format!("FAILED ({})", outcome.error.as_deref().unwrap_or("?"))
        };
        lines.push(format!(
            "      {project}: pull {res}, {} attempt(s), finished {}",
            outcome.attempts,
            fmt_civil(outcome.finished_at),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PullOutcome;

    fn outcome(ok: bool, err: Option<&str>, attempts: u32, finished_at: i64) -> PullOutcome {
        PullOutcome {
            ok,
            error: err.map(str::to_string),
            attempts,
            finished_at,
        }
    }

    #[test]
    fn machine_lines_show_ok_and_failed_pulls() {
        let mut pulls = std::collections::HashMap::new();
        pulls.insert(
            "dotfiles".to_string(),
            outcome(true, None, 1, 1_700_000_000),
        );
        pulls.insert(
            "notes".to_string(),
            outcome(false, Some("ssh timed out"), 3, 1_700_000_100),
        );
        let status = MachineStatus {
            online: true,
            last_seen: 1_700_000_000,
            last_push: 1_700_000_000,
            pulls,
        };
        let lines = machine_status_lines("desktop", &status);
        assert!(lines[0].contains("desktop: online=true"));
        assert!(lines
            .iter()
            .any(|l| l.contains("dotfiles: pull ok, 1 attempt(s)")));
        assert!(lines
            .iter()
            .any(|l| l.contains("notes: pull FAILED (ssh timed out), 3 attempt(s)")));
    }

    #[test]
    fn machine_lines_without_pulls_show_header_only() {
        let status = MachineStatus {
            online: false,
            last_seen: 0,
            last_push: 0,
            pulls: std::collections::HashMap::new(),
        };
        let lines = machine_status_lines("notebook", &status);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("notebook: online=false"));
    }
}
