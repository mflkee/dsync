use anyhow::Result;

/// SSH client for remote execution on spoke machines.
/// Hub uses this to `git pull` on machines that received a push.
pub struct SshClient {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl SshClient {
    pub fn new(host: String, port: u16, user: String) -> Self {
        Self { host, port, user }
    }

    /// Run a command on the remote machine.
    pub async fn exec(&self, _cmd: &str) -> Result<String> {
        // TODO: использовать russh для SSH exec
        Ok(String::new())
    }
}
