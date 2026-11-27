//! Атомарная запись файлов: временный файл в том же каталоге + `rename`.
//!
//! Используется для индексов состояния и TOFU-хранилищ, где обрыв процесса
//! посередине записи не должен оставлять полу-валидный JSON/TOML.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Записать `data` в `path` атомарно. `mode` — права файла на Unix
/// (например `Some(0o600)`); на других платформах игнорируется.
pub fn write_atomic(path: &Path, data: &[u8], mode: Option<u32>) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let write = || -> io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if let Some(m) = mode {
                opts.mode(m);
            }
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        Ok(())
    };

    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "dsync-atomic-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn writes_and_overwrites() {
        let path = tmp("rw").join("nested/file.txt");
        write_atomic(&path, b"one", None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        write_atomic(&path, b"two", None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn applies_mode() {
        use std::os::unix::fs::MetadataExt;
        let path = tmp("mode").join("secret.toml");
        write_atomic(&path, b"x", Some(0o600)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn leaves_no_temp_files() {
        let dir = tmp("clean");
        let path = dir.join("a.json");
        write_atomic(&path, b"{}", None).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
