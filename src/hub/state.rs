use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::RwLock;

use crate::protocol::MachineState;

#[derive(Default)]
pub struct HubState {
    machines: RwLock<HashMap<String, MachineState>>,
    online: RwLock<HashMap<String, bool>>,
}

impl HubState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn update_machine(&self, state: MachineState) {
        let name = state.name.clone();
        let mut machines = self.machines.write().await;
        machines.insert(name, state);
    }

    pub async fn get_machine(&self, name: &str) -> Option<MachineState> {
        let machines = self.machines.read().await;
        machines.get(name).cloned()
    }

    pub async fn all_machines(&self) -> HashMap<String, MachineState> {
        let machines = self.machines.read().await;
        machines.clone()
    }

    pub async fn set_online(&self, name: &str, online: bool) {
        let mut online_map = self.online.write().await;
        online_map.insert(name.to_string(), online);
    }

    pub async fn is_online(&self, name: &str) -> bool {
        let online_map = self.online.read().await;
        online_map.get(name).copied().unwrap_or(false)
    }

    pub async fn status(&self) -> Result<crate::protocol::StatusResponse> {
        let machines = self.machines.read().await;
        let online_map = self.online.read().await;
        let mut resp = std::collections::HashMap::new();

        for (name, state) in machines.iter() {
            resp.insert(
                name.clone(),
                crate::protocol::MachineStatus {
                    online: online_map.get(name).copied().unwrap_or(false),
                    last_seen: state.last_push,
                    last_push: state.last_push,
                },
            );
        }

        Ok(crate::protocol::StatusResponse { machines: resp })
    }
}
