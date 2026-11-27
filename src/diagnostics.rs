//! Машиночитаемые контракты `--json` для статуса, диагностики и state-индекса.
//!
//! Это публичный контракт для веб-панели и скриптов: поля добавлять можно,
//! удалять/переименовывать — только с ростом `SCHEMA_VERSION`.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::protocol::StatusResponse;

/// Версия JSON-схемы диагностических команд.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub label: String,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Check {
    pub fn new(id: impl Into<String>, label: impl Into<String>, status: CheckStatus) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status,
            detail: None,
            error: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub client_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_version: Option<String>,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn new(checks: Vec<Check>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            client_version: crate::protocol::version_string(),
            hub_version: None,
            checks,
        }
    }

    /// Есть ли проваленные проверки (влияет на код возврата).
    pub fn has_failures(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Fail)
    }

    pub fn to_json_pretty(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullReport {
    pub ok: bool,
    pub attempts: u32,
    pub finished_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineReport {
    pub online: bool,
    pub last_seen: i64,
    pub last_push: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub pulls: BTreeMap<String, PullReport>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateSummary {
    pub seq: i64,
    pub applied: usize,
    pub failing: usize,
    pub dead_letter: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub schema_version: u32,
    pub client_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_version: Option<String>,
    pub machines: BTreeMap<String, MachineReport>,
    pub local_state: StateSummary,
}

impl StatusReport {
    pub fn from_protocol(resp: StatusResponse, local_state: StateSummary) -> Self {
        let machines = resp
            .machines
            .into_iter()
            .map(|(name, m)| {
                let pulls = m
                    .pulls
                    .into_iter()
                    .map(|(project, o)| {
                        (
                            project,
                            PullReport {
                                ok: o.ok,
                                attempts: o.attempts,
                                finished_at: o.finished_at,
                                error: o.error,
                            },
                        )
                    })
                    .collect();
                (
                    name,
                    MachineReport {
                        online: m.online,
                        last_seen: m.last_seen,
                        last_push: m.last_push,
                        version: m.version,
                        pulls,
                    },
                )
            })
            .collect();
        Self {
            schema_version: SCHEMA_VERSION,
            client_version: crate::protocol::version_string(),
            hub_version: resp.hub_version,
            machines,
            local_state,
        }
    }

    pub fn to_json_pretty(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureReport {
    pub key: String,
    pub attempts: u32,
    pub updated: i64,
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadReport {
    pub key: String,
    pub updated: i64,
    pub seq: i64,
    pub error: String,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateReport {
    pub schema_version: u32,
    pub seq: i64,
    pub applied: usize,
    pub failures: Vec<FailureReport>,
    pub dead_letters: Vec<DeadReport>,
}

impl StateReport {
    pub fn to_json_pretty(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MachineStatus, PullOutcome};

    #[test]
    fn status_report_shape() {
        let mut resp = StatusResponse {
            machines: Default::default(),
            hub_version: Some("0.1.0+abc (proto 2)".into()),
        };
        resp.machines.insert(
            "notebook".into(),
            MachineStatus {
                online: true,
                last_seen: 10,
                last_push: 20,
                pulls: [(
                    "dotfiles".to_string(),
                    PullOutcome {
                        ok: false,
                        error: Some("boom".into()),
                        attempts: 2,
                        finished_at: 30,
                    },
                )]
                .into_iter()
                .collect(),
                version: Some("0.1.0+abc (proto 2)".into()),
            },
        );
        let report = StatusReport::from_protocol(
            resp,
            StateSummary {
                seq: 5,
                applied: 3,
                failing: 1,
                dead_letter: 0,
            },
        );
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        assert_eq!(v["machines"]["notebook"]["online"], true);
        assert_eq!(v["machines"]["notebook"]["pulls"]["dotfiles"]["ok"], false);
        assert_eq!(v["local_state"]["seq"], 5);
        assert!(v.get("token").is_none());
    }

    #[test]
    fn doctor_exit_code_depends_only_on_fail() {
        let warn = DoctorReport::new(vec![Check::new("a", "a", CheckStatus::Warn)]);
        assert!(!warn.has_failures());
        let fail = DoctorReport::new(vec![Check::new("b", "b", CheckStatus::Fail)]);
        assert!(fail.has_failures());
    }

    #[test]
    fn empty_state_report_is_valid() {
        let report = StateReport {
            schema_version: SCHEMA_VERSION,
            seq: 0,
            applied: 0,
            failures: Vec::new(),
            dead_letters: Vec::new(),
        };
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["seq"], 0);
        assert_eq!(v["failures"].as_array().unwrap().len(), 0);
        assert_eq!(v["dead_letters"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn state_report_sorted_and_metadata_only() {
        let report = StateReport {
            schema_version: SCHEMA_VERSION,
            seq: 7,
            applied: 2,
            failures: vec![FailureReport {
                key: "tmux:notebook".into(),
                attempts: 1,
                updated: 1,
                seq: 3,
            }],
            dead_letters: vec![DeadReport {
                key: "opencode:ses_a".into(),
                updated: 2,
                seq: 4,
                error: "import failed".into(),
                at: 5,
            }],
        };
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        assert_eq!(v["dead_letters"][0]["key"], "opencode:ses_a");
        assert!(v["dead_letters"][0].get("data").is_none());
    }
}
