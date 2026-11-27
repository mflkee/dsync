//! Синхронизация не-git состояния флота: tmux-раскладка и сессии opencode.
//!
//! Транспорт — хаб (`protocol::StateItem`, `hub::state`). Каналы:
//! * `tmux` — последний снапшот `tmux-resurrect` **этой** машины (один элемент,
//!   ключ = имя машины). Раскладки машин не смешиваются: чужие снапшоты не
//!   применяются, иначе живой tmux одной машины перезаписывался бы раскладкой
//!   другой (и `tmux-resurrect restore` уводил бы фокус в чужую сессию);
//! * `opencode` — экспортированные сессии (`opencode session export`), по
//!   элементу на сессию (ключ — id сессии, `meta` — каталог проекта).
//!
//! Локальный индекс `~/.local/share/dsync/state.json` хранит:
//! * `seq` — последний известный сквозной seq хаба (забираем только новее);
//! * `items` — `{ "channel:key": updated }` уже отправленных/применённых
//!   элементов, чтобы не гонять их повторно.
//!
//! Всё best-effort: отсутствие tmux/opencode или ошибка применения не должны
//! валить общий `dsync push`/`pull`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::{Config, OpencodeStateConfig};
use crate::infra::time::now_secs;
use crate::protocol::{StateItem, StatePullRequest, StatePushRequest};

pub(crate) const TMUX_CHANNEL: &str = "tmux";
pub(crate) const OPENCODE_CHANNEL: &str = "opencode";
/// Не приближаемся к hub `max_message_size` (8 MB по умолчанию).
pub(crate) const MAX_BATCH_BYTES: usize = 6 * 1024 * 1024;
/// Размер страницы при pull состояния: хаб отдаёт стор порциями.
pub(crate) const PAGE_BYTES: u64 = 6 * 1024 * 1024;
/// Предохранитель от бесконечной пагинации (баг/старый хаб).
pub(crate) const MAX_PAGES: usize = 64;
/// Таймаут одной state-операции: загрузка/выгрузка может быть многомегабайтной.
pub(crate) const STATE_TIMEOUT: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------------------
// Локальный индекс
// ---------------------------------------------------------------------------

mod apply;
mod index;
mod opencode;
mod tmux;

#[cfg(test)]
mod tests;

pub use apply::{apply, ApplyReport};
pub use index::{
    describe, get_seq, health, report, retry_dead, set_seq, DeadItem, Failure, StateHealth,
};
pub(crate) use index::{
    mark_synced, prune_unwanted_index, record_failures, set_seq_force, StateIndex,
};
pub use opencode::opencode_binary;
pub(crate) use opencode::refresh_opencode_index;
#[cfg(test)]
pub(crate) use tmux::content_hash;

/// Поведение одного канала состояния (tmux/opencode).
pub(crate) trait StateChannel {
    fn name(&self) -> &'static str;
    /// Включён ли канал в конфиге.
    fn wanted(&self, cfg: &Config) -> bool;
    /// Можно ли применять этот элемент на этой машине.
    fn accepts(&self, cfg: &Config, item: &StateItem) -> bool;
    fn collect(&self, cfg: &Config, idx: &StateIndex) -> Vec<StateItem>;
    fn apply(&self, cfg: &Config, item: &StateItem) -> Result<String>;
}

static TMUX: tmux::TmuxChannel = tmux::TmuxChannel;
static OPENCODE: opencode::OpencodeChannel = opencode::OpencodeChannel;

pub(crate) fn channels() -> [&'static dyn StateChannel; 2] {
    [&TMUX, &OPENCODE]
}

// ---------------------------------------------------------------------------
// Сбор локальных изменений
// ---------------------------------------------------------------------------

/// Собирает локальные элементы состояния, изменившиеся с прошлого раза.
pub fn collect(cfg: &Config) -> Vec<StateItem> {
    let idx = StateIndex::load();
    collect_with(&channels(), cfg, &idx)
}

/// Сбор через явный список каналов (для тестов оркестрации).
pub(crate) fn collect_with(
    channels: &[&dyn StateChannel],
    cfg: &Config,
    idx: &StateIndex,
) -> Vec<StateItem> {
    let mut out = Vec::new();
    for channel in channels {
        if channel.wanted(cfg) {
            out.extend(channel.collect(cfg, idx));
        }
    }
    out
}

/// Каналы, которые эта машина реально синхронизирует. Отправляется хабу,
/// чтобы он не присылал мегабайты чужих сессий машине с выключенным каналом.
pub(crate) fn wanted_channels(cfg: &Config) -> Vec<String> {
    channels()
        .into_iter()
        .filter(|c| c.wanted(cfg))
        .map(|c| c.name().to_string())
        .collect()
}

