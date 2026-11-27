//! Read-only сводка state store для TUI/`--json` и последняя ошибка канала.

use super::index::{normalize_ts, StoredStateItem};
use super::*;

impl HubState {
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
        let state_dir = self.state_dir.clone().unwrap();
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
                let error = errors.get(&channel).cloned();
                let bytes = items
                    .iter()
                    .filter_map(|i| std::fs::metadata(state_dir.join(&i.file)).ok())
                    .map(|m| m.len())
                    .sum();
                crate::protocol::ChannelStatus {
                    channel,
                    item_count: items.len(),
                    bytes,
                    last_updated,
                    last_origin,
                    error,
                }
            })
            .collect();
        Ok(crate::protocol::StateStatusResponse { channels })
    }
}
