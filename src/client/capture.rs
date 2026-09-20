//! Захват live-правок dotfiles в их git-репозитории.
//!
//! Проблема: `dsync push` коммитит только изменения внутри git-репозиториев
//! (projects). Правка живого файла вроде `~/.zshrc` или `~/.config/nvim/…`
//! в git не видна — её нужно сначала перенести в исходник (`chezmoi re-add`).
//!
//! Решение:
//! * **Периодически** (`dsync push` / `dsync watch`) сканируем живые файлы
//!   (по умолчанию — все `chezmoi managed`), сравниваем sha256 с прошлым
//!   слепком и `re-add`-им изменившиеся. Так ловятся правки откуда угодно:
//!   nvim, bash, sed, скрипты — отдельный nvim-хук не обязателен.
//! * **Мгновенно** — `dsync capture <path>`, который дёргает nvim-хук на
//!   сохранение, чтобы флот получил изменение сразу, не дожидаясь цикла.
//!
//! Под капотом dsync гоняет chezmoi (`source-path` / `re-add` / `apply`) —
//! ровно так, как README и обещает («drives chezmoi under the hood»): сам
//! пользователь работает с dsync и с обычными файлами, а не с `chezmoi edit`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::config::{Config, ProjectConfig};

use super::push::{push, push_core};

// ---------------------------------------------------------------------------
// Внешний API клиента
// ---------------------------------------------------------------------------

/// Мгновенный захват конкретных путей (live-файл или правка исходника) +
/// пуш флота. Пустой список аргументов — это обычный `push`.
pub async fn capture(cfg: Config, paths: Vec<PathBuf>) -> Result<Vec<String>> {
    if paths.is_empty() {
        return push(cfg, None).await;
    }

    let mut msgs: Vec<String> = Vec::new();
    let mut state = CaptureState::load();
    for p in &paths {
        let abs = to_abs(p);
        match capture_one(&cfg, &abs, &mut state) {
            Ok(Some(m)) => msgs.push(m),
            Ok(None) => info!("capture {}: nothing to capture", abs.display()),
            Err(e) => warn!("capture {}: {e:#}", abs.display()),
        }
    }
    if let Err(e) = state.save() {
        warn!("capture state save failed: {e:#}");
    }

    if msgs.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = push_core(cfg, None).await?;
    out.splice(0..0, msgs);
    Ok(out)
}

/// Периодический захват: re-add всех изменившихся с прошлого раза live-файлов.
/// Не падает — ошибки только логируются, чтобы не сломать обычный push.
pub fn capture_changed(cfg: &Config) -> Vec<String> {
    let mut msgs = Vec::new();
    let mut state = CaptureState::load();
    let candidates = watch_candidates(cfg);
    let mut seen = HashSet::with_capacity(candidates.len());
    for live in candidates {
        seen.insert(live.to_string_lossy().to_string());
        match capture_one(cfg, &live, &mut state) {
            Ok(Some(m)) => msgs.push(m),
            Ok(None) => {}
            Err(e) => warn!("capture {}: {e:#}", live.display()),
        }
    }
    // Забываем файлы, которых больше нет в списке.
    state.map.retain(|k, _| seen.contains(k));
    if let Err(e) = state.save() {
        warn!("capture state save failed: {e:#}");
    }
    msgs
}

// ---------------------------------------------------------------------------
// Одиночный файл
// ---------------------------------------------------------------------------

/// Обрабатывает один путь. Возвращает Some(сообщение), если что-то сделано.
fn capture_one(cfg: &Config, path: &Path, state: &mut CaptureState) -> Result<Option<String>> {
    // Правка внутри репозитория проекта (например ~/dotfiles/dot_zshrc) —
    // это исходник, его не re-add-им, а применяем локально, чтобы живой файл
    // обновился сразу здесь же.
    if inside_project(cfg, path) {
        return apply_source_edit(cfg, path);
    }

    if !path.is_file() {
        return Ok(None);
    }
    if is_excluded(cfg, path) {
        return Ok(None);
    }
    // Вложенные git-репозитории внутри ~ (например projects/*) не трогаем.
    if let Some(dir) = path.parent() {
        if dir.join(".git").exists() {
            return Ok(None);
        }
    }

    let key = path.to_string_lossy().to_string();
    let hash = file_hash(path)?;
    if state.map.get(&key) == Some(&hash) {
        return Ok(None); // не менялся с прошлого захвата
    }

    let src = chezmoi_source(path).context("chezmoi source-path")?;
    let Some(src) = src else {
        return Ok(None); // не под управлением chezmoi
    };
    if src.ends_with(".tmpl") {
        // Живой файл — результат рендера шаблона: re-add перезапишет шаблон
        // литеральным содержимым, такие правки делаем в исходнике.
        return Ok(None);
    }
    if project_containing(cfg, &src).is_none() {
        return Ok(None); // исходник вне настроенных проектов
    }

    let out = Command::new(chezmoi_path())
        .args(["re-add", &path.to_string_lossy()])
        .output()?;
    if !out.status.success() {
        return Err(anyhow::anyhow!(
            "chezmoi re-add failed: {}",
            stderr_of(&out)
        ));
    }
    state.map.insert(key, hash);
    info!("captured {} -> {}", path.display(), src.display());
    Ok(Some(format!("captured {}", file_label(path))))
}

