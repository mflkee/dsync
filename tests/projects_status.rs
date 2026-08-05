use std::collections::HashMap;
use std::process::Command;

use dsync::config::ProjectConfig;
use dsync::projects::status;

fn init_git_repo(dir: &std::path::Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "test"])
        .current_dir(dir)
        .output()
        .unwrap();
    std::fs::write(dir.join("file.txt"), b"hello").unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir)
        .output()
        .unwrap();
}

#[test]
fn test_scan_clean_repo() {
    let dir = std::env::temp_dir().join("dsync-test-clean");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);

    let mut projects = HashMap::new();
    projects.insert(
        "test-project".to_string(),
        ProjectConfig {
            path: dir.clone(),
            branch: None,
            machines: None,
            post_pull: None,
        },
    );

    let states = status::scan(&projects).unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].name, "test-project");
    assert_eq!(states[0].dirty, false);
    assert_eq!(states[0].branch, "main");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_scan_dirty_repo() {
    let dir = std::env::temp_dir().join("dsync-test-dirty");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);

    std::fs::write(dir.join("dirty.txt"), b"modified").unwrap();

    let mut projects = HashMap::new();
    projects.insert(
        "test".to_string(),
        ProjectConfig {
            path: dir.clone(),
            branch: None,
            machines: None,
            post_pull: None,
        },
    );

    let states = status::scan(&projects).unwrap();
    assert_eq!(states[0].dirty, true);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_scan_nonexistent_path() {
    let dir = std::env::temp_dir().join("dsync-test-nonexistent");
    let _ = std::fs::remove_dir_all(&dir);

    let mut projects = HashMap::new();
    projects.insert(
        "gone".to_string(),
        ProjectConfig {
            path: dir.clone(),
            branch: None,
            machines: None,
            post_pull: None,
        },
    );

    let states = status::scan(&projects).unwrap();
    assert_eq!(states[0].branch, "");
    assert_eq!(states[0].dirty, false);
    assert_eq!(states[0].commit_hash, "");
}
