//! Payload-файлы состояния: запись при merge, чтение страниц, удаление
//! мусора и осиротевших tmp-файлов.

use super::index::{sanitize_key, StoredStateItem};
use super::*;

/// Удаляет осиротевшие `*.tmp-*` файлы старше `max_age` в каталоге хаба и в
/// `state/`. Атомарная запись (`tmp + rename`) оставляет мусор, если процесс
/// убили между create и rename; без уборки накапливаются сотни файлов.
pub fn cleanup_stale_temp_files(dir: &Path, max_age: std::time::Duration) -> usize {
    let mut removed = 0usize;
    for d in [dir.to_path_buf(), dir.join("state")] {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.contains(".tmp-") {
                continue;
            }
            let stale = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.elapsed().ok())
                .map(|age| age >= max_age)
                .unwrap_or(false);
            if stale && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// LWW-решение без файловой системы: принимаем элемент, если он свежее
/// существующего или пришёл от того же origin (его собственные обновления не
/// блокируются численно меньшим `updated`).
fn accept_lww(existing: Option<&StoredStateItem>, item: &StateItem, origin: &str) -> bool {
    match existing {
        Some(e) => item.updated > e.updated || e.origin == origin,
        None => true,
    }
}

impl HubState {
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
                // Совместимость: клиенты до 21.11.2026 пишут общий tmux-элемент
                // `latest`, смешивая раскладки всех машин. Не принимаем его —
                // актуальные клиенты присылают `tmux:<имя машины>`.
                if item.channel == "tmux" && item.key == "latest" {
                    tracing::debug!("ignoring legacy tmux item keyed 'latest' from {origin}");
                    continue;
                }
                // LWW: не откатываем более свежее чужое значение старым.
                let existing = idx
                    .items
                    .iter()
                    .find(|i| i.channel == item.channel && i.key == item.key);
                if !accept_lww(existing, &item, origin) {
                    continue;
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
    /// Возвращает (элементы, текущий seq хаба). Без ограничения размера —
    /// используется тестами.
    #[cfg(test)]
    pub async fn state_since(&self, since: i64, exclude_origin: &str) -> (Vec<StateItem>, i64) {
        let (items, cur, _, _) = self.state_since_page(since, exclude_origin, 0, None).await;
        (items, cur)
    }

    /// Страница состояния: не больше `max_bytes` payload'ов (0 = без лимита).
    ///
    /// Возвращает `(элементы, текущий seq хаба, курсор следующей страницы,
    /// остались ли элементы)`. Элементы, чей `origin` — сама запрашивающая
    /// машина, не отправляются, но курсор через них продвигается (иначе клиент
    /// навсегда застрял бы на собственных записях). Если один элемент больше
    /// лимита, он отдаётся отдельной страницей — прогресс важнее размера.
    pub async fn state_since_page(
        &self,
        since: i64,
        exclude_origin: &str,
        max_bytes: u64,
        channels: Option<&[String]>,
    ) -> (Vec<StateItem>, i64, i64, bool) {
        let (selected, cur): (Vec<StoredStateItem>, i64) = {
            let idx = self.state_index.read().await;
            let mut selected: Vec<StoredStateItem> = idx
                .items
                .iter()
                .filter(|i| i.seq > since)
                .cloned()
                .collect();
            // Индекс не гарантирует порядок (retain + push), а курсор страницы
            // обязан быть монотонным — иначе часть элементов потеряется.
            selected.sort_by_key(|i| i.seq);
            (selected, idx.seq)
        };
        let Some(state_dir) = self.state_dir.clone() else {
            return (Vec::new(), cur, cur, false);
        };
        let mut out = Vec::with_capacity(selected.len().min(64));
        let mut used: u64 = 0;
        let mut cursor = since;
        let mut has_more = false;
        for it in selected {
            // Чужие origin и каналы, которые машине не нужны, не отправляем,
            // но курсор через них двигаем — иначе пагинация застрянет на них.
            if it.origin == exclude_origin {
                // Исключение — tmux-снапшот самой машины (ключ = имя машины):
                // это её бэкап раскладки, нужный для восстановления после
                // переустановки/потери локального state.json. Клиент применит
                // его только если локального индекса по нему ещё нет.
                let own_tmux = it.channel == "tmux" && it.key == exclude_origin;
                if !own_tmux {
                    cursor = it.seq;
                    continue;
                }
            }
            if let Some(wanted) = channels {
                if !wanted.iter().any(|c| c == &it.channel) {
                    cursor = it.seq;
                    continue;
                }
            }
            let path = state_dir.join(&it.file);
            let size = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if max_bytes > 0 && !out.is_empty() && used + size > max_bytes {
                has_more = true;
                break;
            }
            let Ok(data) = tokio::fs::read_to_string(&path).await else {
                // Payload пропал (ручная чистка) — не блокируем курсор.
                cursor = it.seq;
                continue;
            };
            used += size.max(data.len() as u64);
            cursor = it.seq;
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
        (out, cur, cursor, has_more)
    }

    /// Удаляет элементы каналов, которых нет в `keep` (мусор вроде `zellij`,
    /// поддержка которого убрана из кода). Возвращает число удалённых элементов.
    pub async fn prune_state_channels(&self, keep: &std::collections::HashSet<String>) -> usize {
        self.drop_state_items(|i| !keep.contains(&i.channel)).await
    }

    /// Удаляет устаревший общий tmux-элемент (`key = "latest"`). До 21.11.2026
    /// все машины писали раскладку в один элемент, из-за чего снапшоты
    /// смешивались; теперь ключ — имя машины, а `latest` применять некому.
    pub async fn prune_legacy_tmux(&self) -> usize {
        self.drop_state_items(|i| i.channel == "tmux" && i.key == "latest")
            .await
    }

    async fn drop_state_items<F>(&self, should_drop: F) -> usize
    where
        F: Fn(&StoredStateItem) -> bool,
    {
        let Some(state_dir) = self.state_dir.clone() else {
            return 0;
        };
        let dropped: Vec<StoredStateItem> = {
            let mut idx = self.state_index.write().await;
            let dropped: Vec<StoredStateItem> = idx
                .items
                .iter()
                .filter(|i| should_drop(i))
                .cloned()
                .collect();
            if dropped.is_empty() {
                return 0;
            }
            idx.items.retain(|i| !should_drop(i));
            dropped
        };
        let mut dirs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for it in &dropped {
            let _ = tokio::fs::remove_file(state_dir.join(&it.file)).await;
            dirs.insert(sanitize_key(&it.channel));
        }
        for d in dirs {
            // Каталог пуст только если в канале больше ничего не осталось.
            let _ = tokio::fs::remove_dir(state_dir.join(d)).await;
        }
        self.save_state_index().await;
        dropped.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(updated: i64, origin: &str) -> StoredStateItem {
        StoredStateItem {
            channel: "tmux".into(),
            key: "notebook".into(),
            updated,
            origin: origin.into(),
            seq: 1,
            meta: None,
            file: "tmux/notebook".into(),
        }
    }

    fn item(updated: i64) -> StateItem {
        StateItem {
            channel: "tmux".into(),
            key: "notebook".into(),
            updated,
            data: "x".into(),
            ..Default::default()
        }
    }

    #[test]
    fn lww_accepts_newer_and_same_origin() {
        let old = stored(10, "desktop");
        assert!(accept_lww(None, &item(1), "notebook"));
        assert!(accept_lww(Some(&old), &item(11), "notebook"));
        assert!(!accept_lww(Some(&old), &item(10), "notebook"));
        assert!(!accept_lww(Some(&old), &item(9), "notebook"));
        // Собственный push не блокируется меньшим/равным updated.
        assert!(accept_lww(Some(&old), &item(9), "desktop"));
        assert!(accept_lww(Some(&old), &item(10), "desktop"));
    }
}
