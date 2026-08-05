use std::path::Path;
use std::process::{Command, Output};

use anyhow::Result;
use tracing::{info, warn};

/// Commit local changes, rebase onto origin and push, mirroring the v1
/// `sync_project_repo` behavior. Returns Ok(true) if anything was pushed.
pub fn commit_and_push(name: &str, path: &Path) -> Result<bool> {
    if !path.join(".git").exists() {
        info!("project {name}: not a git repo, skipping");
        return Ok(false);
    }

    let branch = git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let branch = branch.as_deref().unwrap_or("").trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        info!("project {name}: detached HEAD or empty, skipping");
        return Ok(false);
    }

    let dirty = git_output(path, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    if dirty {
        let add = run_git(path, &["add", "-A"]);
        if !add.status.success() {
            anyhow::bail!("{name}: git add failed: {}", stderr_of(&add));
        }
        let commit = run_git(path, &["commit", "-m", &format!("project sync: {name}")]);
        if !commit.status.success() {
            let err = stderr_of(&commit);
            if err.contains("Author identity unknown") || err.contains("tell me who you are") {
                info!("{name}: author not set, skipping commit");
            } else if err.contains("nothing to commit") {
                info!("{name}: nothing to commit");
            } else {
                anyhow::bail!("{name}: git commit failed: {err}");
            }
        } else {
            info!("{name}: committed changes");
        }
    }

    let has_remote = git_output(path, &["remote"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !has_remote {
        info!("{name}: no remote — push skipped");
        return Ok(false);
    }

    let fetch = run_git(path, &["fetch", "origin"]);
    if !fetch.status.success() {
        warn!("{name}: git fetch failed: {}", stderr_of(&fetch));
    }

    let (ahead, behind) = diverged(path, &branch);
    if behind > 0 {
        let pull = run_git(path, &["pull", "--rebase", "origin", &branch]);
        if !pull.status.success() {
            anyhow::bail!(
                "{name}: git pull --rebase failed: {}",
                stderr_of(&pull)
            );
        }
        info!("{name}: rebased onto origin/{branch}");
    }

    let (ahead_now, _) = diverged(path, &branch);
    let total_ahead = ahead_now + behind.saturating_sub(behind).max(0);
    let total_ahead = if ahead > 0 && behind == 0 { ahead } else { total_ahead };

    if total_ahead > 0 {
        let push = run_git(path, &["push", "origin", &branch]);
        if !push.status.success() {
            anyhow::bail!("{name}: git push failed: {}", stderr_of(&push));
        }
        info!("{name}: pushed {total_ahead} commits to origin/{branch}");
        return Ok(true);
    }

    Ok(false)
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

fn run_git(path: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap_or_else(|e| {
            panic!("failed to run git {args:?} in {}: {e}", path.display())
        })
}

fn stderr_of(o: &Output) -> String {
    let err = String::from_utf8_lossy(&o.stderr);
    let out = String::from_utf8_lossy(&o.stdout);
    format!("{}{}", out.trim(), err.trim()).trim().to_string()
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
