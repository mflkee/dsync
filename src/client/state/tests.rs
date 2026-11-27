use super::*;

/// Тесты, трогающие `DSYNC_STATE_PATH`, идут последовательно: переменная
/// окружения общая на процесс, а cargo гоняет тесты параллельно.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn item(channel: &str, key: &str, updated: i64, data_len: usize) -> StateItem {
    StateItem {
        channel: channel.into(),
        key: key.into(),
        updated,
        data: "x".repeat(data_len),
        ..Default::default()
    }
}

#[test]
fn sanitize_strips_paths_and_specials() {
    assert_eq!(sanitize_filename("/etc/foo/bar.txt"), "bar.txt");
    assert_eq!(sanitize_filename("a/b:c"), "b_c");
    assert_eq!(sanitize_filename("ses_123-abc"), "ses_123-abc");
}

#[test]
fn batches_split_by_size() {
    let items = vec![
        item("opencode", "a", 1, 10),
        item("opencode", "b", 1, MAX_BATCH_BYTES + 10),
        item("opencode", "c", 1, 10),
    ];
    let b = batches(items);
    assert_eq!(b.len(), 3, "oversized item gets its own batch");
    assert_eq!(b[0][0].key, "a");
    assert_eq!(b[1][0].key, "b");
    assert_eq!(b[2][0].key, "c");
}

#[test]
fn batches_single_when_small() {
    let b = batches(vec![item("tmux", "latest", 1, 10)]);
    assert_eq!(b.len(), 1);
}

#[test]
fn content_hash_stable_and_positive() {
    assert_eq!(content_hash("abc"), content_hash("abc"));
    assert_ne!(content_hash("abc"), content_hash("abd"));
    assert!(content_hash("abc") >= 0);
}

fn state_cfg() -> Config {
    Config {
        config_version: 1,
        machine: crate::config::MachineConfig {
            name: "t".into(),
            ssh_key: None,
        },
        hub: None,
        hub_connect: None,
        projects: None,
        auto_projects: None,
        remote: None,
        capture: None,
        state: Some(crate::config::StateConfig {
            tmux: true,
            tmux_restore: false,
            opencode: None,
            retry_limit: 3,
        }),
    }
}

#[test]
fn collect_with_uses_channels_and_filters_wanted() {
    struct Fake(bool);
    impl StateChannel for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn wanted(&self, _cfg: &Config) -> bool {
            self.0
        }
        fn accepts(&self, _cfg: &Config, _item: &StateItem) -> bool {
            true
        }
        fn collect(&self, _cfg: &Config, _idx: &StateIndex) -> Vec<StateItem> {
            vec![StateItem {
                channel: "fake".into(),
                key: "k".into(),
                updated: 1,
                data: "x".into(),
                ..Default::default()
            }]
        }
        fn apply(&self, _cfg: &Config, _item: &StateItem) -> Result<String> {
            Ok("ok".into())
        }
    }

    let cfg = state_cfg();
    let idx = StateIndex::default();
    let on = Fake(true);
    let off = Fake(false);
    let out = collect_with(&[&on, &off], &cfg, &idx);
    assert_eq!(out.len(), 1, "only wanted channels are collected");
    assert_eq!(out[0].channel, "fake");
}

#[test]
fn collect_without_state_config_is_empty() {
    let cfg = Config {
        config_version: 1,
        machine: crate::config::MachineConfig {
            name: "t".into(),
            ssh_key: None,
        },
        hub: None,
        hub_connect: None,
        projects: None,
        auto_projects: None,
        remote: None,
        capture: None,
        state: None,
    };
    assert!(collect(&cfg).is_empty());
    assert!(apply(&cfg, vec![item("tmux", "latest", 1, 3)])
        .messages
        .is_empty());
}

#[test]
fn disabled_opencode_channel_is_skipped() {
    // `[state.opencode] projects = []` — явное отключение канала: машина
    // без opencode V2 не должна пытаться применять чужие сессии.
    let mut cfg = state_cfg();
    let st = cfg.state.as_mut().unwrap();
    st.tmux = false;
    st.opencode = Some(crate::config::OpencodeStateConfig {
        projects: Some(Vec::new()),
    });
    let report = apply(&cfg, vec![item("opencode", "ses_a", 1, 2)]);
    assert!(report.applied.is_empty());
    assert!(report.failed.is_empty(), "не пытаемся применять вовсе");
    assert_eq!(report.skipped, 1);
    assert!(
        wanted_channels(&cfg).is_empty(),
        "хабу не запрашиваем каналы"
    );
}

