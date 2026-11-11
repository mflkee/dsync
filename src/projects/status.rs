use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::Result;

use crate::config::ProjectConfig;
use crate::protocol::ProjectState;

pub fn scan(projects: &HashMap<String, ProjectConfig>) -> Result<Vec<ProjectState>> {
    let entries: Vec<(String, std::path::PathBuf)> = projects
        .iter()
        .map(|(name, config)| (name.clone(), expand_user_path(&config.path)))
        .collect();
    Ok(scan_parallel(entries))
}

/// Сканирует несколько репозиториев параллельно: на каждый репозиторий
/// приходится ~5 запусков `git`, и 29 авто-проектов последовательно занимали
/// 8–11 секунд на каждый `dsync push` (раз в 15 минут на каждой машине).
/// Порядок результата — по имени, чтобы вывод был детерминированным.
pub fn scan_parallel(entries: Vec<(String, std::path::PathBuf)>) -> Vec<ProjectState> {
    use std::sync::Mutex;
    let n = entries.len();
    if n <= 1 {
        let mut v: Vec<ProjectState> = entries
            .iter()
            .map(|(name, path)| scan_one(name, path))
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        return v;
    }
    let workers = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
        .clamp(2, 8);
    let out: Mutex<Vec<ProjectState>> = Mutex::new(Vec::with_capacity(n));
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..workers.min(n) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((name, path)) = entries.get(i) else {
                    break;
                };
                let st = scan_one(name, path);
                out.lock().unwrap().push(st);
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Каталоги git-репозиториев для авто-анонса (глубина 1 под `root`), которых
/// нет в `static_names` и в `excluded`.
pub fn discover_entries(
    root: &Path,
    static_names: &std::collections::HashSet<String>,
    excluded: &std::collections::HashSet<String>,
) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let dir = entry?.path();
        if !dir.is_dir() || !dir.join(".git").exists() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if static_names.contains(name) || excluded.contains(name) {
            continue;
        }
        out.push((name.to_string(), dir));
    }
    Ok(out)
}

/// Обходит `root` на глубину 1 и собирает состояния git-репозиториев
/// (каталогов с `.git`), которых нет в `static_names` (явные `[projects.*]`)
/// и в `excluded` (скретч/учёба из `[auto_projects] exclude`).
/// Используется авто-обнаружением новых проектов флота (`[auto_projects]`).
pub fn discover(
    root: &Path,
    static_names: &std::collections::HashSet<String>,
    excluded: &std::collections::HashSet<String>,
) -> Result<Vec<ProjectState>> {
    Ok(scan_parallel(discover_entries(
        root,
        static_names,
        excluded,
    )?))
}

pub fn scan_one(name: &str, path: &Path) -> ProjectState {
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
    let url = git_output(path, &["remote", "get-url", "origin"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    ProjectState {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        branch: branch.unwrap_or_default().trim().to_string(),
        dirty,
        ahead,
        behind,
        commit_hash,
        last_commit_time,
        url,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Чинит git-репозиторий без конфига user.name/email (CI-окружения).
    fn setup_git_ident(dir: &Path) {
        let _ = std::process::Command::new("git")
            .args([
                "-C",
                dir.to_str().unwrap(),
                "config",
                "user.email",
                "test@example.com",
            ])
            .status();
        let _ = std::process::Command::new("git")
            .args([
                "-C",
                dir.to_str().unwrap(),
                "config",
                "user.name",
                "dsync-tests",
            ])
            .status();
    }

    #[test]
    fn discover_finds_git_dirs_and_skips_others() {
        let base = std::env::temp_dir().join(format!("dsync-disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("alpha")).unwrap();
        std::fs::create_dir_all(base.join("beta")).unwrap();
        std::fs::create_dir_all(base.join("plain-dir")).unwrap();
        std::fs::create_dir_all(base.join("not-repo/file.txt")).unwrap();

        // alpha — полноценный git-репо с одним коммитом и remote.
        for d in ["alpha", "beta"] {
            std::process::Command::new("git")
                .args(["-C", base.join(d).to_str().unwrap(), "init", "-b", "main"])
                .status()
                .unwrap();
            setup_git_ident(&base.join(d));
            std::fs::write(base.join(d).join("file.txt"), "hello").unwrap();
            std::process::Command::new("git")
                .args(["-C", base.join(d).to_str().unwrap(), "add", "-A"])
                .status()
                .unwrap();
            std::process::Command::new("git")
                .args(["-C", base.join(d).to_str().unwrap(), "commit", "-m", "init"])
                .status()
                .unwrap();
        }
        std::process::Command::new("git")
            .args([
                "-C",
                base.join("alpha").to_str().unwrap(),
                "remote",
                "add",
                "origin",
                "git@github.com:mflkee/alpha.git",
            ])
            .status()
            .unwrap();

        let static_names = std::collections::HashSet::from(["beta".to_string()]);
        let excluded = std::collections::HashSet::from(["alpha".to_string()]);
        // alpha в exclude — не анонсируется вовсе.
        let states = discover(&base, &static_names, &excluded).unwrap();
        assert!(states.is_empty(), "excluded repo must not be announced");

        let states = discover(&base, &static_names, &std::collections::HashSet::new()).unwrap();
        assert_eq!(states.len(), 1, "beta excluded by static names");
        assert_eq!(states[0].name, "alpha");
        assert_eq!(states[0].branch, "main");
        assert_eq!(states[0].url, "git@github.com:mflkee/alpha.git");
        assert_eq!(states[0].path, base.join("alpha").to_string_lossy());

        // Без исключений — оба git-репо.
        let states = discover(
            &base,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        )
        .unwrap();
        let mut names: Vec<_> = states.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["alpha", "beta"]);

        let _ = std::fs::remove_dir_all(&base);
    }
}
