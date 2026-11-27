//! Персистентность индекса состояния и развёрнутых коммитов.
//!
//! Только этот модуль пишет `state/index.json` и `deployed.json`.

use super::*;

/// Метаданные одного элемента состояния (сам payload — в отдельном файле).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct StoredStateItem {
    pub(super) channel: String,
    pub(super) key: String,
    pub(super) updated: i64,
    pub(super) origin: String,
    pub(super) seq: i64,
    #[serde(default)]
    pub(super) meta: Option<String>,
    /// Относительный путь (под `<data_dir>/state`) к файлу с payload.
    pub(super) file: String,
}

/// Индекс состояния хаба: сквозной `seq` и метаданные всех элементов.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct StateIndex {
    #[serde(default)]
    pub(super) seq: i64,
    #[serde(default)]
    pub(super) items: Vec<StoredStateItem>,
}

/// Безопасное имя файла из ключа элемента.
pub(super) fn sanitize_key(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn normalize_ts(v: i64) -> i64 {
    if v > 1_000_000_000_000 {
        v / 1000
    } else {
        v
    }
}

pub(super) fn deployed_key(machine: &str, project: &str) -> String {
    format!("{machine}/{project}")
}

impl HubState {
    /// Последний успешно развёрнутый commit по (машина, проект).
    pub async fn deployed_commit(&self, machine: &str, project: &str) -> Option<String> {
        let map = self.deployed.read().await;
        map.get(&deployed_key(machine, project)).cloned()
    }

    /// Запоминает commit, развёрнутый на машине (после успешного пулла).
    pub async fn record_deployed(&self, machine: &str, project: &str, commit: &str) {
        if commit.is_empty() {
            return;
        }
        {
            let mut map = self.deployed.write().await;
            let key = deployed_key(machine, project);
            if map.get(&key).map(String::as_str) == Some(commit) {
                return;
            }
            map.insert(key, commit.to_string());
        }
        self.save_deployed().await;
    }

    pub(super) async fn save_state_index(&self) {
        let _guard = self.save_lock.lock().await;
        let Some(dir) = &self.data_dir else {
            return;
        };
        let idx = self.state_index.read().await;
        let data = match serde_json::to_string_pretty(&*idx) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("failed to serialize state index: {e}");
                return;
            }
        };
        let state_dir = dir.join("state");
        if let Err(e) = tokio::fs::create_dir_all(&state_dir).await {
            tracing::error!("failed to create state dir {state_dir:?}: {e}");
            return;
        }
        let tmp = state_dir.join(format!(
            "index.json.tmp-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        if let Err(e) = tokio::fs::write(&tmp, data).await {
            tracing::error!("failed to save state index: {e}");
            return;
        }
        if let Err(e) = tokio::fs::rename(&tmp, state_dir.join("index.json")).await {
            tracing::error!("failed to rename state index: {e}");
        }
    }

    pub(super) fn load_state_index(dir: &Path) -> Result<StateIndex> {
        let path = dir.join("state/index.json");
        if !path.exists() {
            return Ok(StateIndex::default());
        }
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data).unwrap_or_default())
    }

    pub(super) async fn save_deployed(&self) {
        let _guard = self.save_lock.lock().await;
        let Some(dir) = &self.data_dir else {
            return;
        };
        let map = self.deployed.read().await;
        let Ok(data) = serde_json::to_string_pretty(&*map) else {
            tracing::error!("failed to serialize deployed commits");
            return;
        };
        drop(map);
        if let Err(e) = tokio::fs::create_dir_all(dir).await {
            tracing::error!("failed to create data dir {dir:?}: {e}");
            return;
        }
        if let Err(e) = tokio::fs::write(dir.join("deployed.json"), data).await {
            tracing::error!("failed to save deployed commits: {e}");
        }
    }

    pub(super) fn load_deployed(dir: &Path) -> Result<HashMap<String, String>> {
        let path = dir.join("deployed.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data).unwrap_or_default())
    }
}
