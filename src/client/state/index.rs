//! Локальный state-индекс: seq хаба, применённые элементы, failures и dead-letter.

use super::*;

/// Счётчик неудачных применений элемента (для retry/backoff).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub pub(crate) struct Failure {
    pub attempts: u32,
    pub updated: i64,
    /// hub-seq элемента — чтобы вернуться к нему на следующем pull.
    pub seq: i64,
}

/// Элемент, который не применился `retry_limit` раз. Больше не блокирует
/// канал: следующие элементы применяются, а этот ждёт нового `updated`
/// (тогда origin пришлёт новую версию) или `dsync state retry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub pub(crate) struct DeadItem {
    pub updated: i64,
    pub seq: i64,
    pub error: String,
    /// Когда элемент попал в dead-letter (сек. эпохи).
    pub at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct StateIndex {
    #[serde(default)]
    pub(crate) seq: i64,
    #[serde(default)]
    pub(crate) items: HashMap<String, i64>,
    /// Ключ `channel:key` → счётчик неудач.
    #[serde(default)]
    pub(crate) failures: HashMap<String, Failure>,
    /// Ключ `channel:key` → окончательно сбойный элемент.
    #[serde(default)]
    pub(crate) dead: HashMap<String, DeadItem>,
}

fn index_path() -> PathBuf {
    if let Ok(p) = std::env::var("DSYNC_STATE_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::infra::paths::data_dir()
        .unwrap_or_default()
        .join("dsync/state.json")
}

impl StateIndex {
    pub(crate) fn load() -> Self {
        let p = index_path();
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self) -> Result<()> {
        let p = index_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub(crate) fn key_of(item: &StateItem) -> String {
        format!("{}:{}", item.channel, item.key)
    }

    /// Уже знаем это значение (не нужно повторно отправлять/применять)?
    pub(crate) fn has(&self, item: &StateItem) -> bool {
        self.items
            .get(&Self::key_of(item))
            .map(|u| *u >= item.updated)
            .unwrap_or(false)
    }

    /// Точное совпадение `updated` — для каналов, где `updated` является хешем
    /// содержимого (tmux-снапшот), а не монотонным счётчиком. Сравнение `>=`
    /// здесь ошибочно: новый хеш может оказаться численно МЕНЬШЕ прежнего, и
    /// тогда обновление молча теряется навсегда — в индексе остаётся большое
    /// «отравленное» значение, которое блокирует все последующие раскладки.
    pub(crate) fn has_exact(&self, item: &StateItem) -> bool {
        self.items
            .get(&Self::key_of(item))
            .map(|u| *u == item.updated)
            .unwrap_or(false)
    }

    /// Элемент уже применён (не нужно применять повторно)?
    ///
    /// Критично для ретраев: раньше seq откатывался назад при любой ошибке,
    /// и вместе с упавшим элементом заново применялись все успешные — включая
    /// tmux-снапшот, из-за чего `tmux-resurrect restore` дёргался каждые
    /// 15 минут. Проверка перед `apply_one` это исключает.
    pub(crate) fn is_known(&self, item: &StateItem) -> bool {
        if item.channel == TMUX_CHANNEL {
            self.has_exact(item)
        } else {
            self.has(item)
        }
    }
}

/// Публичная сводка локального state-индекса (для `doctor`/`state list`).
pub pub(crate) struct StateHealth {
    pub seq: i64,
    pub applied: usize,
    pub failures: Vec<(String, Failure)>,
    pub dead: Vec<(String, DeadItem)>,
}

pub fn health() -> StateHealth {
    let idx = StateIndex::load();
    let mut failures: Vec<_> = idx.failures.into_iter().collect();
    failures.sort_by(|a, b| a.0.cmp(&b.0));
    let mut dead: Vec<_> = idx.dead.into_iter().collect();
    dead.sort_by(|a, b| a.0.cmp(&b.0));
    StateHealth {
        pub(crate) seq: idx.seq,
        applied: idx.items.len(),
        failures,
        dead,
    }
}

/// Машиночитаемая сводка локального state-индекса (`dsync state list --json`).
pub fn report() -> crate::diagnostics::StateReport {
    use crate::diagnostics::{DeadReport, FailureReport, StateReport, SCHEMA_VERSION};
    let h = health();
    StateReport {
        schema_version: SCHEMA_VERSION,
        pub(crate) seq: h.seq,
        applied: h.applied,
        pub(crate) failures: h
            .failures
            .into_iter()
            .map(|(key, f)| FailureReport {
                key,
                attempts: f.attempts,
                updated: f.updated,
                pub(crate) seq: f.seq,
            })
            .collect(),
        dead_letters: h
            .dead
            .into_iter()
            .map(|(key, d)| DeadReport {
                key,
                updated: d.updated,
                pub(crate) seq: d.seq,
                error: d.error,
                at: d.at,
            })
            .collect(),
    }
}

/// Человекочитаемая сводка локального state-индекса (`dsync state list`).
pub fn describe() -> Vec<String> {
    let h = health();
    let mut out = vec![format!(
        "state index: seq={}, applied={}, failing={}, dead-letter={}",
        h.seq,
        h.applied,
        h.failures.len(),
        h.dead.len()
    )];
    for (key, f) in &h.failures {
        out.push(format!(
            "  failing  {key}: {} attempt(s), updated={}",
            f.attempts, f.updated
        ));
    }
    for (key, d) in &h.dead {
        out.push(format!(
            "  dead     {key}: {} (seq={}, updated={})",
            d.error, d.seq, d.updated
        ));
    }
    out
}

/// Убирает элементы из dead-letter (все или один `channel:key`) и откатывает
/// seq к самому раннему из них, чтобы хаб выдал их снова.
pub fn retry_dead(only: Option<&str>) -> usize {
    let mut idx = StateIndex::load();
    let keys: Vec<String> = match only {
        Some(want) => idx
            .dead
            .keys()
            .filter(|k| k.as_str() == want)
            .cloned()
            .collect(),
        None => idx.dead.keys().cloned().collect(),
    };
    if keys.is_empty() {
        return 0;
    }
    let mut min_seq: Option<i64> = None;
    for k in &keys {
        if let Some(d) = idx.dead.remove(k) {
            min_seq = Some(min_seq.map_or(d.seq, |m: i64| m.min(d.seq)));
            idx.failures.remove(k);
        }
    }
    if let Some(seq) = min_seq {
        idx.seq = (seq - 1).max(0);
    }
    if let Err(e) = idx.save() {
        warn!("can't save state index: {e:#}");
    }
    keys.len()
}

/// Последний известный seq хаба (для инкрементального pull).
pub fn get_seq() -> i64 {
    StateIndex::load().seq
}

/// Запоминает seq хаба (монотонно — не откатываем назад).
pub fn set_seq(seq: i64) {
    let mut idx = StateIndex::load();
    if seq > idx.seq {
        idx.seq = seq;
        if let Err(e) = idx.save() {
            warn!("can't save state index: {e:#}");
        }
    }
}

/// Принудительно ставит seq (в т.ч. назад) — чтобы на следующем pull повторить
/// элементы, которые не удалось применить.
pub(crate) fn set_seq_force(seq: i64) {
    let mut idx = StateIndex::load();
    idx.seq = seq.max(0);
    if let Err(e) = idx.save() {
        warn!("can't save state index: {e:#}");
    }
}

/// Помечает элементы отправленными/применёнными (после успешной синхронизации).
pub(crate) fn mark_synced(items: &[StateItem]) {
    let mut idx = StateIndex::load();
    for it in items {
        idx.items.insert(StateIndex::key_of(it), it.updated);
    }
    if let Err(e) = idx.save() {
        warn!("can't save state index: {e:#}");
    }
}

/// Обновляет учёт неудач и возвращает seq самого раннего элемента, у которого
/// ещё остались попытки (курсор pull'а должен остаться перед ним), либо None.
///
/// После `retry_limit` неудач элемент уходит в dead-letter: канал больше не
/// блокируется, а элемент ждёт новой версии от origin или `dsync state retry`.
pub(crate) fn record_failures(
    idx: &mut StateIndex,
    applied: &[StateItem],
    failed: &[StateItem],
    retry_limit: u32,
) -> Option<i64> {
    for it in applied {
        let k = StateIndex::key_of(it);
        idx.failures.remove(&k);
        idx.dead.remove(&k);
    }
    let mut blocked: Option<i64> = None;
    for it in failed {
        let k = StateIndex::key_of(it);
        let f = idx.failures.entry(k.clone()).or_insert(Failure {
            attempts: 0,
            updated: it.updated,
            pub(crate) seq: it.seq,
        });
        if f.updated != it.updated {
            // Пришла новая версия сбойного элемента — считаем попытки заново.
            f.attempts = 0;
            f.updated = it.updated;
            idx.dead.remove(&k);
        }
        f.attempts += 1;
        f.seq = it.seq;
        if f.attempts >= retry_limit {
            let error = format!(
                "не удалось применить за {} попыт(ки) (канал {})",
                f.attempts, it.channel
            );
            warn!("state: {}/{} → dead-letter", it.channel, it.key);
            idx.dead.insert(
                k.clone(),
                DeadItem {
                    updated: it.updated,
                    pub(crate) seq: it.seq,
                    error,
                    at: now_secs(),
                },
            );
            idx.failures.remove(&k);
        } else if it.seq > 0 {
            blocked = Some(blocked.map_or(it.seq, |m: i64| m.min(it.seq)));
        }
    }
    blocked
}

/// Убирает из локального индекса dead-letter и счётчики каналов, которые
/// машина больше не синхронизирует (`[state.opencode] projects = []` и т.п.):
/// они никогда не будут повторены, но засоряют `dsync status`/`doctor`.
pub(crate) fn prune_unwanted_index(channels: &[String]) {
    let mut idx = StateIndex::load();
    let keep = |key: &str| channels.iter().any(|c| key.starts_with(&format!("{c}:")));
    let before = idx.failures.len() + idx.dead.len();
    idx.failures.retain(|k, _| keep(k));
    idx.dead.retain(|k, _| keep(k));
    if idx.failures.len() + idx.dead.len() != before {
        if let Err(e) = idx.save() {
            warn!("can't save state index: {e:#}");
        }
    }
}
