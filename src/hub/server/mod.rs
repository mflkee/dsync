//! QUIC-сервер хаба: приём соединений, диспетчеризация запросов и SSH-пуллы.
//!
//! Границы: `serve` — accept-loop и auth, `dispatch` — обработчики запросов,
//! `pull` — планировщик/исполнитель SSH-пуллов.

mod dispatch;
mod pull;
mod serve;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use dispatch::handle_deploy;
#[cfg(test)]
pub(crate) use pull::{
    build_pull_command, is_transient_error, pull_skip_reason, retry_pull, should_pull_project,
};
use serve::serve_loop;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quinn::{Endpoint, Incoming, ServerConfig};
use tokio::io::AsyncReadExt;
use tokio::signal;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::config::{Config, SecretTokens};
use crate::infra::time::now_secs;
use crate::protocol::{
    error_envelope, DeployRequest, DeployResponse, DeployResult, PullOutcome, PullRequest,
    PullResponse, PushRequest, PushResponse, StatePullRequest, StatePullResponse, StatePushRequest,
    StatePushResponse, StateStatusRequest,
};

use super::error::HubError;
use super::state::HubState;
use super::token_matches;

/// Итоговая карта токенов хаба: конфиг главнее, sidecar-файл — fallback.
/// Пустая карта означает fail-secure (хаб не стартует).
fn effective_hub_tokens(
    cfg_tokens: &std::collections::HashMap<String, String>,
    secret: SecretTokens,
) -> std::collections::HashMap<String, String> {
    if !cfg_tokens.is_empty() {
        cfg_tokens.clone()
    } else {
        secret.hub.map(|h| h.tokens).unwrap_or_default()
    }
}

pub async fn run_server(cfg: Config) -> Result<()> {
    // Fail-secure: без токенов хаб не стартует (см. hub-auth). Источник —
    // `[hub] tokens` из конфига, иначе — sidecar `tokens.toml` (главный конфиг
    // управляется chezmoi и перегенерируется при `chezmoi apply`, поэтому
    // секреты в нём не живут; см. config::read_secret_tokens).
    let (cfg_tokens, bind): (std::collections::HashMap<String, String>, SocketAddr) = {
        let hub_cfg = cfg
            .hub
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no [hub] section in config — can't run daemon"))?;
        (hub_cfg.tokens.clone(), hub_cfg.bind.parse()?)
    };
    let hub_tokens = effective_hub_tokens(&cfg_tokens, crate::config::read_secret_tokens());
    if hub_tokens.is_empty() {
        anyhow::bail!(
            "hub refuses to start: [hub] tokens is missing/empty.\n\
             Configure per-machine tokens: rerun `dsync init` with the hub role, set\n\
             [hub] tokens in the config, or create the sidecar file:\n\n\
             ~/.config/dsync/dsync/tokens.toml\n\
             [hub]\n\
             tokens = {{ \"<machine>\": \"<token>\" }}\n\n\
             and put the matching [hub_connect] token on each client."
        );
    }
    // Резолвим токены один раз и кладём обратно в конфиг: проверка запросов
    // (authorized) читает cfg.hub.tokens, так что быть должна уже резолвленная
    // карта, а не только конфиговая.
    let mut cfg = cfg;
    cfg.hub.as_mut().unwrap().tokens = hub_tokens;
    let hub_cfg = cfg.hub.as_ref().unwrap();

    info!(
        "starting dsync hub {} on {bind}",
        crate::protocol::version_string()
    );

    let data_dir = cfg.hub_data_dir();

    let (cert, key) = load_or_generate_certs(&cfg, &data_dir)?;
    let server_config = make_server_config(cert, key)?;
    let endpoint = Endpoint::server(server_config, bind)?;

    let state = Arc::new(HubState::new(
        Some(data_dir.clone()),
        hub_cfg.retention_days,
    ));
    if state.prune_stale().await {
        info!("pruned stale machines at startup");
    }
    // Гигиена: осиротевшие tmp-файлы атомарных записей (`tmp` + `rename`)
    // остаются, если процесс убили между create и rename. Наблюдались сотни
    // таких файлов в каталоге хаба — чистим всё старше часа.
    let removed_tmp =
        crate::hub::state::cleanup_stale_temp_files(&data_dir, Duration::from_secs(3600));
    if removed_tmp > 0 {
        info!(
            "removed {removed_tmp} stale tmp file(s) from {}",
            data_dir.display()
        );
    }
    // Мусор от каналов, которых больше нет в коде/конфиге (например `zellij`,
    // поддержка которого удалена): применяться всё равно некому.
    if let Some(st) = cfg.state.as_ref() {
        let mut keep = std::collections::HashSet::new();
        if st.tmux {
            keep.insert("tmux".to_string());
        }
        if st.opencode.is_some() {
            keep.insert("opencode".to_string());
        }
        if !keep.is_empty() {
            let pruned = state.prune_state_channels(&keep).await;
            if pruned > 0 {
                warn!("pruned {pruned} state item(s) from unknown channels");
            }
        }
        // Общий tmux-элемент (`latest`) после перехода на ключ «имя машины»
        // никому не нужен, а его payload — чужая раскладка.
        let legacy = state.prune_legacy_tmux().await;
        if legacy > 0 {
            warn!("pruned {legacy} legacy tmux item(s) keyed 'latest'");
        }
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

fn make_server_config(
    cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    priv_key: rustls::pki_types::PrivateKeyDer<'static>,
) -> std::result::Result<ServerConfig, HubError> {
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, priv_key)?,
    )
    .map_err(|e| HubError::Config(format!("quic tls: {e}")))?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config.transport = Arc::new(quinn::TransportConfig::default());
    Ok(config)
}

fn load_or_generate_certs(
    cfg: &Config,
    data_dir: &std::path::Path,
) -> std::result::Result<
    (
        Vec<rustls::pki_types::CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    ),
    HubError,
> {
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
        return Err(HubError::Config(format!(
            "server_key.pem in {data_dir:?} contains no private key"
        )));
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
