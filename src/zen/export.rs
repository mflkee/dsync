use anyhow::Result;

/// Export Zen browser profile to JSON.
/// Reads from ~/.zen/chrome/user-data-dir, extracts:
/// - containers (contextualIdentities)
/// - themes
/// - spaces (zenWorkspaces)
/// - groups (tabGroups)
/// - folders (zenLiveFolderItemIds)
/// - pinned tabs
pub fn export(_profile_path: &std::path::Path) -> Result<Vec<u8>> {
    // TODO: Фаза 2 — портировать из Python `zen.py`
    Ok(Vec::new())
}
