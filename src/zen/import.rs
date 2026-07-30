use anyhow::Result;

/// Import Zen browser profile from JSON.
/// Merges spaces/groups/folders, adds new pinned tabs,
/// cleans sessionstore to prevent old-session restore.
pub fn import(_profile_path: &std::path::Path, _data: &[u8]) -> Result<()> {
    // TODO: Фаза 2 — портировать из Python `zen.py`
    Ok(())
}
