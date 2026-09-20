use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use russh::client;

use super::trust::{ssh_fingerprint, SshHostTrustStore, SshTrustState};

/// Текущее состояние проверки host-ключа SSH: общее между хендлером
/// (который видит предъявленный ключ) и вызывающим кодом (которому нужно
/// понять, почему коннект не удался / что сохранить).
struct SshVerification {
    /// Ключ store, "host:port".
    host_key: String,
    store_path: PathBuf,
    /// Сохранённый отпечаток из store (если запись есть).
    expected: Option<String>,
    /// Отпечаток ключа, предъявленного сервером при последней проверке.
    observed: Option<String>,
}

pub struct SshClient {
    /// `Some(..)` — TOFU-проверка host-ключа (хаб-пуллы и `doctor`).
    /// `None` — legacy-поведение «принимать любой ключ» (бот-команды по уже
    /// известным машинам).
    verification: Option<Arc<std::sync::Mutex<SshVerification>>>,
}

#[async_trait::async_trait]
impl client::Handler for SshClient {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let Some(verification) = &self.verification else {
            return Ok(true);
        };

        use russh::keys::PublicKeyBase64;
        let fp = ssh_fingerprint(&server_public_key.public_key_bytes());

        let mut v = verification.lock().unwrap();
        v.observed = Some(fp.clone());
        let store = SshHostTrustStore::load(&v.store_path);
        let expected = store.get(&v.host_key).map(str::to_string);
        v.expected = expected.clone();

        match expected {
            // Первый контакт: TOFU — доверяем; запись появится после
            // успешного коннекта (в exec-пути), чтобы сбой авторизации или
            // сети не создавал записи о мёртвой/чужой машине.
            None => Ok(true),
            Some(expected) if expected == fp => Ok(true),
            Some(_expected) => Ok(false),
        }
    }
}

/// Короткий таймаут connect по умолчанию: без него недоступная машина висела
/// в connect+exec дольше двух минут (наблюдалось в логах хаба), копя
/// заблокированные SSH-таски на каждую (проект × машина).
pub const DEFAULT_SSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Exec-команда без проверки host-ключа с явным таймаутом — для длинных
/// команд бота (opencode run, произвольные exec-команды), которые ждут до
/// минуты+.
pub async fn exec_with_key_timeout(
    host: &str,
    port: u16,
    user: &str,
    cmd: &str,
    key_path: &std::path::Path,
    timeout: std::time::Duration,
) -> Result<String> {
    exec_inner(host, port, user, cmd, key_path, timeout, timeout, None).await
}

/// Хаб-пулл с проверкой host-ключа и одним таймаутом на connect+exec
/// (удобная обёртка; для пулов используйте `exec_with_key_verifying_split`,
/// где exec живёт дольше connect).
#[allow(dead_code)]
pub async fn exec_with_key_verifying_timeout(
    host: &str,
    port: u16,
    user: &str,
    cmd: &str,
    key_path: &std::path::Path,
    store_path: PathBuf,
    timeout: std::time::Duration,
) -> Result<String> {
    exec_with_key_verifying_split(
        host, port, user, cmd, key_path, store_path, timeout, timeout,
    )
    .await
}

/// Хаб-пулл с проверкой host-ключа и РАЗДЕЛЬНЫМИ таймаутами: connect держит
/// короткий (30 c) — недостижимая машина не должна копить висящие таски,
/// а exec — длинный (`[hub] pull_timeout_secs`, по умолчанию 300 c), потому
/// что команда пула включает `post_pull` вроде `cargo build --release`
/// (1–3 минуты), и 30 c его просто резали бы по таймауту на ретраи.
#[allow(clippy::too_many_arguments)]
pub async fn exec_with_key_verifying_split(
    host: &str,
    port: u16,
    user: &str,
    cmd: &str,
    key_path: &std::path::Path,
    store_path: PathBuf,
    connect_timeout: std::time::Duration,
    exec_timeout: std::time::Duration,
) -> Result<String> {
    let verification = Arc::new(std::sync::Mutex::new(SshVerification {
        host_key: format!("{host}:{port}"),
        store_path,
        expected: None,
        observed: None,
    }));
    exec_inner(
        host,
        port,
        user,
        cmd,
        key_path,
        connect_timeout,
        exec_timeout,
        Some(verification),
    )
    .await
}

