//! POSIX-shell: экранирование аргументов и склейка путей для удалённых команд.
//!
//! Любая строка, которая уходит в шелл на другой машине, собирается здесь.
//! `Path::join` для remote-команд не используется: на Windows-хосте он даёт
//! обратные слэши и ломает guard (`...\\.git`).

/// Экранировать строку для POSIX-shell одинарными кавычками.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Склеить базовый путь и компонент через `/` (без двойных слэшей).
pub fn join_posix(base: &str, part: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        part.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_escapes_single_quotes() {
        assert_eq!(quote("simple"), "'simple'");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn join_posix_is_platform_independent() {
        assert_eq!(join_posix("/home/u/proj", ".git"), "/home/u/proj/.git");
        assert_eq!(join_posix("/home/u/proj/", ".git"), "/home/u/proj/.git");
        assert_eq!(join_posix("/home/u/proj", "/.git"), "/home/u/proj/.git");
        assert_eq!(
            join_posix("/tmp/a b/'quote'", ".git"),
            "/tmp/a b/'quote'/.git"
        );
        assert_eq!(
            join_posix("/home/u/Проекты/мой репо", ".git"),
            "/home/u/Проекты/мой репо/.git"
        );
    }
}
