use super::*;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::config::{HubConfig, MachineConfig, ProjectConfig, RemoteMachine};

#[test]
fn hub_token_precedence_config_over_sidecar() {
    use std::collections::HashMap;
    // Конфиг непустой → победил конфиг.
    let cfg_toks = HashMap::from([("desktop".to_string(), "cfg-tok".to_string())]);
    let secret = SecretTokens {
        hub: Some(crate::config::SecretHub {
            tokens: HashMap::from([("desktop".to_string(), "file-tok".to_string())]),
        }),
        hub_connect: None,
    };
    let got = effective_hub_tokens(&cfg_toks, secret);
    assert_eq!(got["desktop"], "cfg-tok");
}

#[test]
fn hub_token_precedence_sidecar_when_config_empty() {
    use std::collections::HashMap;
    let secret = SecretTokens {
        hub: Some(crate::config::SecretHub {
            tokens: HashMap::from([("notebook".to_string(), "file-tok".to_string())]),
        }),
        hub_connect: None,
    };
    let got = effective_hub_tokens(&HashMap::new(), secret);
    assert_eq!(got["notebook"], "file-tok");
}

#[test]
fn hub_token_precedence_empty_when_both_empty() {
    use std::collections::HashMap;
    let got = effective_hub_tokens(&HashMap::new(), SecretTokens::default());
    assert!(got.is_empty());
}

#[test]
fn should_pull_project_semantics() {
    // Явные [projects.*] пулятся всегда, даже при контроль-режиме.
    assert!(should_pull_project(false, Some(false)));
    assert!(should_pull_project(false, None));
    assert!(should_pull_project(false, Some(true)));
    // Авто-проекты: по умолчанию (None) и sync=true — разворачиваются.
    assert!(should_pull_project(true, None));
    assert!(should_pull_project(true, Some(true)));
    // Контроль-режим: авто-проекты анонсируются, но не пулятся.
    assert!(!should_pull_project(true, Some(false)));
}

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
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
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
            pull_timeout_secs: crate::config::default_pull_timeout_secs(),
            pull_only_on_change: true,
            offline_skip_after_secs: 0,
            state_pull_page_bytes: crate::config::default_state_pull_page_bytes(),
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
        auto_projects: None,
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
        state: None,
    }
}

/// Уникальный каталог данных для тестового хаба (иначе state не хранится).
static HUB_TEST_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

