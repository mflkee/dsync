use anyhow::Result;

/// Scan git repositories and return their status.
/// For each repo: branch, dirty, ahead/behind, last commit.
pub fn scan(_paths: &[std::path::PathBuf]) -> Result<Vec<crate::protocol::ProjectState>> {
    // TODO: Фаза 3 — git status через std::process::Command
    Ok(Vec::new())
}
