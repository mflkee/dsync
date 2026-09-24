use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::RwLock;

use crate::protocol::{MachineState, PullOutcome, StateItem};

/// Метаданные одного элемента состояния (сам payload — в отдельном файле).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct StoredStateItem {
    channel: String,
    key: String,
    updated: i64,
    origin: String,
    seq: i64,
    #[serde(default)]
    meta: Option<String>,
    /// Относительный путь (под `<data_dir>/state`) к файлу с payload.
    file: String,
}

/// Индекс состояния хаба: сквозной `seq` и метаданные всех элементов.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct StateIndex {
    #[serde(default)]
    seq: i64,
    #[serde(default)]
    items: Vec<StoredStateItem>,
}

/// Безопасное имя файла из ключа элемента.
fn sanitize_key(s: &str) -> String {
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

/// Приводит timestamp к секундам: opencode-канал пишет миллисекунды,
/// tmux — секунды. Значения > 1e12 — это однозначно ms (секунды сейчас
/// ~1.7e9, и ещё ~век до 1e12).
fn normalize_ts(v: i64) -> i64 {
    if v > 1_000_000_000_000 {
        v / 1000
    } else {
        v
    }
}

pub struct HubState {
    machines: RwLock<HashMap<String, MachineState>>,
    data_dir: Option<PathBuf>,
    /// Машины не подававшие признаков жизни дольше retention (0 = никогда).
    retention_days: u64,
    /// Сериализует одновременные `save()`: несколько пулл-тасок финишируют
    /// разом и писали в один tmp-файл (rename второго ловил ENOENT).
    save_lock: tokio::sync::Mutex<()>,
    /// Не-git состояние флота (tmux / opencode), проиндексированное по (channel, key).
    state_index: RwLock<StateIndex>,
    /// Каталог для payload-файлов состояния (`<data_dir>/state`).
    state_dir: Option<PathBuf>,
    /// Последняя ошибка синка по каналу (отдаётся в `state_status`).
    state_errors: RwLock<HashMap<String, String>>,
}

impl HubState {
    pub fn new(data_dir: Option<PathBuf>, retention_days: u64) -> Self {
        let data_dir = data_dir.map(|d| expand(&d));
        let machines = data_dir
            .as_ref()
            .and_then(|d| Self::load_machines(d).ok())
            .unwrap_or_default();
        let state_dir = data_dir.as_ref().map(|d| d.join("state"));
        let state_index = data_dir
            .as_ref()
            .and_then(|d| Self::load_state_index(d).ok())
            .unwrap_or_default();

        tracing::info!(
            "hub state: {} machines, {} state items loaded from disk",
            machines.len(),
            state_index.items.len()
        );

        Self {
            machines: RwLock::new(machines),
            data_dir,
            retention_days,
            save_lock: tokio::sync::Mutex::new(()),
            state_index: RwLock::new(state_index),
            state_dir,
            state_errors: RwLock::new(HashMap::new()),
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

    /// Склеивает присланные элементы состояния (last-write-wins по `updated`),
    /// присваивает сквозные `seq` и сохраняет индекс. Возвращает текущий seq.
    pub async fn merge_state(&self, items: Vec<StateItem>, origin: &str) -> i64 {
        if self.state_dir.is_none() {
            return 0;
        }
        let state_dir = self.state_dir.clone().unwrap();
        {
            let mut idx = self.state_index.write().await;
            for item in items {
                if item.key.is_empty() || item.channel.is_empty() {
                    continue;
                }
                // LWW: не откатываем более свежее чужое значение старым.
                if let Some(existing) = idx
                    .items
                    .iter()
                    .find(|i| i.channel == item.channel && i.key == item.key)
                {
                    if item.updated <= existing.updated && existing.origin != origin {
                        continue;
                    }
                }

                idx.seq += 1;
                let seq = idx.seq;
                let rel = format!(
                    "{}/{}",
                    sanitize_key(&item.channel),
                    sanitize_key(&item.key)
                );
                let path = state_dir.join(&rel);
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(e) = tokio::fs::write(&path, item.data.as_bytes()).await {
                    idx.seq -= 1;
                    self.record_state_error(&item.channel, format!("payload write failed: {e}"))
                        .await;
                    continue;
                }
                idx.items
                    .retain(|i| !(i.channel == item.channel && i.key == item.key));
                let updated = item.updated;
                let meta = item.meta;
                let channel = item.channel;
                let key = item.key;
                idx.items.push(StoredStateItem {
                    channel,
                    key,
                    updated,
                    origin: origin.to_string(),
                    seq,
                    meta,
                    file: rel,
                });
            }
        }
        self.save_state_index().await;
        self.state_index.read().await.seq
    }

    /// Элементы состояния с `seq > since`, записанные не этой машиной.
    /// Возвращает (элементы, текущий seq хаба).
    pub async fn state_since(&self, since: i64, exclude_origin: &str) -> (Vec<StateItem>, i64) {
        let (selected, cur): (Vec<StoredStateItem>, i64) = {
            let idx = self.state_index.read().await;
            (
                idx.items
                    .iter()
                    .filter(|i| i.seq > since && i.origin != exclude_origin)
                    .cloned()
                    .collect(),
                idx.seq,
            )
        };
        let Some(state_dir) = self.state_dir.clone() else {
            return (Vec::new(), cur);
        };
        let mut out = Vec::with_capacity(selected.len());
        for it in selected {
            let path = state_dir.join(&it.file);
            let Ok(data) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            out.push(StateItem {
                channel: it.channel,
                key: it.key,
                updated: it.updated,
                origin: it.origin,
                seq: it.seq,
                meta: it.meta,
                data,
            });
        }
        (out, cur)
    }

    /// Фиксирует последнюю ошибку синка по каналу (для вкладки State).
    pub async fn record_state_error(&self, channel: &str, err: String) {
        let mut errors = self.state_errors.write().await;
        errors.insert(channel.to_string(), err);
    }

    /// Сводка state store для вкладки State: по каналу — число элементов,
    /// время/автор последнего обновления, последняя ошибка (если была).
    pub async fn state_status(&self) -> Result<crate::protocol::StateStatusResponse> {
        if self.state_dir.is_none() {
            tracing::warn!("state_status requested but state store is not configured");
            return Ok(crate::protocol::StateStatusResponse {
                channels: Vec::new(),
            });
        }
        let idx = self.state_index.read().await;
        let errors = self.state_errors.read().await;
        let mut by_channel: std::collections::BTreeMap<String, Vec<&StoredStateItem>> =
            Default::default();
        for it in &idx.items {
            by_channel.entry(it.channel.clone()).or_default().push(it);
        }
        let channels = by_channel
            .into_iter()
            .map(|(channel, items)| {
                // Самый свежий элемент: по (updated, seq).
                let newest = items.iter().max_by_key(|i| (i.updated, i.seq));
                let (last_updated, last_origin) = match newest {
                    Some(n) => (normalize_ts(n.updated), n.origin.clone()),
                    None => (0, String::new()),
                };
                crate::protocol::ChannelStatus {
                    channel,
                    item_count: items.len(),
                    last_updated,
                    last_origin,
                    error: errors.get(&channel).cloned(),
                }
            })
            .collect();
        Ok(crate::protocol::StateStatusResponse { channels })
    }

    async fn save_state_index(&self) {
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

    fn load_state_index(dir: &Path) -> Result<StateIndex> {
        let path = dir.join("state/index.json");
        if !path.exists() {
            return Ok(StateIndex::default());
        }
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data).unwrap_or_default())
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
    fn concurrent_record_pull_saves_do_not_race() {
        // Несколько пулл-тасок финишируют разом: все record_pull сохраняют
        // в один machines.json, и без сериализации save()/уникального tmp
        // rename конкурентных сохранений ловил ENOENT.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("dsync-state-race-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let state = std::sync::Arc::new(HubState::new(Some(dir.clone()), 30));
            state.update_machine(machine("desktop", 100)).await;

            let mut tasks = Vec::new();
            for i in 0..10 {
                let state = state.clone();
                tasks.push(tokio::spawn(async move {
                    state
                        .record_pull("desktop", &format!("p{i}"), &PullOutcome::success(1))
                        .await;
                }));
            }
            for t in tasks {
                t.await.unwrap();
            }

            // Файл валиден и содержит все outcomes.
            let data = std::fs::read_to_string(dir.join("machines.json")).unwrap();
            let back: HashMap<String, MachineState> = serde_json::from_str(&data).unwrap();
            assert_eq!(back["desktop"].pulls.len(), 10);
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

    #[test]
    fn state_merge_and_since_roundtrip() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("dsync-st-items-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let state = HubState::new(Some(dir.clone()), 30);
            let items = vec![StateItem {
                channel: "opencode".into(),
                key: "ses_a".into(),
                updated: 100,
                data: r#"{"a":1}"#.into(),
                ..Default::default()
            }];
            assert_eq!(state.merge_state(items, "notebook").await, 1);

            // Другая машина видит элемент; автор — нет.
            let (got, cur) = state.state_since(0, "desktop").await;
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].key, "ses_a");
            assert_eq!(got[0].origin, "notebook");
            assert_eq!(got[0].data, r#"{"a":1}"#);
            assert_eq!(cur, 1);
            assert!(state.state_since(0, "notebook").await.0.is_empty());

            // Персистентность: перечитываем с диска.
            let state2 = HubState::new(Some(dir.clone()), 30);
            let (got2, cur2) = state2.state_since(0, "desktop").await;
            assert_eq!(got2.len(), 1);
            assert_eq!(cur2, 1);

            // LWW: устаревшее чужое обновление игнорируется.
            let stale = vec![StateItem {
                channel: "opencode".into(),
                key: "ses_a".into(),
                updated: 50,
                data: "OLD".into(),
                ..Default::default()
            }];
            assert_eq!(
                state2.merge_state(stale, "desktop").await,
                1,
                "stale ignored"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn state_status_summarizes_channels_with_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("dsync-st-status-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let state = HubState::new(Some(dir.clone()), 30);

            // tmux: 1 элемент, updated в секундах
            state
                .merge_state(
                    vec![StateItem {
                        channel: "tmux".into(),
                        key: "latest".into(),
                        updated: unix_now() - 120,
                        data: "{}".into(),
                        ..Default::default()
                    }],
                    "notebook",
                )
                .await;
            // opencode: 2 элемента, updated в мс
            state
                .merge_state(
                    vec![
                        StateItem {
                            channel: "opencode".into(),
                            key: "ses_a".into(),
                            updated: 1_776_000_000_000,
                            data: "{}".into(),
                            ..Default::default()
                        },
                        StateItem {
                            channel: "opencode".into(),
                            key: "ses_b".into(),
                            updated: 1_776_000_100_000,
                            data: "{}".into(),
                            ..Default::default()
                        },
                    ],
                    "desktop",
                )
                .await;
            state
                .record_state_error("opencode", "export truncated (CLI bug)")
                .await;

            let resp = state.state_status().await.unwrap();
            assert_eq!(resp.channels.len(), 2);

            let tmux = resp.channels.iter().find(|c| c.channel == "tmux").unwrap();
            assert_eq!(tmux.item_count, 1);
            assert_eq!(tmux.last_origin, "notebook");
            // seconds не делятся
            assert!(tmux.last_updated > 0 && tmux.last_updated < 1_000_000_000_000);
            assert!(tmux.error.is_none());

            let oc = resp.channels.iter().find(|c| c.channel == "opencode").unwrap();
            assert_eq!(oc.item_count, 2);
            assert_eq!(oc.last_origin, "desktop");
            // ms → s
            assert_eq!(oc.last_updated, 1_776_000_100);
            assert_eq!(oc.error.as_deref(), Some("export truncated (CLI bug)"));

            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