/// Правка исходника (файл внутри репо проекта): локальный `chezmoi apply`
/// для соответствующего живого файла. Возвращает None для не-chezmoi файлов
/// (доки, скрипты) и для служебных сущностей (run_*, .chezmoidata, …).
fn apply_source_edit(cfg: &Config, src: &Path) -> Result<Option<String>> {
    let Some(live) = src_to_live(cfg, src) else {
        return Ok(None);
    };
    // Round-trip проверка: chezmoi source-path(live) должен вернуть тот же src.
    match chezmoi_source(&live)? {
        Some(got) if got == src => {}
        _ => return Ok(None),
    }
    let out = Command::new(chezmoi_path())
        .args(["apply", "--force", &live.to_string_lossy()])
        .output()?;
    if !out.status.success() {
        return Err(anyhow::anyhow!(
            "chezmoi apply failed: {}",
            stderr_of(&out)
        ));
    }
    info!("applied {} -> {}", src.display(), live.display());
    Ok(Some(format!("applied {}", file_label(&live))))
}

// ---------------------------------------------------------------------------
// Кандидаты на периодический захват
// ---------------------------------------------------------------------------

fn watch_candidates(cfg: &Config) -> Vec<PathBuf> {
    let Some(capture) = &cfg.capture else {
        return managed_files();
    };
    if let Some(watch) = &capture.watch {
        let mut out = Vec::new();
        for w in watch {
            let p = to_abs(Path::new(w));
            if p.is_dir() {
                collect_files(&p, &mut out);
            } else if p.is_file() {
                out.push(p);
            }
        }
        return out;
    }
    managed_files()
}

/// По умолчанию — все файлы под управлением chezmoi (любое изменение ловится).
fn managed_files() -> Vec<PathBuf> {
    let Ok(out) = Command::new(chezmoi_path())
        .args(["managed", "--include", "files"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| to_abs(Path::new(l)))
        .collect()
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) != Some(".git") {
                subdirs.push(p);
            }
        } else if p.is_file() {
            out.push(p);
        }
    }
    for d in subdirs {
        collect_files(&d, out);
    }
}

fn is_excluded(cfg: &Config, path: &Path) -> bool {
    let mut excludes: Vec<PathBuf> = vec![
        home_join(".config/ghostty/themes"),
        home_join(".config/kitty/themes"),
        home_join(".config/dsync"), // конфиг dsync рождается из шаблона
    ];
    if let Some(cap) = &cfg.capture {
        if let Some(list) = &cap.exclude {
            for e in list {
                excludes.push(to_abs(Path::new(e)));
            }
        }
    }
    excludes.iter().any(|e| path.starts_with(e))
}

// ---------------------------------------------------------------------------
// Маппинг исходник -> живой файл (по соглашениям имён chezmoi)
// ---------------------------------------------------------------------------

fn src_to_live(cfg: &Config, src: &Path) -> Option<PathBuf> {
    let project = project_containing(cfg, src)?;
    let base = project_path(&project);
    let rel = src.strip_prefix(&base).ok()?;
    let rel_str = rel.to_string_lossy();
    // Скрипты и внутренняя кухня chezmoi — не обычные файлы, наружу не применяем.
    if rel_str.starts_with("run_")
        || rel_str.starts_with("once_")
        || rel_str.starts_with("commit_")
        || rel_str.starts_with(".chezmoi")
        || rel_str.starts_with(".chezmoidata")
        || rel_str.starts_with(".git")
    {
        return None;
    }
    let transf = transform_rel(rel);
    dirs::home_dir().map(|h| h.join(transf))
}

/// Трансформация относительного пути исходника chezmoi в относительный путь
/// живого файла (соглашения имён: `dot_`, `private_`, `executable_`, `.tmpl`).
fn transform_rel(rel: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in rel.components() {
        let mut name = comp.as_os_str().to_string_lossy().to_string();
        if let Some(s) = name.strip_suffix(".tmpl") {
            name = s.to_string();
        }
        if let Some(s) = name.strip_prefix("private_") {
            name = s.to_string();
        }
        if let Some(s) = name.strip_prefix("executable_") {
            name = s.to_string();
        }
        if let Some(s) = name.strip_prefix("dot_") {
            name = format!(".{s}");
        }
        out.push(name);
    }
    out
}

