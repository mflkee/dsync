//! Применение чужих элементов состояния через каналы.

use super::*;

#[derive(Default)]
pub struct ApplyReport {
    pub messages: Vec<String>,
    pub applied: Vec<StateItem>,
    pub failed: Vec<StateItem>,
    /// Пропущено: уже применено ранее / канал выключен / неизвестный канал.
    pub skipped: usize,
}

/// Применяет полученные от хаба элементы.
///
/// Уже применённые элементы (по локальному индексу) пропускаются — это делает
/// ретраи безопасными: повторная страница состояния не перезапускает
/// `tmux-resurrect restore` и не переимпортирует сессии.
pub fn apply(cfg: &Config, items: Vec<StateItem>) -> ApplyReport {
    let mut report = ApplyReport::default();
    if cfg.state.is_none() {
        return report;
    }
    let idx = StateIndex::load();
    let channels = channels();
    let mut refresh_projects: Vec<PathBuf> = Vec::new();
    for item in items {
        let channel = channels.iter().find(|c| c.name() == item.channel);
        let Some(channel) = channel else {
            tracing::debug!("state: канал {} не поддерживается — пропуск", item.channel);
            report.skipped += 1;
            continue;
        };
        if !channel.accepts(cfg, &item) {
            report.skipped += 1;
            continue;
        }
        if idx.is_known(&item) {
            report.skipped += 1;
            continue;
        }
        match channel.apply(cfg, &item) {
            Ok(msg) => {
                report.messages.push(msg);
                if item.channel == OPENCODE_CHANNEL {
                    if let Some(m) = &item.meta {
                        refresh_projects.push(PathBuf::from(m));
                    }
                }
                report.applied.push(item);
            }
            Err(e) => {
                warn!(
                    "state: не удалось применить {}/{}: {e:#}",
                    item.channel, item.key
                );
                report.failed.push(item);
            }
        }
    }
    if !report.applied.is_empty() {
        mark_synced(&report.applied);
    }
    if !refresh_projects.is_empty() {
        refresh_projects.sort();
        refresh_projects.dedup();
        refresh_opencode_index(&refresh_projects);
    }
    report
}
