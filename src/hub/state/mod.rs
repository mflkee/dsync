//! Состояние хаба: машины, индекс non-git состояния и развёрнутые коммиты.
//!
//! Границы: `index` — единственный писатель `state/index.json` и
//! `deployed.json`; `payload` — единственный, кто трогает файлы payload'ов;
//! `machines` — `machines.json`; `status` — read-only сводки.

mod index;
mod machines;
mod payload;
mod status;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::RwLock;

use crate::infra::time::now_secs;
use crate::protocol::{MachineState, PullOutcome, StateItem};

use index::StateIndex;
pub use payload::cleanup_stale_temp_files;

fn expand(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = crate::infra::paths::home_dir() {
            return home.join(rest);
        }
    }
    p.to_path_buf()
}

const ONLINE_TIMEOUT_SECS: i64 = 35 * 60;

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
    /// Последний успешно развёрнутый commit по ключу `machine/project`.
    /// Нужен, чтобы не гонять SSH-пулл (и `post_pull`) на каждый push, когда
    /// на машине-источнике ничего не изменилось.
    deployed: RwLock<HashMap<String, String>>,
    /// Активные on-demand deploy'и (`machine/project`) — защита от дублей.
    deploying: tokio::sync::Mutex<std::collections::HashSet<String>>,
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
        let deployed = data_dir
            .as_ref()
            .and_then(|d| Self::load_deployed(d).ok())
            .unwrap_or_default();

        tracing::info!(
            "hub state: {} machines, {} state items, {} deployed commits loaded from disk",
            machines.len(),
            state_index.items.len(),
            deployed.len()
        );

        Self {
            machines: RwLock::new(machines),
            data_dir,
            retention_days,
            save_lock: tokio::sync::Mutex::new(()),
            state_index: RwLock::new(state_index),
            state_dir,
            state_errors: RwLock::new(HashMap::new()),
            deployed: RwLock::new(deployed),
            deploying: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }
}