// ---------------------------------------------------------------------------
// Проекты
// ---------------------------------------------------------------------------

fn inside_project(cfg: &Config, path: &Path) -> bool {
    project_containing(cfg, path).is_some()
}

fn project_path(project: &ProjectConfig) -> PathBuf {
    crate::projects::status::expand_user_path(&project.path)
}

fn project_containing(cfg: &Config, path: &Path) -> Option<ProjectConfig> {
    let projects = cfg.projects.as_ref()?;
    for pc in projects.values() {
        let base = project_path(pc);
        if path.starts_with(&base) {
            return Some(pc.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Слепок хешей
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CaptureState {
    /// live-path -> sha256
    map: HashMap<String, String>,
}

impl CaptureState {
    fn path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp/dsync"))
            .join("dsync/capture-state.json")
    }

    fn load() -> Self {
        let map = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { map }
    }

    fn save(&self) -> Result<()> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(&self.map)?)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Утилиты
// ---------------------------------------------------------------------------

/// Находит chezmoi: иначе в GUI-окружениях PATH может не содержать /usr/sbin.
fn chezmoi_path() -> &'static str {
    static PATH: OnceLock<&'static str> = OnceLock::new();
    *PATH.get_or_init(|| {
        for cand in [
            "/usr/sbin/chezmoi",
            "/usr/bin/chezmoi",
            "/usr/local/bin/chezmoi",
        ] {
            if Path::new(cand).exists() {
                return Box::leak(cand.to_string().into_boxed_str());
            }
        }
        Box::leak("chezmoi".to_string().into_boxed_str())
    })
}

fn chezmoi_source(live: &Path) -> Result<Option<PathBuf>> {
    let out = Command::new(chezmoi_path())
        .args(["source-path", live.to_string_lossy().as_ref()])
        .output()?;
    if !out.status.success() {
        return Ok(None);
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let first = s.lines().next().map(str::trim).unwrap_or("");
    if first.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(first)))
    }
}

fn file_hash(path: &Path) -> Result<String> {
    let data = std::fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&data);
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    Ok(s)
}

fn file_label(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = path.strip_prefix(&home) {
            return rel.to_string_lossy().to_string();
        }
    }
    path.to_string_lossy().to_string()
}

fn home_join(p: &str) -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(p))
        .unwrap_or_else(|| PathBuf::from(p))
}

fn to_abs(p: &Path) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p.to_owned();
    };
    if s.starts_with("~/") {
        home_join(&s[2..])
    } else if s.starts_with('/') {
        p.to_owned()
    } else {
        home_join(s)
    }
}

fn stderr_of(out: &std::process::Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    format!("{}{}", stdout.trim(), err.trim())
}

// ---------------------------------------------------------------------------
// Тесты
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_common_sources() {
        assert_eq!(
            transform_rel(Path::new("dot_zshrc")),
            PathBuf::from(".zshrc")
        );
        assert_eq!(
            transform_rel(Path::new("dot_config/nvim/init.lua")),
            PathBuf::from(".config/nvim/init.lua")
        );
        assert_eq!(
            transform_rel(Path::new("private_dot_ssh/config")),
            PathBuf::from(".ssh/config")
        );
        assert_eq!(
            transform_rel(Path::new("dot_local/bin/executable_dot-autosync")),
            PathBuf::from(".local/bin/dot-autosync")
        );
        assert_eq!(
            transform_rel(Path::new("dot_gtkrc-2.0.tmpl")),
            PathBuf::from(".gtkrc-2.0")
        );
        assert_eq!(
            transform_rel(Path::new("Pictures/wallpaper.png")),
            PathBuf::from("Pictures/wallpaper.png")
        );
    }

    #[test]
    fn transform_skips_peculiar_components() {
        // exec-скрипт с dot_ в имени после префикса
        assert_eq!(
            transform_rel(Path::new("dot_local/bin/executable_dot-autosync")),
            PathBuf::from(".local/bin/dot-autosync")
        );
        // обычный файл, начинающийся с не-dot_ префикса, не должен ломаться
        assert_eq!(
            transform_rel(Path::new("dot_config/kitty/kitty.conf")),
            PathBuf::from(".config/kitty/kitty.conf")
        );
    }

    #[test]
    fn to_abs_expands_tilde_and_home() {
        let home = dirs::home_dir().expect("home");
        assert_eq!(to_abs(Path::new("~/zshrc")), home.join("zshrc"));
        assert_eq!(to_abs(Path::new(".zshrc")), home.join(".zshrc"));
        assert_eq!(to_abs(Path::new("/etc/hosts")), PathBuf::from("/etc/hosts"));
    }
}