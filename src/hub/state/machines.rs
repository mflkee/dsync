//! Машины флота: online-статус, история pull'ов и персистентность
//! `machines.json`.

use super::*;

impl HubState {
    /// Удаляет машины, не подававшие признаков жизни дольше retention.
    /// Машины с `last_push == 0` (только добавленные) не трогаем.
    /// Возвращает true, если что-то удалили (и сохранили).
    pub async fn prune_stale(&self) -> bool {
        let window_days = self.retention_days;
        if window_days == 0 {
            return false;
        }
        let window = (window_days as i64).saturating_mul(86400);
        let now = now_secs();
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

    /// Время последнего push машины (секунды эпохи); None — машина неизвестна.
    pub async fn last_push(&self, machine: &str) -> Option<i64> {
        self.machines.read().await.get(machine).map(|m| m.last_push)
    }

    pub async fn status(&self) -> Result<crate::protocol::StatusResponse> {
        let machines = self.machines.read().await;
        let now = now_secs();
        let mut resp = std::collections::HashMap::new();

        for (name, state) in machines.iter() {
            resp.insert(
                name.clone(),
                crate::protocol::MachineStatus {
                    online: now - state.last_push <= ONLINE_TIMEOUT_SECS,
                    last_seen: state.last_push,
                    last_push: state.last_push,
                    pulls: state.pulls.clone(),
                    version: state.version.clone(),
                },
            );
        }

        Ok(crate::protocol::StatusResponse {
            machines: resp,
            hub_version: None,
        })
    }

    pub(super) async fn save(&self) {
        let _guard = self.save_lock.lock().await;
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
        // список машин и вся история синка пропала бы). Имя tmp уникально —
        // на случай сохранений, не покрытых мьютексом (save() может вызваться
        // извне с самостоятельной сериализацией данных).
        let tmp = dir.join(format!(
            "machines.json.tmp-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let final_path = dir.join("machines.json");
        if let Err(e) = tokio::fs::write(&tmp, data).await {
            tracing::error!("failed to save state: {e}");
            return;
        }
        if let Err(e) = tokio::fs::rename(&tmp, &final_path).await {
            tracing::error!("failed to rename state file: {e}");
        }
    }

    pub(super) fn load_machines(dir: &Path) -> Result<HashMap<String, MachineState>> {
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
