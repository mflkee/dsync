use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use russh::client;
use russh_keys::load_secret_key;

pub struct SshClient;

impl client::Handler for SshClient {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
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

    let key_pair = load_secret_key(&key_path.to_string_lossy(), None)?;
    let auth = session.authenticate(user, key_pair).await?;

    if !auth {
        anyhow::bail!("SSH authentication failed for {user}@{host}");
    }

    let mut channel = session.channel_open_session().await?;
    channel.exec(true, cmd.as_bytes()).await?;

    let stdout = channel.wait().await?;
    let output = String::from_utf8_lossy(&stdout).to_string();

    Ok(output)
}
