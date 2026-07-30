use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ZenExport {
    pub _source: String,
    pub containers: Option<Value>,
    pub themes: Option<Value>,
    pub spaces: Option<Vec<Workspace>>,
    pub groups: Option<Vec<Group>>,
    pub folders: Option<Vec<Folder>>,
    pub pinned_tabs: Option<Vec<PinnedTab>>,
    pub space_routing: Option<Value>,
    pub live_folders: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Workspace {
    pub uuid: String,
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub theme: Option<Value>,
    #[serde(default)]
    pub hasCollapsedPinnedTabs: Option<bool>,
    #[serde(default)]
    pub containerTabId: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Group {
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub collapsed: Option<bool>,
    #[serde(default)]
    pub splitView: Option<bool>,
    #[serde(default)]
    pub saveOnWindowClose: Option<bool>,
    #[serde(default)]
    pub workspaceId: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Folder {
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub workspaceId: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub collapsed: Option<bool>,
    #[serde(default)]
    pub splitViewGroup: Option<bool>,
    #[serde(default)]
    pub saveOnWindowClose: Option<bool>,
    #[serde(default)]
    pub parentId: Option<String>,
    #[serde(default)]
    pub userIcon: Option<String>,
    #[serde(default)]
    pub prevSiblingInfo: Option<Value>,
    #[serde(default)]
    pub emptyTabIds: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PinnedTab {
    #[serde(default)]
    pub entries: Vec<TabEntry>,
    #[serde(default)]
    pub groupId: Option<String>,
    #[serde(default)]
    pub zenWorkspace: Option<String>,
    #[serde(default)]
    pub zenLiveFolderItemId: Option<String>,
    #[serde(default)]
    pub zenSyncId: Option<String>,
    #[serde(default)]
    pub userContextId: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TabEntry {
    pub url: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionData {
    pub lastCollected: Option<i64>,
    pub tabs: Option<Vec<Value>>,
    pub folders: Option<Vec<Value>>,
    pub groups: Option<Vec<Value>>,
    pub spaces: Option<Vec<Value>>,
    #[serde(default)]
    pub liveFolders: Option<Vec<Value>>,
}
