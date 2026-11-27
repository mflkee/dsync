use super::*;
use crate::protocol::{ProjectState, PullOutcome};

fn machine(name: &str, last_push: i64) -> MachineState {
    MachineState {
        name: name.to_string(),
        last_push,
        projects: Vec::<ProjectState>::new(),
        pulls: HashMap::new(),
        version: None,
    }
}

#[test]
fn update_machine_preserves_pulls() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let state = HubState::new(None, 30);
        state.update_machine(machine("desktop", 100)).await;
        state
            .record_pull(
                "desktop",
                "dotfiles",
                &PullOutcome::failure("ssh failed".into(), 1),
            )
            .await;
        state.update_machine(machine("desktop", 200)).await;
        let m = state.all_machines().await;
        let pulls = &m["desktop"].pulls;
        assert_eq!(pulls.len(), 1, "pull outcome survives push cycle");
        assert_eq!(pulls["dotfiles"].error.as_deref(), Some("ssh failed"));
    });
}

#[test]
fn record_pull_roundtrip_with_persistence() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("dsync-state-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = HubState::new(Some(dir.clone()), 30);
        state.update_machine(machine("desktop", 100)).await;
        state
            .record_pull("desktop", "dotfiles", &PullOutcome::success(1))
            .await;

        // Пересоздаём из того же data_dir — pulls должны прочитаться.
        let state2 = HubState::new(Some(dir.clone()), 30);
        let m = state2.all_machines().await;
        assert!(m["desktop"].pulls["dotfiles"].ok);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn concurrent_record_pull_saves_do_not_race() {
    // Несколько пулл-тасок финишируют разом: все record_pull сохраняют
    // в один machines.json, и без сериализации save()/уникального tmp
    // rename конкурентных сохранений ловил ENOENT.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("dsync-state-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = std::sync::Arc::new(HubState::new(Some(dir.clone()), 30));
        state.update_machine(machine("desktop", 100)).await;

        let mut tasks = Vec::new();
        for i in 0..10 {
            let state = state.clone();
            tasks.push(tokio::spawn(async move {
                state
                    .record_pull("desktop", &format!("p{i}"), &PullOutcome::success(1))
                    .await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }

        // Файл валиден и содержит все outcomes.
        let data = std::fs::read_to_string(dir.join("machines.json")).unwrap();
        let back: HashMap<String, MachineState> = serde_json::from_str(&data).unwrap();
        assert_eq!(back["desktop"].pulls.len(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn old_machines_json_without_pulls_loads() {
    let dir = std::env::temp_dir().join(format!("dsync-state-old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("machines.json"),
        r#"[{"name":"desktop","last_push":1,"projects":[]}]"#,
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let state = HubState::new(Some(dir.clone()), 30);
        let m = state.all_machines().await;
        assert!(m["desktop"].pulls.is_empty());
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prune_removes_only_stale_and_keeps_new() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let state = HubState::new(None, 30); // 30 дней
        let now = now_secs();
        state.update_machine(machine("recent", now - 10)).await;
        state
            .update_machine(machine("stale", now - (31 * 86400)))
            .await;
        state.update_machine(machine("fresh", 0)).await; // last_push == 0
        let removed = state.prune_stale().await;
        assert!(removed);
        let m = state.all_machines().await;
        assert!(m.contains_key("recent"));
        assert!(!m.contains_key("stale"));
        assert!(m.contains_key("fresh"), "last_push==0 never pruned");
    });
}

#[test]
fn custom_retention_window_honored() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let state = HubState::new(None, 1); // 1 день
        let now = now_secs();
        state
            .update_machine(machine("old", now - (2 * 86400)))
            .await;
        state.update_machine(machine("ok", now - 3600)).await;
        state.prune_stale().await;
        let m = state.all_machines().await;
        assert!(!m.contains_key("old"));
        assert!(m.contains_key("ok"));
    });
}

#[test]
fn zero_retention_disables_pruning() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let state = HubState::new(None, 0);
        let now = now_secs();
        state
            .update_machine(machine("ancient", now - (365 * 86400)))
            .await;
        assert!(!state.prune_stale().await);
        assert!(state.all_machines().await.contains_key("ancient"));
    });
}

#[test]
fn state_merge_and_since_roundtrip() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("dsync-st-items-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = HubState::new(Some(dir.clone()), 30);
        let items = vec![StateItem {
            channel: "opencode".into(),
            key: "ses_a".into(),
            updated: 100,
            data: r#"{"a":1}"#.into(),
            ..Default::default()
        }];
        assert_eq!(state.merge_state(items, "notebook").await, 1);

        // Другая машина видит элемент; автор — нет.
        let (got, cur) = state.state_since(0, "desktop").await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "ses_a");
        assert_eq!(got[0].origin, "notebook");
        assert_eq!(got[0].data, r#"{"a":1}"#);
        assert_eq!(cur, 1);
        assert!(state.state_since(0, "notebook").await.0.is_empty());

        // Персистентность: перечитываем с диска.
        let state2 = HubState::new(Some(dir.clone()), 30);
        let (got2, cur2) = state2.state_since(0, "desktop").await;
        assert_eq!(got2.len(), 1);
        assert_eq!(cur2, 1);

        // LWW: устаревшее чужое обновление игнорируется.
        let stale = vec![StateItem {
            channel: "opencode".into(),
            key: "ses_a".into(),
            updated: 50,
            data: "OLD".into(),
            ..Default::default()
        }];
        assert_eq!(
            state2.merge_state(stale, "desktop").await,
            1,
            "stale ignored"
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

async fn push_item(
    state: &HubState,
    channel: &str,
    key: &str,
    updated: i64,
    origin: &str,
    data: &str,
) -> i64 {
    state
        .merge_state(
            vec![StateItem {
                channel: channel.into(),
                key: key.into(),
                updated,
                data: data.into(),
                ..Default::default()
            }],
            origin,
        )
        .await
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsync-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn legacy_tmux_latest_item_is_rejected() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tmp_dir("legacytmux");
        let state = HubState::new(Some(dir.clone()), 30);
        // Старый клиент пишет общий `latest` — игнорируем.
        assert_eq!(
            push_item(&state, "tmux", "latest", 1, "notebook", "layout").await,
            0,
            "legacy shared tmux item is not stored"
        );
        // Актуальный клиент — ключ = имя машины.
        assert_eq!(
            push_item(&state, "tmux", "notebook", 2, "notebook", "layout").await,
            1
        );
        let (items, ..) = state.state_since_page(0, "desktop", 0, None).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].key, "notebook");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn state_pull_pages_by_bytes_and_reports_cursor() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tmp_dir("page");
        let state = HubState::new(Some(dir.clone()), 30);
        for i in 1..=3 {
            push_item(
                &state,
                "opencode",
                &format!("k{i}"),
                100 + i,
                "notebook",
                &"x".repeat(100),
            )
            .await;
        }

        let (items, cur, next, more) = state.state_since_page(0, "desktop", 250, None).await;
        assert_eq!(items.len(), 2, "two 100-byte payloads fit into 250");
        assert_eq!(cur, 3, "hub seq is reported in full");
        assert_eq!(next, 2, "cursor at the last included item");
        assert!(more, "third item remains");

        let (items2, _, next2, more2) = state.state_since_page(next, "desktop", 250, None).await;
        assert_eq!(items2.len(), 1);
        assert_eq!(items2[0].key, "k3");
        assert!(!more2);
        assert_eq!(next2, 3);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn state_pull_sends_oversized_item_alone_to_make_progress() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tmp_dir("oversize");
        let state = HubState::new(Some(dir.clone()), 30);
        push_item(&state, "opencode", "big", 1, "notebook", &"x".repeat(500)).await;

        let (items, _, next, more) = state.state_since_page(0, "desktop", 10, None).await;
        assert_eq!(items.len(), 1, "item bigger than the page limit is sent");
        assert!(!more, "no endless loop on an oversized item");
        assert_eq!(next, 1);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn state_pull_filters_channels_the_machine_does_not_want() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tmp_dir("channels");
        let state = HubState::new(Some(dir.clone()), 30);
        push_item(&state, "tmux", "notebook", 1, "notebook", "layout").await;
        push_item(&state, "opencode", "ses_a", 2, "notebook", "{}").await;

        let want = vec!["tmux".to_string()];
        let (items, _, next, more) = state.state_since_page(0, "desktop", 0, Some(&want)).await;
        assert_eq!(items.len(), 1, "only the requested channel is sent");
        assert_eq!(items[0].channel, "tmux");
        assert_eq!(next, 2, "cursor still advanced past the skipped item");
        assert!(!more);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn state_pull_skips_own_origin_but_advances_cursor() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tmp_dir("origin");
        let state = HubState::new(Some(dir.clone()), 30);
        push_item(&state, "opencode", "mine", 1, "notebook", "{}").await;
        push_item(&state, "tmux", "notebook", 2, "notebook", "layout").await;
        push_item(&state, "opencode", "theirs", 3, "desktop", "{}").await;

        let (items, _, next, _) = state.state_since_page(0, "notebook", 0, None).await;
        assert_eq!(
            items.len(),
            2,
            "own opencode is not echoed; own tmux snapshot is (backup restore)"
        );
        assert_eq!(items[0].channel, "tmux");
        assert_eq!(items[0].key, "notebook");
        assert_eq!(items[1].key, "theirs");
        assert_eq!(next, 3, "cursor moved past the own opencode item");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn prune_state_channels_drops_items_and_payload_dirs() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tmp_dir("prune");
        let state = HubState::new(Some(dir.clone()), 30);
        push_item(&state, "tmux", "notebook", 1, "notebook", "layout").await;
        push_item(&state, "zellij", "latest", 2, "notebook", "layout").await;

        let keep = std::collections::HashSet::from(["tmux".to_string()]);
        assert_eq!(state.prune_state_channels(&keep).await, 1);

        let status = state.state_status().await.unwrap();
        assert_eq!(status.channels.len(), 1);
        assert_eq!(status.channels[0].channel, "tmux");
        assert!(status.channels[0].bytes > 0, "payload size reported");
        assert!(!dir.join("state/zellij").exists(), "payload dir removed");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn cleanup_stale_temp_files_removes_only_tmp_files() {
    let dir = tmp_dir("tmpfiles");
    std::fs::create_dir_all(dir.join("state")).unwrap();
    std::fs::write(dir.join("machines.json"), "{}").unwrap();
    std::fs::write(dir.join("machines.json.tmp-1-2"), "").unwrap();
    std::fs::write(dir.join("state/index.json.tmp-3-4"), "").unwrap();

    let removed = cleanup_stale_temp_files(&dir, std::time::Duration::ZERO);
    assert_eq!(removed, 2);
    assert!(dir.join("machines.json").exists(), "real state kept");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deployed_commits_persist_and_dedupe() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tmp_dir("deployed");
        let state = HubState::new(Some(dir.clone()), 30);
        assert!(state.deployed_commit("notebook", "dsync").await.is_none());
        state.record_deployed("notebook", "dsync", "abc123").await;
        assert_eq!(
            state.deployed_commit("notebook", "dsync").await.as_deref(),
            Some("abc123")
        );

        // Перечитываем с диска.
        let state2 = HubState::new(Some(dir.clone()), 30);
        assert_eq!(
            state2.deployed_commit("notebook", "dsync").await.as_deref(),
            Some("abc123")
        );
        // Пустой commit не затирает запись.
        state2.record_deployed("notebook", "dsync", "").await;
        assert_eq!(
            state2.deployed_commit("notebook", "dsync").await.as_deref(),
            Some("abc123")
        );
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn state_status_summarizes_channels_with_error() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("dsync-st-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = HubState::new(Some(dir.clone()), 30);

        // tmux: 1 элемент, updated в секундах
        state
            .merge_state(
                vec![StateItem {
                    channel: "tmux".into(),
                    key: "notebook".into(),
                    updated: now_secs() - 120,
                    data: "{}".into(),
                    ..Default::default()
                }],
                "notebook",
            )
            .await;
        // opencode: 2 элемента, updated в мс
        state
            .merge_state(
                vec![
                    StateItem {
                        channel: "opencode".into(),
                        key: "ses_a".into(),
                        updated: 1_776_000_000_000,
                        data: "{}".into(),
                        ..Default::default()
                    },
                    StateItem {
                        channel: "opencode".into(),
                        key: "ses_b".into(),
                        updated: 1_776_000_100_000,
                        data: "{}".into(),
                        ..Default::default()
                    },
                ],
                "desktop",
            )
            .await;
        state
            .record_state_error("opencode", "export truncated (CLI bug)".to_string())
            .await;

        let resp = state.state_status().await.unwrap();
        assert_eq!(resp.channels.len(), 2);

        let tmux = resp.channels.iter().find(|c| c.channel == "tmux").unwrap();
        assert_eq!(tmux.item_count, 1);
        assert_eq!(tmux.last_origin, "notebook");
        // seconds не делятся
        assert!(tmux.last_updated > 0 && tmux.last_updated < 1_000_000_000_000);
        assert!(tmux.error.is_none());

        let oc = resp
            .channels
            .iter()
            .find(|c| c.channel == "opencode")
            .unwrap();
        assert_eq!(oc.item_count, 2);
        assert_eq!(oc.last_origin, "desktop");
        // ms → s
        assert_eq!(oc.last_updated, 1_776_000_100);
        assert_eq!(oc.error.as_deref(), Some("export truncated (CLI bug)"));

        let _ = std::fs::remove_dir_all(&dir);
    });
}
