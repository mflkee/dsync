use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use russh::client;

pub struct SshClient;

#[async_trait::async_trait]
impl client::Handler for SshClient {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub async fn exec(host: &str, port: u16, user: &str, cmd: &str) -> Result<String> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let config = Arc::new(client::Config::default());

    let mut session = client::connect(config, addr, SshClient).await?;

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let key_path = home.join(".ssh/id_ed25519");

    let key_pair = Arc::new(russh::keys::load_secret_key(&key_path, None)?);
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
}
