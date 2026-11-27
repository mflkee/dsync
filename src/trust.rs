use std::path::PathBuf;

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::infra::fingerprint::{FingerprintStore, StoreFormat};

/// Хранилище доверенных отпечатков хаба (TOFU known-hosts).
///
/// Файл: `{data_dir}/dsync/known_hosts.toml`. Формат — карта адрес хаба → отпечаток:
///
/// ```toml
/// [hosts."100.89.126.211:42069"]
/// fingerprint = "sha256:ab12..."
/// ```
pub struct TrustStore {
    inner: FingerprintStore,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self {
            inner: FingerprintStore::new(Self::path(), StoreFormat::Nested, Some(0o600)),
        }
    }
}

impl TrustStore {
    pub fn path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_default()
            .join("dsync")
            .join("known_hosts.toml")
    }

    pub fn load() -> Self {
        Self {
            inner: FingerprintStore::load(Self::path(), StoreFormat::Nested, Some(0o600)),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.inner.save()?;
        Ok(())
    }

    pub fn get(&self, addr: &str) -> Option<&str> {
        self.inner.get(addr)
    }

    pub fn insert(&mut self, addr: &str, fingerprint: &str) {
        self.inner.insert(addr, fingerprint);
    }

    pub fn remove(&mut self, addr: &str) -> bool {
        self.inner.remove(addr)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner.iter()
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
