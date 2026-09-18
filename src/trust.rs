use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Хранилище доверенных отпечатков хаба (TOFU known-hosts).
///
/// Файл: `{data_dir}/dsync/known_hosts.toml`. Формат — карта адрес хаба → отпечаток:
///
/// ```toml
/// [hosts."100.89.126.211:42069"]
/// fingerprint = "sha256:ab12..."
/// ```
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct TrustStore {
    #[serde(default)]
    hosts: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Entry {
    fingerprint: String,
}

impl TrustStore {
    pub fn path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_default()
            .join("dsync")
            .join("known_hosts.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // 0600: известные хосты не секрет, но и не для всех.
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true).mode(0o600);
            let mut f = opts.open(&path)?;
            f.write_all(toml::to_string(self)?.as_bytes())?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&path, toml::to_string(self)?)?;
            Ok(())
        }
    }

    pub fn get(&self, addr: &str) -> Option<&str> {
        self.hosts.get(addr).map(|e| e.fingerprint.as_str())
    }

    pub fn insert(&mut self, addr: &str, fingerprint: &str) {
        self.hosts.insert(
            addr.to_string(),
            Entry {
                fingerprint: fingerprint.to_string(),
            },
        );
    }

    pub fn remove(&mut self, addr: &str) -> bool {
        self.hosts.remove(addr).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.hosts
            .iter()
            .map(|(a, e)| (a.as_str(), e.fingerprint.as_str()))
    }
}

/// SHA-256 отпечаток сертификата в формате `sha256:<hex>` (как в OpenSSH known_hosts).
pub fn fingerprint(cert_der: &[u8]) -> String {
    let digest = Sha256::digest(cert_der);
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// `dsync trust list` — показать доверенные отпечатки хаба.
pub fn trust_list() -> Result<Vec<String>> {
    let store = TrustStore::load();
    let mut out = vec!["Trusted hubs:".to_string()];
    let mut any = false;
    for (addr, fp) in store.iter() {
        out.push(format!("  {addr}  {fp}"));
        any = true;
    }
    if !any {
        out.push("  (none — first connect will trust the hub it meets)".to_string());
    }
    Ok(out)
}

/// `dsync trust rm <addr>` — забыть отпечаток; следующий коннект снова TOFU.
pub fn trust_rm(address: &str) -> Result<Vec<String>> {
    let mut store = TrustStore::load();
    if store.remove(address) {
        store.save()?;
        Ok(vec![format!(
            "removed trust for {address}; next connect will re-trust (TOFU)"
        )])
    } else {
        Ok(vec![format!("no trusted fingerprint for {address}")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_format() {
        // SHA-256 от пустой строки — известная константа.
        let fp = fingerprint(&[]);
        assert_eq!(
            fp,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn store_roundtrip() {
        let mut s = TrustStore::default();
        s.insert("100.89.126.211:42069", "sha256:abc");
        s.insert("127.0.0.1:42069", "sha256:def");
        assert_eq!(s.get("100.89.126.211:42069"), Some("sha256:abc"));
        assert!(s.remove("127.0.0.1:42069"));
        assert!(!s.remove("127.0.0.1:42069"));
        let out: Vec<String> = s.iter().map(|(a, f)| format!("{a}={f}")).collect();
        assert_eq!(out, vec!["100.89.126.211:42069=sha256:abc"]);
    }
}