pub async fn sync_state(cfg: &Config, capture: bool) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.state.is_none() {
        return out;
    }

    // Сначала (возможно, долгий) сбор — экспорт сессий opencode занимает
    // десятки секунд. Делаем это ДО подключения: иначе QUIC-соединение
    // простаивает и рвётся по idle-таймауту.
    let changed = if capture { collect(cfg) } else { Vec::new() };
    if !changed.is_empty() {
        info!("state: {} item(s) to upload", changed.len());
    }

    let conn = match super::connect::connect_with_retry(cfg).await {
        Ok(c) => c,
        Err(e) => {
            warn!("state sync skipped (hub unavailable): {e:#}");
            return out;
        }
    };
    for batch in batches(changed) {
        let req = StatePushRequest {
            machine: cfg.machine.name.clone(),
            token: super::connect::hub_token(cfg),
            timestamp: now_secs(),
            items: batch.clone(),
        };
        match tokio::time::timeout(STATE_TIMEOUT, super::connect::send_state_push(&conn, &req))
            .await
        {
            Ok(Ok(resp)) if resp.ok => mark_synced(&batch),
            Ok(Ok(resp)) => warn!("state push rejected: {}", resp.error.unwrap_or_default()),
            Ok(Err(e)) => {
                warn!("state push failed: {e:#}");
                break;
            }
            Err(_) => {
                warn!("state push timed out (hub too old?)");
                break;
            }
        }
    }

    // 2. Загрузка чужих изменений — страницами, чтобы не тянуть весь стор
    //    (сейчас ~66 МБ opencode-сессий) одним сообщением.
    let retry_limit = cfg
        .state
        .as_ref()
        .map(|s| s.retry_limit)
        .unwrap_or_else(crate::config::default_state_retry_limit)
        .max(1);
    let mut since = get_seq();
    let channels = wanted_channels(cfg);
    prune_unwanted_index(&channels);
    for _page in 0..MAX_PAGES {
        let req = StatePullRequest {
            machine: cfg.machine.name.clone(),
            token: super::connect::hub_token(cfg),
            state_seq: since,
            max_bytes: PAGE_BYTES,
            // Всегда явный список (возможно пустой): машина с полностью
            // выключенным состоянием не должна получать чужие каналы.
            channels: Some(channels.clone()),
        };
        let resp =
            match tokio::time::timeout(STATE_TIMEOUT, super::connect::send_state_pull(&conn, &req))
                .await
            {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    warn!("state pull failed: {e:#}");
                    break;
                }
                Err(_) => {
                    warn!("state pull timed out (hub too old?)");
                    break;
                }
            };

        let page_len = resp.items.len();
        let report = apply(cfg, resp.items);
        out.extend(report.messages);

        // Учёт неудач: после `retry_limit` попыток элемент уходит в dead-letter
        // и перестаёт блокировать канал (иначе один сломанный импорт заставлял
        // перекачивать и переприменять всё состояние каждый цикл — навсегда).
        let mut idx = StateIndex::load();
        let blocked = record_failures(&mut idx, &report.applied, &report.failed, retry_limit);
        if let Err(e) = idx.save() {
            warn!("can't save state index: {e:#}");
        }

        match blocked {
            // Есть сбойные элементы с остатком попыток: держим курсор перед
            // самым ранним из них и повторим на следующем цикле. Успешные
            // элементы повторно не применяются (см. `apply`).
            Some(min_seq) => {
                set_seq_force(min_seq - 1);
                break;
            }
            None => {
                // Старый хаб не умеет страницы (next_seq = 0) — он отдал всё,
                // дальше идти некуда.
                let next = if resp.next_seq > 0 {
                    resp.next_seq
                } else {
                    resp.state_seq
                };
                set_seq(next);
                since = next;
                if !resp.has_more || resp.next_seq <= 0 {
                    break;
                }
            }
        }
        if page_len == 0 && !resp.has_more {
            break;
        }
    }

    super::connect::close_conn(&conn);
    out
}

/// Разбивает элементы на батчи, не превышающие `MAX_BATCH_BYTES`.
fn batches(items: Vec<StateItem>) -> Vec<Vec<StateItem>> {
    let mut out = Vec::new();
    let mut cur: Vec<StateItem> = Vec::new();
    let mut bytes = 0usize;
    for it in items {
        let sz = it.data.len()
            + it.key.len()
            + it.channel.len()
            + it.meta.as_deref().map(str::len).unwrap_or(0)
            + 128;
        if bytes + sz > MAX_BATCH_BYTES && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        bytes += sz;
        cur.push(it);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Безопасное имя файла из произвольного ключа/метаданных.
fn sanitize_filename(s: &str) -> String {
    let base = s.rsplit(['/', '\\']).next().unwrap_or(s);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "state".to_string()
    } else {
        cleaned
    }
}
