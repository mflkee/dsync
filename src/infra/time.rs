//! Общие time-хелперы: секунды/миллисекунды эпохи.

use std::time::{SystemTime, UNIX_EPOCH};

/// Секунды с Unix-эпохи (0, если системные часы уехали до 1970).
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_sane() {
        assert!(now_secs() > 1_600_000_000, "sane epoch seconds");
    }
}