#[test]
fn prune_unwanted_index_drops_disabled_channel_state() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("dsync-state-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));

    let mut idx = StateIndex::default();
    let bad = StateItem {
        seq: 5,
        ..item("opencode", "ses_x", 100, 1)
    };
    for _ in 0..3 {
        record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3);
    }
    idx.failures.insert(
        "tmux:latest".into(),
        Failure {
            attempts: 1,
            updated: 1,
            seq: 6,
        },
    );
    idx.save().unwrap();

    prune_unwanted_index(&["tmux".to_string()]);
    let h = health();
    assert!(h.dead.is_empty(), "disabled channel dead letters dropped");
    assert_eq!(h.failures.len(), 1, "enabled channel kept");
    assert!(
        h.failures.iter().any(|(k, _)| k == "tmux:latest"),
        "enabled channel kept"
    );

    std::env::remove_var("DSYNC_STATE_PATH");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_skips_already_applied_items() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Ретрай страницы состояния не должен повторно писать tmux-снапшот
    // (и запускать restore): уже применённый элемент пропускается.
    let dir = std::env::temp_dir().join(format!("dsync-state-skip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));

    let cfg = state_cfg();
    let tmux = item("tmux", "t", 42, 3);
    mark_synced(std::slice::from_ref(&tmux));

    let report = apply(&cfg, vec![tmux]);
    assert!(report.applied.is_empty(), "nothing re-applied");
    assert_eq!(report.skipped, 1);
    assert!(report.failed.is_empty());

    std::env::remove_var("DSYNC_STATE_PATH");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn foreign_tmux_snapshot_is_never_applied() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Раскладка другой машины не должна ни писаться в локальный
    // resurrect-каталог, ни (тем более) восстанавливаться в живой tmux.
    let dir = std::env::temp_dir().join(format!("dsync-state-foreign-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));

    let mut cfg = state_cfg();
    cfg.state.as_mut().unwrap().tmux_restore = true;
    let foreign = item("tmux", "mkair-server-tmn", 7, 4);
    let report = apply(&cfg, vec![foreign]);
    assert!(report.applied.is_empty(), "foreign layout is skipped");
    assert!(report.failed.is_empty());
    assert_eq!(report.skipped, 1);
    assert!(report.messages.is_empty(), "no restore was triggered");

    std::env::remove_var("DSYNC_STATE_PATH");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn record_failures_moves_item_to_dead_letter_after_limit() {
    let mut idx = StateIndex::default();
    let bad = StateItem {
        seq: 100,
        ..item("opencode", "ses_x", 100, 1)
    };
    assert_eq!(
        record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3),
        Some(100),
        "first failure keeps the cursor before the item"
    );
    assert_eq!(
        record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3),
        Some(100)
    );
    assert_eq!(
        record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3),
        None,
        "after the limit the channel is no longer blocked"
    );
    assert_eq!(idx.dead.len(), 1, "dead-letter recorded");
    assert!(idx.failures.is_empty(), "counter cleared after dead-letter");
}

#[test]
fn record_failures_returns_earliest_pending_seq() {
    let mut idx = StateIndex::default();
    let a = StateItem {
        seq: 30,
        ..item("opencode", "ses_a", 1, 1)
    };
    let b = StateItem {
        seq: 10,
        ..item("opencode", "ses_b", 1, 1)
    };
    let blocked = record_failures(&mut idx, &[], &[a, b], 5);
    assert_eq!(blocked, Some(10), "cursor must stop before the earliest");
    assert_eq!(idx.failures.len(), 2);
}

#[test]
fn new_version_resets_failure_counter() {
    let mut idx = StateIndex::default();
    let old = item("opencode", "ses_x", 100, 1);
    for _ in 0..2 {
        record_failures(&mut idx, &[], std::slice::from_ref(&old), 3);
    }
    assert_eq!(idx.failures["opencode:ses_x"].attempts, 2);
    let new = item("opencode", "ses_x", 200, 1);
    record_failures(&mut idx, &[], std::slice::from_ref(&new), 3);
    assert_eq!(
        idx.failures["opencode:ses_x"].attempts, 1,
        "new updated — fresh budget"
    );
}

#[test]
fn retry_dead_rolls_seq_back_and_clears() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("dsync-state-retry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));

    let mut idx = StateIndex::default();
    let bad = StateItem {
        seq: 77,
        ..item("opencode", "ses_x", 100, 1)
    };
    for _ in 0..3 {
        record_failures(&mut idx, &[], std::slice::from_ref(&bad), 3);
    }
    idx.seq = 120;
    idx.save().unwrap();
    assert_eq!(health().dead.len(), 1);

    assert_eq!(retry_dead(None), 1);
    let h = health();
    assert!(h.dead.is_empty());
    assert_eq!(h.seq, 76, "seq rolled back before the failed item");

    std::env::remove_var("DSYNC_STATE_PATH");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn index_roundtrip_marks_and_skips() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("dsync-state-idx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DSYNC_STATE_PATH", dir.join("state.json"));
    let it = item("opencode", "ses_a", 100, 1);
    assert!(!StateIndex::load().has(&it));
    mark_synced(std::slice::from_ref(&it));
    assert!(StateIndex::load().has(&it));
    let older = item("opencode", "ses_a", 50, 1);
    assert!(StateIndex::load().has(&older), "older counts as known");
    let newer = item("opencode", "ses_a", 200, 1);
    assert!(!StateIndex::load().has(&newer));
    set_seq(7);
    assert_eq!(get_seq(), 7);
    set_seq(3); // не откатываем
    assert_eq!(get_seq(), 7);
    std::env::remove_var("DSYNC_STATE_PATH");
    let _ = std::fs::remove_dir_all(&dir);
}
