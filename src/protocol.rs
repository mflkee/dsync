use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PushRequest {
    pub machine: String,
    /// Hub auth token (see [hub_connect] token / [hub] tokens).
    #[serde(default)]
    pub token: String,
    pub timestamp: i64,
    pub projects: Vec<ProjectState>,
    /// Restrict hub SSH-pulls to this machine (default: all in project config)
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PushResponse {
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullRequest {
    pub machine: String,
    /// Hub auth token (see [hub_connect] token / [hub] tokens).
    #[serde(default)]
    pub token: String,
    pub timestamp: i64,
    pub filter: Option<PullFilter>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullFilter {
    pub projects: bool,
    /// Only return state for this machine
    #[serde(default)]
    pub machine: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullResponse {
    pub machines: HashMap<String, MachineState>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MachineState {
    pub name: String,
    pub last_push: i64,
    pub projects: Vec<ProjectState>,
    /// Last SSH-pull outcome per project (see `hub-pull-orchestration`).
    #[serde(default)]
    pub pulls: HashMap<String, PullOutcome>,
}

/// Outcome of a hub-initiated SSH pull for one (machine, project) pair.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PullOutcome {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Total attempts made (initial + retries).
    #[serde(default)]
    pub attempts: u32,
    /// Hub clock, seconds since epoch.
    #[serde(default)]
    pub finished_at: i64,
}

impl PullOutcome {
    pub fn success(attempts: u32) -> Self {
        Self {
            ok: true,
            error: None,
            attempts,
            finished_at: unix_now(),
        }
    }

    pub fn failure(error: String, attempts: u32) -> Self {
        Self {
            ok: false,
            error: Some(error),
            attempts,
            finished_at: unix_now(),
        }
    }
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
    /// Hub auth token (see [hub_connect] token / [hub] tokens).
    #[serde(default)]
    pub token: String,
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
    /// Last SSH-pull outcome per project (see `hub-pull-orchestration`).
    #[serde(default)]
    pub pulls: HashMap<String, PullOutcome>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Serde-friendly error envelope sent by the hub for any rejected request.
pub fn error_envelope(message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({ "type": "error", "error": message.into() })
}

/// Parses a hub response; converts the hub's error envelope into a Rust error.
pub fn parse_response<T: serde::de::DeserializeOwned>(buf: &[u8]) -> anyhow::Result<T> {
    let val: serde_json::Value = serde_json::from_slice(buf)?;
    if val.get("type").and_then(|v| v.as_str()) == Some("error") {
        let msg = val
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("hub error");
        anyhow::bail!("hub error: {msg}");
    }
    serde_json::from_value(val).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_default_token_and_pulls_absent() {
        // Старые клиенты/конфиги без token и pulls парсятся.
        let push: PushRequest =
            serde_json::from_str(r#"{"machine":"desktop","timestamp":1,"projects":[]}"#).unwrap();
        assert_eq!(push.token, "");
        let st: MachineState =
            serde_json::from_str(r#"{"name":"desktop","last_push":1,"projects":[]}"#).unwrap();
        assert!(st.pulls.is_empty());
    }

    #[test]
    fn pull_outcome_serializes_roundtrip() {
        let ok = PullOutcome::success(2);
        let s = serde_json::to_string(&ok).unwrap();
        let back: PullOutcome = serde_json::from_str(&s).unwrap();
        assert!(back.ok);
        assert_eq!(back.attempts, 2);

        let fail = PullOutcome::failure("ssh timed out".into(), 3);
        let back: PullOutcome =
            serde_json::from_str(&serde_json::to_string(&fail).unwrap()).unwrap();
        assert!(!back.ok);
        assert_eq!(back.error.as_deref(), Some("ssh timed out"));
        assert_eq!(back.attempts, 3);
    }

    #[test]
    fn parse_response_handles_error_envelope() {
        let err = error_envelope("authentication failed for machine 'x'");
        let buf = serde_json::to_vec(&err).unwrap();
        let r: anyhow::Result<PushResponse> = parse_response(&buf);
        assert!(r.is_err());
        assert!(
            r.unwrap_err().to_string().contains("authentication failed"),
            "actionable message survives"
        );
    }

    #[test]
    fn parse_response_passes_normal_envelope() {
        let resp = PushResponse {
            ok: true,
            error: None,
        };
        let buf = serde_json::to_vec(&resp).unwrap();
        let parsed: PushResponse = parse_response(&buf).unwrap();
        assert!(parsed.ok);
    }

    #[test]
    fn machine_status_tolerates_old_json() {
        let st: MachineStatus =
            serde_json::from_str(r#"{"online":true,"last_seen":1,"last_push":1}"#).unwrap();
        assert!(st.pulls.is_empty());
    }
}
