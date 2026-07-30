use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::Result;

use crate::config::ProjectConfig;
use crate::protocol::ProjectState;

pub fn scan(projects: &HashMap<String, ProjectConfig>) -> Result<Vec<ProjectState>> {
    let mut states = Vec::new();
    for (name, config) in projects {
        let path = expand_user_path(&config.path);
        let state = scan_one(name, &path);
        states.push(state);
    }
    Ok(states)
}

fn scan_one(name: &str, path: &Path) -> ProjectState {
    let branch = git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = git_output(path, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let (ahead, behind) = diverged(path, branch.as_deref().unwrap_or(""));
    let commit_hash = git_output(path, &["rev-parse", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let last_commit_time = git_output(path, &["log", "-1", "--format=%ct"])
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0);

    ProjectState {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        branch: branch.unwrap_or_default().trim().to_string(),
        dirty,
        ahead,
        behind,
        commit_hash,
        last_commit_time,
    }
}

fn git_output(path: &Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
}

fn diverged(path: &Path, branch: &str) -> (usize, usize) {
    let ref_str = format!("HEAD...origin/{branch}");
    let out = git_output(path, &["rev-list", "--count", "--left-right", &ref_str]);
    match out {
        Some(s) => {
            let trimmed = s.trim();
            if let Some((a, b)) = trimmed.split_once('\t') {
                let a = a.parse().unwrap_or(0);
                let b = b.parse().unwrap_or(0);
                (a, b)
            } else if let Some((a, b)) = trimmed.split_once(' ') {
                let a = a.parse().unwrap_or(0);
                let b = b.parse().unwrap_or(0);
                (a, b)
            } else {
                (0, 0)
            }
        }
        None => (0, 0),
    }
}

pub fn expand_user_path(p: &Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p.to_owned()
}
