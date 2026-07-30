#![allow(non_snake_case)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ZenExport {
    pub _source: String,
    pub containers: Option<Value>,
    pub themes: Option<Value>,
    pub spaces: Option<Vec<Value>>,
    pub groups: Option<Vec<Value>>,
    pub folders: Option<Vec<Value>>,
    pub pinned_tabs: Option<Vec<Value>>,
    pub space_routing: Option<Value>,
    pub live_folders: Option<Value>,
}
