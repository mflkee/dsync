use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// TOFU-хранилище отпечатков SSH host-keys, которым доверяет хаб.
///
/// Файл: `{hub data_dir}/ssh_known_hosts.toml`. Формат — карта `host:port` →
/// отпечаток:
///
/// ```toml
/// host = "sha256:ab12..."
/// ```
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SshHostTrustStore {
    #[serde(default)]
    hosts: BTreeMap<String, String>,
}

impl SshHostTrustStore {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("ssh_known_hosts.toml")
    }

    /// Загрузка из файла; отсутствующий/битый файл = пустой store.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string(self)?)?;
        Ok(())
    }

    pub fn get(&self, host: &str) -> Option<&str> {
        self.hosts.get(host).map(String::as_str)
    }

    pub fn insert(&mut self, host: String, fp: String) {
        self.hosts.insert(host, fp);
    }

    pub fn remove(&mut self, host: &str) -> bool {
        self.hosts.remove(host).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.hosts.iter().map(|(h, f)| (h.as_str(), f.as_str()))
    }
}

/// SHA-256 отпечаток «сырого» SSH-ключа (blob в wire-формате) в виде
/// `sha256:<hex>` — тот же формат, что у QUIC-сертификата хаба и OpenSSH.
pub fn ssh_fingerprint(blob: &[u8]) -> String {
    let digest = Sha256::digest(blob);
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// Состояние доверия SSH-host для доктора.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshTrustState {
    /// Предъявленный отпечаток совпал с сохранённым.
    Trusted,
    /// Машина новая — отпечаток запишется при первом успешном коннекте.
    Untrusted,
    /// Сохранённый отпечаток не совпал с предъявленным.
    Mismatch { expected: String, observed: String },
    /// До машины не дотянуться (таймаут / сеть / sshd недоступен).
    Unreachable(String),
}

/// `dsync trust ssh list` — показать доверенные SSH host-отпечатки.
pub fn ssh_trust_list(data_dir: &Path) -> Vec<String> {
    let store = SshHostTrustStore::load(&SshHostTrustStore::path(data_dir));
    let mut out = vec!["Trusted SSH hosts:".to_string()];
    let mut any = false;
    for (host, fp) in store.iter() {
        out.push(format!("  {host}  {fp}"));
        any = true;
    }
    if !any {
        out.push("  (none — first hub pull will trust each machine it meets)".to_string());
    }
    out
}

/// `dsync trust ssh rm` — забыть отпечаток машины; следующий пул снова TOFU.
pub fn ssh_trust_rm(data_dir: &Path, host_port: &str) -> Vec<String> {
    let path = SshHostTrustStore::path(data_dir);
    let mut store = SshHostTrustStore::load(&path);
    if store.remove(host_port) {
        if let Err(e) = store.save(&path) {
            return vec![format!("failed to save ssh trust store: {e:#}")];
        }
        vec![format!(
            "removed SSH trust for {host_port}; next pull will re-trust (TOFU)"
        )]
    } else {
        vec![format!("no trusted SSH fingerprint for {host_port}")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_known_vector() {
        // SHA-256 от пустой строки.
        assert_eq!(
            ssh_fingerprint(&[]),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn store_roundtrip_and_missing_file_default() {
        let dir = std::env::temp_dir().join(format!("dsync-ssh-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = SshHostTrustStore::path(&dir);

        // Отсутствующий файл = пустой store, без паники.
        let s = SshHostTrustStore::load(&path);
        assert!(s.iter().next().is_none());

        let mut s = s;
        s.insert("10.0.0.5:22".to_string(), "sha256:abc".to_string());
        s.insert("10.0.0.6:22".to_string(), "sha256:def".to_string());
        s.save(&path).unwrap();

        let mut back = SshHostTrustStore::load(&path);
        assert_eq!(back.get("10.0.0.5:22"), Some("sha256:abc"));
        assert_eq!(back.get("10.0.0.6:22"), Some("sha256:def"));
        assert!(back.remove("10.0.0.5:22"));
        assert!(!back.remove("10.0.0.5:22"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