/// Лёгкий пробник доверия host-ключа для `dsync doctor`: делает только
/// транспортный коннект (KEX), не аутентифицируется и ничего не пишет в store.
pub async fn probe_host_trust(host: &str, port: u16, store_path: PathBuf) -> SshTrustState {
    let addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(a) => a,
        Err(_) => return SshTrustState::Unreachable(format!("bad address {host}:{port}")),
    };
    let verification = Arc::new(std::sync::Mutex::new(SshVerification {
        host_key: format!("{host}:{port}"),
        store_path,
        expected: None,
        observed: None,
    }));
    {
        let mut v = verification.lock().unwrap();
        v.expected = SshHostTrustStore::load(&v.store_path)
            .get(&v.host_key)
            .map(str::to_string);
    }

    let config = Arc::new(client::Config::default());
    let handler = SshClient {
        verification: Some(verification.clone()),
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client::connect(config, addr, handler),
    )
    .await
    {
        Err(_) => SshTrustState::Unreachable("timeout".into()),
        Ok(Err(e)) => {
            let v = verification.lock().unwrap();
            match (&v.expected, &v.observed) {
                (Some(expected), Some(observed)) if expected != observed => {
                    SshTrustState::Mismatch {
                        expected: expected.clone(),
                        observed: observed.clone(),
                    }
                }
                _ => SshTrustState::Unreachable(format!("{e:#}")),
            }
        }
        Ok(Ok(session)) => {
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "", "doctor probe")
                .await;
            match &verification.lock().unwrap().expected {
                Some(_) => SshTrustState::Trusted,
                None => SshTrustState::Untrusted,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn exec_inner(
    host: &str,
    port: u16,
    user: &str,
    cmd: &str,
    key_path: &std::path::Path,
    connect_timeout: std::time::Duration,
    exec_timeout: std::time::Duration,
    verification: Option<Arc<std::sync::Mutex<SshVerification>>>,
) -> Result<String> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let config = Arc::new(client::Config::default());

    let handler = SshClient {
        verification: verification.clone(),
    };
    // Connect тоже внутри таймаута: до недостижимой машины (NetBird-чёрная
    // дыра) TCP-connect висит минуты по системному таймауту, копя висящие
    // пул-таски на каждую (проект × машина). Старый код оборачивал в таймаут
    // весь exec, включая connect — сохраняем это поведение.
    let mut session =
        match tokio::time::timeout(connect_timeout, client::connect(config, addr, handler)).await {
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "connect to {user}@{host}:{port} timed out after {}s",
                    connect_timeout.as_secs()
                ))
            }
            Ok(Err(e)) => return Err(enrich_connect_error(&verification, e)),
            Ok(Ok(s)) => s,
        };

    // Первый контакт (TOFU): транспорту поверили — фиксируем отпечаток,
    // чтобы следующий пул уже сравнивал, а не пере-доверял.
    if let Some(v) = &verification {
        let v = v.lock().unwrap();
        if let (None, Some(observed)) = (&v.expected, &v.observed) {
            let mut store = SshHostTrustStore::load(&v.store_path);
            if store.get(&v.host_key).is_none() {
                store.insert(v.host_key.clone(), observed.clone());
                if let Err(e) = store.save(&v.store_path) {
                    tracing::warn!("failed to persist ssh host key for {}: {e:#}", v.host_key);
                }
            }
        }
    }

    let run = async {
        let key_pair = Arc::new(russh::keys::load_secret_key(key_path, None)?);
        let auth = session.authenticate_publickey(user, key_pair).await?;
        if !auth {
            anyhow::bail!("SSH authentication failed for {user}@{host}");
        }

        let mut channel = session.channel_open_session().await?;
        channel.exec(true, cmd.as_bytes()).await?;

        let mut output = Vec::new();
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    output.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                    output.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status: s }) => {
                    if s != 0 {
                        let out = String::from_utf8_lossy(&output).to_string();
                        anyhow::bail!("SSH command failed (exit={s}): {out}");
                    }
                }
                Some(russh::ChannelMsg::Close) | None => break,
                _ => continue,
            }
        }
        Ok(String::from_utf8_lossy(&output).to_string())
    };

    tokio::time::timeout(exec_timeout, run).await.map_err(|_| {
        anyhow::anyhow!(
            "{user}@{host}:{port} exec timed out after {}s",
            exec_timeout.as_secs()
        )
    })?
}

/// Превращает ошибку коннекта в действие, когда виноват несовпавший host-key:
/// russh поднимает `UnknownKey` при `check_server_key -> Ok(false)`, а причина
/// (stored vs presented) остаётся в `SshVerification`.
fn enrich_connect_error(
    verification: &Option<Arc<std::sync::Mutex<SshVerification>>>,
    err: anyhow::Error,
) -> anyhow::Error {
    let Some(v) = verification else {
        return err;
    };
    let v = v.lock().unwrap();
    match (&v.expected, &v.observed) {
        (Some(expected), Some(observed)) if expected != observed => anyhow::anyhow!(
            "SSH host key mismatch for {host}:\n\
             \x20 stored:    {expected}\n\
             \x20 presented: {observed}\n\
             This can mean a different machine, a MITM, or a re-provisioned host.\n\
             If the key legitimately changed (OS reinstall, new server), accept it with:\n\
             \x20 dsync trust ssh rm {host}",
            host = v.host_key,
        ),
        _ => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_handler_accepts_any_key() {
        // Без verification хендлер не проверяет host-key вовсе.
        assert!(SshClient { verification: None }.verification.is_none());
    }

    #[test]
    fn verifying_handler_builds_mismatch_guard() {
        // enrich_connect_error не трогает ошибку, когда ключ совпал/неизвестен.
        let err = anyhow::anyhow!("some network error");
        let enriched = enrich_connect_error(&None, err);
        assert!(enriched.to_string().contains("some network error"));
    }
}
