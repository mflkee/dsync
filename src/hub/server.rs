use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quinn::{Endpoint, Incoming, ServerConfig};
use tokio::io::AsyncReadExt;
use tokio::signal;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::protocol::{
    error_envelope, PullOutcome, PullRequest, PullResponse, PushRequest, PushResponse,
};

use super::state::HubState;
use super::token_matches;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn run_server(cfg: Config) -> Result<()> {
    let hub_cfg = cfg
        .hub
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no [hub] section in config — can't run daemon"))?;

    // Fail-secure: без токенов хаб не стартует (см. hub-auth).
    if hub_cfg.tokens.is_empty() {
        anyhow::bail!(
            "hub refuses to start: [hub] tokens is missing/empty.\n\
             Configure per-machine tokens: rerun `dsync init` with the hub role, or set:\n\n\
             [hub]\n\
             tokens = {{ \"<machine>\": \"<token>\" }}\n\n\
             and put the matching [hub_connect] token on each client."
        );
    }

    let bind: SocketAddr = hub_cfg.bind.parse()?;
    info!("starting dsync hub on {bind}");

    let data_dir = cfg.hub_data_dir();

    let (cert, key) = load_or_generate_certs(&cfg, &data_dir)?;
    let server_config = make_server_config(cert, key)?;
    let endpoint = Endpoint::server(server_config, bind)?;

    let state = Arc::new(HubState::new(Some(data_dir), hub_cfg.retention_days));
    if state.prune_stale().await {
        info!("pruned stale machines at startup");
    }

    let max_message_size = hub_cfg.max_message_size;
    let semaphore = Arc::new(Semaphore::new(hub_cfg.max_concurrency as usize));
    let cfg = Arc::new(cfg);

    info!("hub listening on {bind}");

    // Основной цикл accept'а живёт в serve_loop (его же гоняют эндпоинт-тесты);
    // здесь ждём его завершения или Ctrl-C.
    let serve = serve_loop(endpoint, state, cfg, max_message_size, semaphore);
    tokio::select! {
        _ = serve => {}
        _ = signal::ctrl_c() => {
            info!("shutting down hub");
        }
    }

    info!("hub stopped");
    Ok(())
}

