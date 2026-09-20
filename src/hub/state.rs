use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::RwLock;

use crate::protocol::{MachineState, PullOutcome};

fn expand(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p.to_path_buf()
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
    /// Machines not seen for this many days are pruned (0 = never prune).
    retention_days: u64,
}

impl HubState {
    pub fn new(data_dir: Option<PathBuf>, retention_days: u64) -> Self {
        let data_dir = data_dir.map(|d| expand(&d));
        let machines = data_dir
            .as_ref()
            .and_then(|d| Self::load_machines(d).ok())
            .unwrap_or_default();

        tracing::info!("hub state: {} machines loaded from disk", machines.len());

        Self {
            machines: RwLock::new(machines),
            data_dir,
            retention_days,
        }
    }

    /// Удаляет машины, не подававшие признаков жизни дольше retention.
    /// Машины с `last_push == 0` (только добавленные) не трогаем.
    /// Возвращает true, если что-то удалили (и сохранили).
    pub async fn prune_stale(&self) -> bool {
        let window_days = self.retention_days;
        if window_days == 0 {
            return false;
        }
        let window = (window_days as i64).saturating_mul(86400);
        let now = unix_now();
        let mut removed = false;
        {
            let mut machines = self.machines.write().await;
            machines.retain(|_, m| {
                let keep = m.last_push == 0 || now.saturating_sub(m.last_push) <= window;
                if !keep {
                    removed = true;
                }
                keep
            });
        }
        if removed {
            self.save().await;
        }
        removed
    }

    pub async fn update_machine(&self, state: MachineState) {
        let name = state.name.clone();
        {
            let mut machines = self.machines.write().await;
            // Сохраняем прошлые pull-outcomes: update_machine приходит с
            // каждого push-цикла и не должен затирать историю пуллов флота.
            let existing_pulls = machines
                .get(&name)
                .map(|m| m.pulls.clone())
                .unwrap_or_default();
            let mut merged = state;
            merged.pulls = existing_pulls;
            machines.insert(name, merged);
        }
        self.save().await;
    }

    /// Записывает исход SSH-пулла по (машина, проект) и сохраняет состояние.
    pub async fn record_pull(&self, machine: &str, project: &str, outcome: &PullOutcome) {
        {
            let mut machines = self.machines.write().await;
            if let Some(m) = machines.get_mut(machine) {
                m.pulls.insert(project.to_string(), outcome.clone());
            }
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
                    pulls: state.pulls.clone(),
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
        // Атомарная запись: tmp + rename, чтобы краш между truncate и write
        // не оставил битый machines.json (тогда load молча вернул бы пустой
        // список машин и вся история синка пропала бы).
        let tmp = dir.join("machines.json.tmp");
        let final_path = dir.join("machines.json");
        if let Err(e) = tokio::fs::write(&tmp, data).await {
            tracing::error!("failed to save state: {e}");
            return;
        }
        if let Err(e) = tokio::fs::rename(&tmp, &final_path).await {
            tracing::error!("failed to rename state file: {e}");
        }
    }

    fn load_machines(dir: &Path) -> Result<HashMap<String, MachineState>> {
        let path = dir.join("machines.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<HashMap<String, MachineState>>(&data) {
            Ok(map) => Ok(map),
            Err(_) => {
                // Старый формат файла (до hub-auth): плоский массив машин —
                // грузим и перекладываем в карту, чтобы старые файлы не терялись.
                let list = serde_json::from_str::<Vec<MachineState>>(&data)?;
                Ok(list.into_iter().map(|m| (m.name.clone(), m)).collect())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ProjectState, PullOutcome};

    fn machine(name: &str, last_push: i64) -> MachineState {
        MachineState {
            name: name.to_string(),
            last_push,
            projects: Vec::<ProjectState>::new(),
            pulls: HashMap::new(),
        }
    }

    #[test]
    fn update_machine_preserves_pulls() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let state = HubState::new(None, 30);
            state.update_machine(machine("desktop", 100)).await;
            state
                .record_pull(
                    "desktop",
                    "dotfiles",
                    &PullOutcome::failure("ssh failed".into(), 1),
                )
                .await;
            state.update_machine(machine("desktop", 200)).await;
            let m = state.all_machines().await;
            let pulls = &m["desktop"].pulls;
            assert_eq!(pulls.len(), 1, "pull outcome survives push cycle");
            assert_eq!(pulls["dotfiles"].error.as_deref(), Some("ssh failed"));
        });
    }

    #[test]
    fn record_pull_roundtrip_with_persistence() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("dsync-state-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let state = HubState::new(Some(dir.clone()), 30);
            state.update_machine(machine("desktop", 100)).await;
            state
                .record_pull("desktop", "dotfiles", &PullOutcome::success(1))
                .await;

            // Пересоздаём из того же data_dir — pulls должны прочитаться.
            let state2 = HubState::new(Some(dir.clone()), 30);
            let m = state2.all_machines().await;
            assert!(m["desktop"].pulls["dotfiles"].ok);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn old_machines_json_without_pulls_loads() {
        let dir = std::env::temp_dir().join(format!("dsync-state-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("machines.json"),
            r#"[{"name":"desktop","last_push":1,"projects":[]}]"#,
        )
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let state = HubState::new(Some(dir.clone()), 30);
            let m = state.all_machines().await;
            assert!(m["desktop"].pulls.is_empty());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_removes_only_stale_and_keeps_new() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let state = HubState::new(None, 30); // 30 дней
            let now = unix_now();
            state.update_machine(machine("recent", now - 10)).await;
            state
                .update_machine(machine("stale", now - (31 * 86400)))
                .await;
            state.update_machine(machine("fresh", 0)).await; // last_push == 0
            let removed = state.prune_stale().await;
            assert!(removed);
            let m = state.all_machines().await;
            assert!(m.contains_key("recent"));
            assert!(!m.contains_key("stale"));
            assert!(m.contains_key("fresh"), "last_push==0 never pruned");
        });
    }

    #[test]
    fn custom_retention_window_honored() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let state = HubState::new(None, 1); // 1 день
            let now = unix_now();
            state
                .update_machine(machine("old", now - (2 * 86400)))
                .await;
            state.update_machine(machine("ok", now - 3600)).await;
            state.prune_stale().await;
            let m = state.all_machines().await;
            assert!(!m.contains_key("old"));
            assert!(m.contains_key("ok"));
        });
    }

    #[test]
    fn zero_retention_disables_pruning() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let state = HubState::new(None, 0);
            let now = unix_now();
            state
                .update_machine(machine("ancient", now - (365 * 86400)))
                .await;
            assert!(!state.prune_stale().await);
            assert!(state.all_machines().await.contains_key("ancient"));
        });
    }
}
