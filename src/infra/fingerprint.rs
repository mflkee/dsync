//! Общее TOFU-хранилище отпечатков: ключ → `sha256:...`.
//!
//! Форматов два, оба уже лежат на диске у пользователей:
//! * `Nested` — `[hosts."addr"] fingerprint = "..."` (QUIC-сертификат хаба);
//! * `Flat` — `[hosts] "addr" = "..."` (SSH host-key хаба).
//!
//! Различия (путь, формат, права) — параметры; логика загрузки/сохранения и
//! атомарная запись общие.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::atomic::write_atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFormat {
    Nested,
    Flat,
}

#[derive(Debug, Default)]
pub struct FingerprintStore {
    path: PathBuf,
    format: StoreFormat,
    mode: Option<u32>,
    hosts: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct NestedDoc {
    #[serde(default)]
    hosts: BTreeMap<String, NestedEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct NestedEntry {
    fingerprint: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FlatDoc {
    #[serde(default)]
    hosts: BTreeMap<String, String>,
}

impl Default for StoreFormat {
    fn default() -> Self {
        Self::Nested
    }
}

impl FingerprintStore {
    pub fn new(path: impl Into<PathBuf>, format: StoreFormat, mode: Option<u32>) -> Self {
        Self {
            path: path.into(),
            format,
            mode,
            hosts: BTreeMap::new(),
        }
    }

    /// Загрузить стор; отсутствующий/битый файл = пустой стор.
    pub fn load(path: impl Into<PathBuf>, format: StoreFormat, mode: Option<u32>) -> Self {
        let mut store = Self::new(path, format, mode);
        let Ok(text) = std::fs::read_to_string(&store.path) else {
            return store;
        };
        store.hosts = match format {
            StoreFormat::Nested => toml::from_str::<NestedDoc>(&text)
                .map(|d| {
                    d.hosts
                        .into_iter()
                        .map(|(k, v)| (k, v.fingerprint))
                        .collect()
                })
                .unwrap_or_default(),
            StoreFormat::Flat => toml::from_str::<FlatDoc>(&text)
                .map(|d| d.hosts)
                .unwrap_or_default(),
        };
        store
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&self.path)
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let text = match self.format {
            StoreFormat::Nested => {
                let doc = NestedDoc {
                    hosts: self
                        .hosts
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                NestedEntry {
                                    fingerprint: v.clone(),
                                },
                            )
                        })
                        .collect(),
                };
                toml::to_string(&doc)
            }
            StoreFormat::Flat => {
                let doc = FlatDoc {
                    hosts: self.hosts.clone(),
                };
                toml::to_string(&doc)
            }
        }
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(path, text.as_bytes(), self.mode)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.hosts.get(key).map(String::as_str)
    }

    pub fn insert(&mut self, key: &str, fingerprint: &str) {
        self.hosts.insert(key.to_string(), fingerprint.to_string());
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.hosts.remove(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.hosts.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dsync-fp-{tag}-{}", std::process::id()))
    }

    #[test]
    fn nested_roundtrip() {
        let dir = tmp_dir("nested");
        let path = dir.join("known_hosts.toml");
        let mut s = FingerprintStore::new(&path, StoreFormat::Nested, Some(0o600));
        s.insert("hub:42069", "sha256:abc");
        s.save().unwrap();
        let back = FingerprintStore::load(&path, StoreFormat::Nested, Some(0o600));
        assert_eq!(back.get("hub:42069"), Some("sha256:abc"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flat_roundtrip() {
        let dir = tmp_dir("flat");
        let path = dir.join("ssh_known_hosts.toml");
        let mut s = FingerprintStore::new(&path, StoreFormat::Flat, None);
        s.insert("10.0.0.5:22", "sha256:def");
        s.save().unwrap();
        let back = FingerprintStore::load(&path, StoreFormat::Flat, None);
        assert_eq!(back.get("10.0.0.5:22"), Some("sha256:def"));
        assert!(back.iter().any(|(h, _)| h == "10.0.0.5:22"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_broken_file_is_empty() {
        let dir = tmp_dir("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "not = [toml").unwrap();
        let s = FingerprintStore::load(&path, StoreFormat::Flat, None);
        assert!(s.iter().next().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn nested_store_is_0600() {
        use std::os::unix::fs::MetadataExt;
        let dir = tmp_dir("mode");
        let path = dir.join("known_hosts.toml");
        let mut s = FingerprintStore::new(&path, StoreFormat::Nested, Some(0o600));
        s.insert("hub", "sha256:abc");
        s.save().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
