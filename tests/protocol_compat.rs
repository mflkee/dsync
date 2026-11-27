//! Совместимость с конвертами протокола v2: новые версии обязаны принимать
//! JSON, которые присылали/принимали старые клиенты и хабы.

use dsync::protocol::{
    error_envelope, parse_response, PushRequest, StatePullResponse, StatusResponse,
};

#[test]
fn v2_push_request_without_optional_fields() {
    // Клиент v2 до появления version/target/token — поля дефолтятся.
    let raw = r#"{
        "machine": "desktop",
        "timestamp": 1700000000,
        "projects": [{
            "name": "dotfiles",
            "path": "/home/u/dotfiles",
            "branch": "main",
            "dirty": false,
            "ahead": 0,
            "behind": 0,
            "commit_hash": "abc123",
            "last_commit_time": 1700000000
        }]
    }"#;
    let req: PushRequest = parse_response(raw.as_bytes()).unwrap();
    assert_eq!(req.machine, "desktop");
    assert_eq!(req.projects.len(), 1);
    assert_eq!(req.projects[0].url, "", "url defaults for old clients");
    assert!(req.token.is_empty());
    assert!(req.target.is_none());
    assert!(req.version.is_none());
}

#[test]
fn v2_status_without_hub_version() {
    // Хаб v2 до version-string не шлёт hub_version и иногда не шлёт pulls.
    let raw = r#"{
        "machines": {
            "notebook": {"online": true, "last_seen": 10, "last_push": 20}
        }
    }"#;
    let resp: StatusResponse = parse_response(raw.as_bytes()).unwrap();
    assert!(resp.hub_version.is_none());
    assert!(resp.machines["notebook"].pulls.is_empty());
    assert_eq!(resp.machines["notebook"].version, None);
}

#[test]
fn v2_state_pull_page_without_pagination_fields() {
    // Старый хаб отдавал всё сразу: next_seq/has_more отсутствуют.
    let raw = r#"{"items": [], "state_seq": 42}"#;
    let resp: StatePullResponse = parse_response(raw.as_bytes()).unwrap();
    assert_eq!(resp.state_seq, 42);
    assert_eq!(resp.next_seq, 0);
    assert!(!resp.has_more);
}

#[test]
fn v2_error_envelope_is_actionable() {
    let raw = serde_json::to_vec(&error_envelope("unsupported request")).unwrap();
    let err = parse_response::<StatusResponse>(&raw).unwrap_err();
    assert!(err.to_string().contains("unsupported request"));
}
