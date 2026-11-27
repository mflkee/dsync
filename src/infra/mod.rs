//! Общая инфраструктура: атомарная запись, время, retry, POSIX-shell,
//! TOFU-хранилище отпечатков.

pub mod atomic;
pub mod fingerprint;
pub mod retry;
pub mod shell;
pub mod time;
