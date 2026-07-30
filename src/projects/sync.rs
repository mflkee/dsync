use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::Result;
use tracing::info;

use crate::config::ProjectConfig;

use super::status::expand_user_path;

pub fn pull_all(projects: &HashMap<String, ProjectConfig>) -> Result<()> {
    for (name, config) in projects {
        let path = expand_user_path(&config.path);
        if !path.join(".git").exists() {
            info!("project {name}: not a git repo, skipping");
            continue;
        }
        info!("pulling {name} at {}", path.display());
        git_pull(name, &path)?;
    }
    Ok(())
}

fn git_pull(name: &str, path: &Path) -> Result<()> {
    let branch = git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let branch = branch.as_deref().unwrap_or("").trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        info!("project {name}: detached HEAD or empty, skipping pull");
        return Ok(());
    }

    Command::new("git")
        .args(["stash", "push", "-m", "dsync-auto-stash"])
        .current_dir(path)
        .output()
        .ok();

    let out = Command::new("git")
        .args(["pull", "--rebase", "origin", &branch])
        .current_dir(path)
        .output()?;

    if out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        info!("{name}: pull ok — {stdout}{stderr}");
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        info!("{name}: pull failed — {stderr}");
        Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(path)
            .output()
            .ok();
        Command::new("git")
            .args(["stash", "pop"])
            .current_dir(path)
            .output()
            .ok();
    }

    Ok(())
}

pub fn sync_to_remote(
    projects: &HashMap<String, ProjectConfig>,
    remote_name: &str,
    machine_host: &str,
    machine_user: &str,
) -> Result<()> {
    for (name, config) in projects {
        let path = expand_user_path(&config.path);
        if !path.join(".git").exists() {
            continue;
        }

        let branch = config
            .branch
            .clone()
            .or_else(|| {
                git_output(&path, &["rev-parse", "--abbrev-ref", "HEAD"])
            })
            .unwrap_or_else(|| "main".to_string());

        let remote_url = config.remote.clone().unwrap_or_default();

        let script = format!(
            r#"export PATH="$HOME/.local/bin:$PATH"
cd {path}
git stash push -m "dsync-auto-$(date +%s)" 2>/dev/null || true
git pull --rebase {url} {branch} 2>&1 || echo GIT_CONFLICT"#,
            path = path.to_string_lossy(),
            url = &remote_url,
            branch = &branch,
        );

        info!("SSH sync {name} on {remote_name} ({machine_host})");
        let result = crate::ssh::client::exec(machine_host, 22, machine_user, &script)?;
        info!("{name}@{remote_name}: {result}");
    }
    Ok(())
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
