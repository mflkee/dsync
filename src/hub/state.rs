use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use tokio::sync::RwLock;

use crate::protocol::MachineState;

fn expand(p: &PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p.clone()
}

const ONLINE_TIMEOUT_SECS: i64 = 35 * 60;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub struct HubState {
    machines: RwLock<HashMap<String, MachineState>>,
    data_dir: Option<PathBuf>,
}

impl HubState {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let data_dir = data_dir.map(|d| expand(&d));
        let machines = data_dir
            .as_ref()
            .and_then(|d| Self::load_machines(d).ok())
            .unwrap_or_default();

        tracing::info!(
            "hub state: {} machines loaded from disk",
            machines.len()
        );

        Self {
            machines: RwLock::new(machines),
            data_dir,
        }
    }

    pub async fn update_machine(&self, state: MachineState) {
        let name = state.name.clone();
        {
            let mut machines = self.machines.write().await;
            machines.insert(name, state);
        }
        self.save().await;
    }

    pub async fn all_machines(&self) -> HashMap<String, MachineState> {
        let machines = self.machines.read().await;
        machines.clone()
    }

    pub async fn status(&self) -> Result<crate::protocol::StatusResponse> {
        let machines = self.machines.read().await;
        let now = unix_now();
        let mut resp = std::collections::HashMap::new();

        for (name, state) in machines.iter() {
            resp.insert(
                name.clone(),
                crate::protocol::MachineStatus {
                    online: now - state.last_push <= ONLINE_TIMEOUT_SECS,
                    last_seen: state.last_push,
                    last_push: state.last_push,
                },
            );
        }

        Ok(crate::protocol::StatusResponse { machines: resp })
    }

    async fn save(&self) {
        let dir = match &self.data_dir {
            Some(d) => d.clone(),
            None => return,
        };
        let machines = self.machines.read().await;
        let data = match serde_json::to_string_pretty(&*machines) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("failed to serialize state: {e}");
                return;
            }
        };
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::error!("failed to create data dir {dir:?}: {e}");
            return;
        }
        if let Err(e) = tokio::fs::write(dir.join("machines.json"), data).await {
            tracing::error!("failed to save state: {e}");
        }
    }

    fn load_machines(dir: &PathBuf) -> Result<HashMap<String, MachineState>> {
        let path = dir.join("machines.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }
}
