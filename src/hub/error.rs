//! Типизированные ошибки границ хаба.
//!
//! Wire-формат (`PullOutcome.error`, JSON-конверты) по-прежнему использует
//! строки — это часть протокола. Внутри хаба ошибки несут источник и тип.

/// Ошибка запуска/настройки хаба.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HubError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
    #[error("certificate generation: {0}")]
    Certificate(#[from] rcgen::Error),
    #[error("config: {0}")]
    Config(String),
}