/// Основной цикл обработки соединений — вынесен отдельно, чтобы тесты могли
/// поднять реальный quinn-endpoint и прогнать auth/limits без daemon-цикла.
async fn serve_loop(
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
                            "push" | "pull" | "status" if authorized(&val, cfg.hub.as_ref()) => {
                                match kind {
                                    "push" => handle_push(val, &state, &cfg).await,
                                    "pull" => handle_pull(val, &state).await,
                                    _ => handle_status(&state).await,
                                }
                            }
                            "push" | "pull" | "status" => {
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

async fn handle_push(
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
                last_push: unix_now(),
                projects: req.projects.clone(),
                pulls: std::collections::HashMap::new(),
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

async fn trigger_remote_pulls(req: &PushRequest, cfg: &Config, state: &Arc<HubState>) {
    let (Some(projects_cfg), Some(remote_cfg)) = (&cfg.projects, &cfg.remote) else {
        return;
    };
    let pull_retries = cfg
        .hub
        .as_ref()
        .map(|h| h.pull_retries)
        .unwrap_or(crate::config::default_pull_retries());
    let store_path = crate::ssh::trust::SshHostTrustStore::path(&cfg.hub_data_dir());
    let ssh_key = cfg.machine.ssh_key_path();

    for project in &req.projects {
        let Some(project_cfg) = projects_cfg.get(&project.name) else {
            continue;
        };
        let Some(machines) = &project_cfg.machines else {
            continue;
        };

        for machine_name in machines {
            if machine_name == &req.machine {
                continue;
            }
            if let Some(target) = &req.target {
                if machine_name != target {
                    continue;
                }
            }
            let Some(remote) = remote_cfg.get(machine_name) else {
                error!("no remote config for machine {machine_name}");
                continue;
            };

            let host = remote.host.clone();
            let port = remote.port;
            let user = remote.user.clone();
            // Путь раскрываем в абсолютный (~/...) и берём в одинарные
            // кавычки: пробел или спецсимвол в пути/ветке иначе ломает
            // команду на удалённом shell'е.
            let path = crate::projects::status::expand_user_path(&project_cfg.path);
            let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
            let branch = project_cfg.branch.as_deref().unwrap_or("main").to_string();
            let project_name = project.name.clone();
            let machine_name = machine_name.clone();
            let mut cmd = format!(
                "cd {} && git stash push && git pull --rebase origin {}",
                q(&path.display().to_string()),
                q(&branch),
            );
            if let Some(post) = &project_cfg.post_pull {
                let post = post.trim();
                if !post.is_empty() {
                    cmd.push_str(" && ");
                    cmd.push_str(post);
                }
            }

            let state = state.clone();
            let store_path = store_path.clone();
            let ssh_key = ssh_key.clone();
            tokio::spawn(async move {
                info!("SSH pulling {project_name} on {machine_name} ({host})...");
                let outcome = retry_pull(pull_retries, || {
                    let store_path = store_path.clone();
                    let host = host.clone();
                    let user = user.clone();
                    let cmd = cmd.clone();
                    let ssh_key = ssh_key.clone();
                    async move {
                        match crate::ssh::client::exec_with_key_verifying(
                            &host, port, &user, &cmd, &ssh_key, store_path,
                        )
                        .await
                        {
                            Ok(_) => Ok(()),
                            Err(e) => Err(format!("{e:#}")),
                        }
                    }
                })
                .await;
                state
                    .record_pull(&machine_name, &project_name, &outcome)
                    .await;
                match &outcome {
                    PullOutcome { ok: true, .. } => {
                        info!("SSH pull {machine_name}/{project_name}: OK")
                    }
                    PullOutcome { ok: false, .. } => error!(
                        "SSH pull {machine_name}/{project_name} failed after {} attempts: {}",
                        outcome.attempts,
                        outcome.error.as_deref().unwrap_or("unknown error")
                    ),
                }
            });
        }
    }
}

/// Retry-обёртка для SSH-пулла: первая попытка + `pull_retries` повторных,
/// backoff `5s·2^n` с потолком 60 с. Возвращает итоговый `PullOutcome`
/// (см. `hub-pull-orchestration`); тестируется с pause-time.
async fn retry_pull<F, Fut>(pull_retries: u32, mut attempt: F) -> PullOutcome
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut attempts: u32 = 0;
    let mut backoff = Duration::from_secs(5);
    loop {
        attempts += 1;
        match attempt().await {
            Ok(()) => return PullOutcome::success(attempts),
            Err(err) => {
                if attempts > pull_retries {
                    return PullOutcome::failure(err, attempts);
                }
                let wait = backoff.min(Duration::from_secs(60));
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                backoff = backoff.saturating_mul(2);
            }
        }
    }
}

async fn handle_pull(val: serde_json::Value, state: &HubState) -> serde_json::Value {
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

async fn handle_status(state: &HubState) -> serde_json::Value {
    state.prune_stale().await;
    match state.status().await {
        Ok(resp) => serde_json::to_value(resp).unwrap_or_default(),
        Err(_) => serde_json::Value::Null,
    }
}

fn make_server_config(
    cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    priv_key: rustls::pki_types::PrivateKeyDer<'static>,
) -> Result<ServerConfig> {
    let mut config =
        ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, priv_key)?,
        )?));
    config.transport = Arc::new(quinn::TransportConfig::default());
    Ok(config)
}

