//! Build-time stamp: короткий git-хеш текущего коммита.
//!
//! Без него все сборки представляются как `0.1.0`, и дрейф бинарей по флоту
//! незаметен (в аудите 11.11.2026 у четырёх машин были четыре разные сборки
//! при одинаковой версии). `version_string()` добавляет хеш, а хаб копит
//! версии машин в `status`, так что расхождение видно сразу.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=DSYNC_GIT_SHA={sha}");
    // Пересобирать при смене HEAD: хук, файл которого меняется каждый коммит.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
