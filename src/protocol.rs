use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PushRequest {
    pub machine: String,
    pub timestamp: i64,
    pub zen: Option<ZenState>,
    pub projects: Vec<ProjectState>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PushResponse {
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullRequest {
    pub machine: String,
    pub timestamp: i64,
    pub filter: Option<PullFilter>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullFilter {
    pub zen: bool,
    pub projects: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullResponse {
    pub machines: HashMap<String, MachineState>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MachineState {
    pub name: String,
    pub last_push: i64,
    pub zen: Option<ZenState>,
    pub projects: Vec<ProjectState>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ZenState {
    pub data: Vec<u8>,
    pub checksum: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectState {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub dirty: bool,
    pub ahead: usize,
    pub behind: usize,
    pub commit_hash: String,
    pub last_commit_time: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusRequest {
    pub machine: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub machines: HashMap<String, MachineStatus>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MachineStatus {
    pub online: bool,
    pub last_seen: i64,
    pub last_push: i64,
}