fn load_or_generate_certs(
    cfg: &Config,
    data_dir: &std::path::Path,
) -> Result<(
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
)> {
    // 1. Явно заданные через [hub] cert/key — приоритет.
    if let Some(hub) = &cfg.hub {
        if let (Some(cert_path), Some(key_path)) = (&hub.cert, &hub.key) {
            let cert = std::fs::read(cert_path)?;
            let key = std::fs::read(key_path)?;
            let certs =
                rustls_pemfile::certs(&mut cert.as_slice()).collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile::private_key(&mut key.as_slice())?.unwrap();
            return Ok((certs, key));
        }
    }

    // 2. Сертификат из data_dir (сохранён при прошлом запуске) — чтобы
    //    fingerprint был стабильным между рестартами и клиенты с TOFU
    //    не ломались после каждого перезапуска хаба.
    let cert_file = data_dir.join("server_cert.pem");
    let key_file = data_dir.join("server_key.pem");
    if cert_file.exists() && key_file.exists() {
        let cert = std::fs::read(&cert_file)?;
        let key = std::fs::read(&key_file)?;
        let certs = rustls_pemfile::certs(&mut cert.as_slice()).collect::<Result<Vec<_>, _>>()?;
        if let Some(key) = rustls_pemfile::private_key(&mut key.as_slice())? {
            return Ok((certs, key));
        }
        anyhow::bail!("server_key.pem in {data_dir:?} contains no private key");
    }

    // 3. Генерируем новый и сохраняем на диск.
    info!(
        "no certs found, generating self-signed (persisted in {})",
        data_dir.display()
    );
    let cert = rcgen::generate_simple_self_signed(vec!["dsync.local".into()])?;
    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&cert_file, cert_pem)?;
    std::fs::write(&key_file, key_pem)?;

    let cert_der = std::fs::read(&cert_file)?;
    let key_der = std::fs::read(&key_file)?;
    let certs = rustls_pemfile::certs(&mut cert_der.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_der.as_slice())?.unwrap();
    Ok((certs, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use crate::config::{HubConfig, MachineConfig, ProjectConfig, RemoteMachine};

    /// Принимающий всё verifier — тестовый аналог TOFU-клиента.
    #[derive(Debug)]
    struct AcceptAll;

    impl rustls::client::danger::ServerCertVerifier for AcceptAll {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error>
        {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
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

    fn test_server_config() -> ServerConfig {
        let cert = rcgen::generate_simple_self_signed(vec!["dsync.local".into()]).unwrap();
        let cert_der = cert.cert.der().clone();
        let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
        make_server_config(vec![cert_der], key_der).unwrap()
    }

    fn test_client_config() -> quinn::ClientConfig {
        let crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAll))
            .with_no_client_auth();
        let quic = quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap();
        quinn::ClientConfig::new(Arc::new(quic))
    }

    fn test_config(tokens: &[(&str, &str)], max_message_size: u64, max_concurrency: u32) -> Config {
        Config {
            config_version: 1,
            machine: MachineConfig {
                name: "hub-machine".into(),
                ssh_key: None,
            },
            hub: Some(HubConfig {
                bind: "127.0.0.1:0".into(),
                cert: None,
                key: None,
                data_dir: None,
                tokens: tokens
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
                max_message_size,
                max_concurrency,
                retention_days: 30,
                pull_retries: 2,
            }),
            hub_connect: None,
            projects: Some(
                [(
                    "dotfiles".to_string(),
                    ProjectConfig {
                        path: "/tmp/nonexistent-dsync-test".into(),
                        branch: Some("main".into()),
                        machines: Some(vec!["notebook".into()]),
                        post_pull: None,
                    },
                )]
                .into_iter()
                .collect(),
            ),
            remote: Some(
                [(
                    "notebook".to_string(),
                    RemoteMachine {
                        host: "127.0.0.1".into(),
                        port: 1, // несуществующий порт — SSH сразу упадёт
                        user: "nobody".into(),
                    },
                )]
                .into_iter()
                .collect(),
            ),
            capture: None,
        }
    }

    async fn spawn_test_hub(
        tokens: &[(&str, &str)],
        max_message_size: u64,
        max_concurrency: u32,
    ) -> (SocketAddr, Arc<HubState>, Arc<Semaphore>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server_cfg = test_server_config();
        let endpoint = Endpoint::server(server_cfg, "127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = endpoint.local_addr().unwrap();
        let state = Arc::new(HubState::new(None, 30));
        let cfg = Arc::new(test_config(tokens, max_message_size, max_concurrency));
        let semaphore = Arc::new(Semaphore::new(max_concurrency as usize));
        tokio::spawn(serve_loop(
            endpoint,
            state.clone(),
            cfg,
            max_message_size,
            semaphore.clone(),
        ));
        (addr, state, semaphore)
    }

    async fn connect(addr: SocketAddr) -> quinn::Connection {
        let endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint
            .connect_with(test_client_config(), addr, "dsync.local")
            .unwrap()
            .await
            .unwrap()
    }

    async fn send_push(
        conn: &quinn::Connection,
        machine: &str,
        token: &str,
    ) -> anyhow::Result<crate::protocol::PushResponse> {
        let req = PushRequest {
            machine: machine.to_string(),
            token: token.to_string(),
            timestamp: unix_now(),
            projects: vec![],
            target: None,
        };
        crate::client::connect::send_push(conn, &req).await
    }

    #[tokio::test]
    async fn auth_accepts_valid_push_and_rejects_others() {
        let (addr, state, _sem) = spawn_test_hub(&[("desktop", "secret-a")], 1 << 20, 4).await;
        let conn = connect(addr).await;

        // Неизвестная машина — отказ, состояние не тронуто.
        let r = send_push(&conn, "desktop", "wrong-token").await;
        assert!(r.is_err(), "wrong token must be rejected");
        assert!(
            r.unwrap_err().to_string().contains("authentication failed"),
            "actionable error expected"
        );

        let r = send_push(&conn, "intruder", "secret-a").await;
        assert!(r.is_err(), "unknown machine must be rejected");
        assert!(
            state.all_machines().await.is_empty(),
            "no state changes on rejection"
        );

        // Валидный push — принят, состояние записано.
        let resp = send_push(&conn, "desktop", "secret-a").await.unwrap();
        assert!(resp.ok);
        let m = state.all_machines().await;
        assert_eq!(m.len(), 1);
        assert_eq!(m["desktop"].name, "desktop");
    }

    #[tokio::test]
    async fn oversized_request_rejected_and_small_accepted() {
        let (addr, state, _sem) = spawn_test_hub(&[("desktop", "t")], 1024, 4).await;
        let conn = connect(addr).await;

        // Невалидный JSON, но большой — должен упасть по размеру раньше разбора.
        let big = vec![b'x'; 4096];
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        send.write_all(&big).await.unwrap();
        send.finish().unwrap();
        let buf = recv.read_to_end(usize::MAX).await.unwrap();
        let val: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(val["type"], "error");
        assert!(
            val["error"].as_str().unwrap().contains("max_message_size"),
            "explicit size error"
        );

        // Малый запрос проходит.
        let resp = send_push(&conn, "desktop", "t").await.unwrap();
        assert!(resp.ok);
        assert_eq!(state.all_machines().await.len(), 1);
    }

    #[tokio::test]
    async fn concurrency_cap_yields_explicit_busy_error() {
        let max = 1;
        let (addr, _state, semaphore) = spawn_test_hub(&[("desktop", "t")], 1 << 20, max).await;
        let conn = connect(addr).await;

        // Захватываем единственный permit сервера вручную — второй запрос
        // должен получить «hub busy» через 5с таймаут ожидания, а не молча
        // пропасть (и не дождаться обработки).
        let _held = semaphore.acquire_owned().await.unwrap();

        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let req = PushRequest {
            machine: "desktop".into(),
            token: "t".into(),
            timestamp: unix_now(),
            projects: vec![],
            target: None,
        };
        let mut msg = serde_json::to_value(&req).unwrap();
        msg["type"] = serde_json::json!("push");
        send.write_all(&serde_json::to_vec(&msg).unwrap())
            .await
            .unwrap();
        send.finish().unwrap();
        let buf = recv.read_to_end(usize::MAX).await.unwrap();
        let val: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(val["type"], "error");
        assert!(val["error"].as_str().unwrap().contains("hub busy"));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_first_attempt_fails_then_succeeds() {
        let mut calls: u32 = 0;
        let outcome = retry_pull(2, || {
            calls += 1;
            async move {
                if calls == 1 {
                    Err("ssh dead".to_string())
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert!(outcome.ok, "pulled after the first failure");
        assert_eq!(outcome.attempts, 2);
        assert_eq!(outcome.error, None);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_all_failures_store_last_error() {
        let outcome = retry_pull(2, || async { Err("boom".to_string()) }).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.attempts, 3, "initial attempt + 2 retries");
        assert_eq!(outcome.error.as_deref(), Some("boom"));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_zero_pull_retries_single_attempt() {
        let outcome = retry_pull(0, || async { Err("nope".to_string()) }).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.attempts, 1);
        assert_eq!(outcome.error.as_deref(), Some("nope"));
    }
}