async fn spawn_test_hub(
    tokens: &[(&str, &str)],
    max_message_size: u64,
    max_concurrency: u32,
) -> (SocketAddr, Arc<HubState>, Arc<Semaphore>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server_cfg = test_server_config();
    let endpoint = Endpoint::server(server_cfg, "127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = endpoint.local_addr().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "dsync-hub-test-{}-{}",
        std::process::id(),
        HUB_TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let state = Arc::new(HubState::new(Some(dir), 30));
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
        timestamp: now_secs(),
        projects: vec![],
        target: None,
        version: None,
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
        timestamp: now_secs(),
        projects: vec![],
        target: None,
        version: None,
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

#[tokio::test]
async fn state_push_pull_routing_and_self_exclusion() {
    use crate::protocol::{StateItem, StatePullRequest, StatePushRequest};

    let (addr, _state, _sem) =
        spawn_test_hub(&[("desktop", "td"), ("notebook", "tn")], 1 << 20, 4).await;
    let conn = connect(addr).await;

    // desktop загружает элемент состояния.
    let push = StatePushRequest {
        machine: "desktop".into(),
        token: "td".into(),
        timestamp: now_secs(),
        items: vec![StateItem {
            channel: "opencode".into(),
            key: "ses_1".into(),
            updated: 5,
            data: r#"{"k":1}"#.into(),
            ..Default::default()
        }],
    };
    let resp = crate::client::connect::send_state_push(&conn, &push)
        .await
        .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.state_seq, 1);

    // notebook видит элемент.
    let pull = StatePullRequest {
        machine: "notebook".into(),
        token: "tn".into(),
        state_seq: 0,
        max_bytes: 0,
        channels: None,
    };
    let r = crate::client::connect::send_state_pull(&conn, &pull)
        .await
        .unwrap();
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].key, "ses_1");
    assert_eq!(r.items[0].origin, "desktop");
    assert_eq!(r.items[0].data, r#"{"k":1}"#);
    assert_eq!(r.state_seq, 1);

    // desktop не получает свой же элемент обратно.
    let own = StatePullRequest {
        machine: "desktop".into(),
        token: "td".into(),
        state_seq: 0,
        max_bytes: 0,
        channels: None,
    };
    assert!(crate::client::connect::send_state_pull(&conn, &own)
        .await
        .unwrap()
        .items
        .is_empty());

    // Чужой токен — отказ (и не утечка).
    let bad = StatePullRequest {
        machine: "notebook".into(),
        token: "wrong".into(),
        state_seq: 0,
        max_bytes: 0,
        channels: None,
    };
    let err = crate::client::connect::send_state_pull(&conn, &bad).await;
    assert!(err.is_err(), "bad token must be rejected");
    assert!(err
        .unwrap_err()
        .to_string()
        .contains("authentication failed"));
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

#[test]
fn pull_skip_reason_offline_and_unchanged() {
    let now = 1_000_000;
    // Машина офлайн дольше порога — не пулим.
    assert!(pull_skip_reason(true, 2700, now, Some(now - 3600), None, "abc").is_some());
    // Недавний push и новый commit — пулим.
    assert!(pull_skip_reason(true, 2700, now, Some(now - 60), None, "abc").is_none());
    // Этот commit уже развёрнут — не пулим.
    assert!(pull_skip_reason(true, 2700, now, Some(now - 60), Some("abc"), "abc").is_some());
    // Change-detection выключен — пулим даже тот же commit.
    assert!(pull_skip_reason(false, 0, now, Some(now - 60), Some("abc"), "abc").is_none());
    // Репо без коммитов — определять нечего, пулим.
    assert!(pull_skip_reason(true, 2700, now, Some(now - 60), Some(""), "").is_none());
    // Машина ещё ни разу не пушила — не считаем офлайн.
    assert!(pull_skip_reason(true, 2700, now, None, None, "abc").is_none());
}

#[test]
fn non_transient_errors_are_not_retried() {
    assert!(!is_transient_error("No route to host (os error 113)"));
    assert!(!is_transient_error("Connection refused"));
    assert!(!is_transient_error("Permission denied (publickey)"));
    assert!(is_transient_error("ssh handshake timed out"));
}

#[tokio::test(start_paused = true)]
async fn retry_stops_immediately_on_non_transient_error() {
    let mut calls = 0u32;
    let outcome = retry_pull(5, || {
        calls += 1;
        async move { Err("No route to host (os error 113)".to_string()) }
    })
    .await;
    assert!(!outcome.ok);
    assert_eq!(outcome.attempts, 1, "no retries for an unreachable host");
    assert_eq!(calls, 1);
}

#[test]
fn pull_command_clones_when_dir_missing_else_pulls() {
    let path = std::path::Path::new("/home/mflkee/projects/newproj");
    // Есть url — bootstrap-клон для машин без каталога.
    let cmd = build_pull_command(path, "main", "git@github.com:mflkee/mycelium.git", None);
    assert!(
        cmd.contains("if [ -d '/home/mflkee/projects/newproj/.git' ]; then"),
        "clone-if-missing guard expected, got: {cmd}"
    );
    assert!(
        cmd.contains(
            "git clone 'git@github.com:mflkee/mycelium.git' '/home/mflkee/projects/newproj'"
        ),
        "clone with url expected, got: {cmd}"
    );
    assert!(cmd.contains("git checkout 'main'"));

    // Путь/имя с пробелом и кавычками в пути — экранирование одинарными.
    let weird = std::path::Path::new("/tmp/a b/'quote'");
    let cmd = build_pull_command(weird, "main", "git@h:r.git", None);
    assert!(cmd.contains("'/tmp/a b/'\\''quote'\\''/.git'"));
}

#[test]
fn pull_command_pull_only_without_url_and_appends_post_pull() {
    let path = std::path::Path::new("/home/mflkee/projects/localonly");
    let cmd = build_pull_command(path, "dev", "", Some("bash ./deploy.sh"));
    assert!(cmd.starts_with(
        "cd '/home/mflkee/projects/localonly' && git pull --rebase --autostash origin 'dev'"
    ));
    assert!(cmd.ends_with("&& bash ./deploy.sh"));
    assert!(!cmd.contains("git clone"));
}
