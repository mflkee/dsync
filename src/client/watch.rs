use std::time::Duration;

use anyhow::Result;
use tracing::{error, info};

use crate::config::Config;

use super::{pull, push};

/// Бесконечный цикл синхронизации: каждые `interval` секунд пушим локальное
/// состояние в хаб и тянем состояние флота. Ошибки (хаб недоступен, упавший
/// SSH) не роняют цикл — логируем и ждём следующего тика.
///
/// Это кросс-платформенная замена systemd-таймера: `dsync watch` работает
/// на любой ОС, где есть xdg-совместимые пути (или фолбэк в CWD).
pub async fn watch(cfg: Config, interval_secs: u64) -> Result<()> {
    let interval = Duration::from_secs(interval_secs.clamp(30, 86400));
    info!("dsync watch: interval {}s", interval.as_secs());

    let mut first = true;
    loop {
        if !first {
            tokio::time::sleep(interval).await;
        }
        first = false;

        let ts = chrono::Local::now().format("%H:%M:%S");
        info!("[{ts}] sync cycle start");

        match push(cfg.clone(), None).await {
            Ok(_) => info!("[{ts}] push OK"),
            Err(e) => error!("[{ts}] push failed: {e:#}"),
        }

        match pull(cfg.clone(), None).await {
            Ok(_) => info!("[{ts}] pull OK"),
            Err(e) => error!("[{ts}] pull failed: {e:#}"),
        }
    }
}
