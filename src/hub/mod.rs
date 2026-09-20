pub(crate) mod server;
pub mod state;

pub use server::run_server;

/// Constant-time token comparison.
///
/// Both sides are hashed (SHA-256) first so the comparison itself always runs
/// over fixed-size (32-byte) digests — a timing side channel cannot reveal
/// length differences between the expected and provided tokens.
pub fn token_matches(expected: &str, provided: &str) -> bool {
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(provided.as_bytes());
    bool::from(a.ct_eq(&b))
}

#[cfg(test)]
mod tests {
    use super::token_matches;

    #[test]
    fn equal_tokens_match() {
        assert!(token_matches("tok-a", "tok-a"));
        assert!(token_matches("", ""));
    }

    #[test]
    fn unequal_tokens_do_not_match() {
        assert!(!token_matches("tok-a", "tok-b"));
        assert!(!token_matches("tok-a", ""));
        assert!(!token_matches("", "tok-a"));
    }

    #[test]
    fn differing_lengths_do_not_match() {
        assert!(!token_matches("short", "a-much-longer-token-value"));
    }
}
