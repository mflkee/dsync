//! Пути пользователя через `directories` (единая замена `dirs`).
//!
//! Все модули ходят за `$HOME`/`$XDG_*` только сюда, чтобы раскладка на
//! Linux/macOS/Windows решалась в одном месте.

use std::path::PathBuf;

pub fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}

pub fn data_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.data_dir().to_path_buf())
}

pub fn config_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_user_dirs() {
        let home = home_dir().expect("home dir");
        assert!(home.is_absolute(), "home is absolute: {}", home.display());
        let data = data_dir().expect("data dir");
        assert!(data.is_absolute(), "data is absolute: {}", data.display());
    }
}
