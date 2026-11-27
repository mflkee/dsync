
use super::*;

fn tmp_file(tag: &str, content: &str) -> (PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("dsync-cfg2-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, content).unwrap();
    (path, dir)
}

fn base_editor(path: PathBuf) -> ConfigEditor {
    let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    ConfigEditor {
        live_path: path.clone(),
        chezmoi_managed: false,
        chezmoi_source: None,
        cfg,
    }
}

const WITH_COMMENTS: &str = r#"# main comment
config_version = 1

[machine]
name = "testbox"

# hub stuff
[hub_connect]
address = "127.0.0.1:42069"

[unknown_section]
keep = "me"

[state]
tmux = true
tmux_restore = false
"#;

#[test]
fn add_project_preserves_comments_and_unknown_sections() {
    let (path, dir) = tmp_file("preserve", WITH_COMMENTS);
    let content = std::fs::read_to_string(&path).unwrap();
    let mut ed = base_editor(path.clone());
    let diff = ed
        .add_project(
            "proj-a",
            "/tmp/proj-a",
            Some("main"),
            &["desktop".to_string()],
            Some("echo hi"),
            false,
        )
        .unwrap();
    assert!(diff.iter().any(|l| l.contains("+ projects.proj-a added")));

    let out = std::fs::read_to_string(&path).unwrap();
    // Комментарии и неизвестная секция на месте.
    assert!(out.contains("# main comment"));
    assert!(out.contains("# hub stuff"));
    assert!(out.contains("[unknown_section]"));
    assert!(out.contains("keep = \"me\""));
    assert!(out.contains("[state]"));
    assert!(out.contains("[projects.proj-a]"));
    assert!(out.contains("path = \"/tmp/proj-a\""));

    // Весь документ остаётся валидным TOML.
    let back: Config = toml::from_str(&out).unwrap();
    assert!(back.projects.as_ref().unwrap().contains_key("proj-a"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = content;
}

#[test]
fn add_remove_roundtrip_keeps_unknown_content() {
    let (path, dir) = tmp_file("roundtrip", WITH_COMMENTS);
    let mut ed = base_editor(path.clone());
    ed.add_project("p1", "/tmp/p1", None, &[], None, false)
        .unwrap();
    ed.add_remote("m1", "100.89.0.1", 22, "user", false)
        .unwrap();
    ed.remove_project("p1", false).unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("[unknown_section]"));
    assert!(!out.contains("[projects.p1]"));
    assert!(out.contains("[remote.m1]"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn utf8_values_survive_add_and_remove() {
    let (path, dir) = tmp_file("utf8", WITH_COMMENTS);
    let mut ed = base_editor(path.clone());
    // Имя проекта и путь с кириллицей/эмодзи — значения пишутся и читаются
    // без потерь, документ остаётся валидным TOML.
    ed.add_project(
        "архив-2026",
        "/tmp/архив-📦",
        None,
        &["десктоп".into()],
        None,
        false,
    )
    .unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("архив-2026"));
    assert!(out.contains("/tmp/архив-📦"));
    let back: Config = toml::from_str(&out).unwrap();
    let p = back.projects.unwrap().get("архив-2026").unwrap().clone();
    assert_eq!(p.path.display().to_string(), "/tmp/архив-📦");
    assert_eq!(p.machines, vec!["десктоп".to_string()].into());
    ed.remove_project("архив-2026", false).unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(!out.contains("архив-2026"));
    assert!(out.contains("# main comment"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_field_survives_remove_remote() {
    let content = "\
# top comment survives\n\
[machine]\nname = \"x\"\n\
[remote.r1]\nhost = \"h\"\nport = 22\nuser = \"u\"\n\
[v2_new_section]\nx = 1\n";
    let (path, dir) = tmp_file("unknown-sect", content);
    let cfg: Config = toml::from_str(content).unwrap();
    let mut ed = ConfigEditor {
        live_path: path.clone(),
        chezmoi_managed: false,
        chezmoi_source: None,
        cfg,
    };
    ed.remove_remote("r1", false).unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(
        out.contains("[v2_new_section]"),
        "unknown section survives: {out}"
    );
    assert!(
        out.contains("# top comment survives"),
        "unrelated comment survives: {out}"
    );
    assert!(!out.contains("[remote.r1]"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn changed_keys_diff_reports_exact_keys() {
    let (path, dir) = tmp_file("diff", WITH_COMMENTS);
    let mut ed = base_editor(path.clone());
    ed.add_project("p1", "/tmp/p1", Some("main"), &["a".into()], None, false)
        .unwrap();
    let diff = ed
        .add_project(
            "p1",
            "/tmp/p2",
            Some("dev"),
            &["a".into(), "b".into()],
            Some("x"),
            false,
        )
        .unwrap();
    assert!(diff.iter().any(|l| l.contains("projects.p1.path")));
    assert!(diff.iter().any(|l| l.contains("projects.p1.branch")));
    assert!(diff.iter().any(|l| l.contains("projects.p1.machines")));
    assert!(diff.iter().any(|l| l.contains("projects.p1.post_pull")));
    assert!(!diff.iter().any(|l| l.contains("projects.other")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn state_patch_toggles_and_keeps_comments() {
    let (path, dir) = tmp_file("state", WITH_COMMENTS);
    let mut ed = base_editor(path.clone());
    let diff = ed
        .apply_state(Some(false), Some(true), Some(vec![]), false)
        .unwrap();
    assert!(diff.iter().any(|l| l.contains("state.tmux")));
    assert!(diff.iter().any(|l| l.contains("state.tmux_restore")));
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("# main comment"));
    assert!(out.contains("tmux = false"));
    assert!(out.contains("tmux_restore = true"));
    let back: Config = toml::from_str(&out).unwrap();
    let st = back.state.unwrap();
    assert!(!st.tmux);
    assert!(st.tmux_restore);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn state_opencode_list_sets_and_clears() {
    let (path, dir) = tmp_file("oc", WITH_COMMENTS);
    let mut ed = base_editor(path.clone());
    ed.apply_state(
        None,
        None,
        Some(vec!["~/projects/a".into(), "~/projects/b".into()]),
        false,
    )
    .unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("projects = [\"~/projects/a\", \"~/projects/b\"]"));
    // Очистка → ключ уходит (означает «все проекты»).
    ed.apply_state(None, None, Some(vec![]), false).unwrap();
    let out2 = std::fs::read_to_string(&path).unwrap();
    assert!(!out2.contains("projects = ["));
    let _ = std::fs::remove_dir_all(&dir);
}

// --- текстовый движок (Go-template шаблоны) ---

const GO_TMPL: &str = r#"# dsync
[machine]
name = "{{ if eq .chezmoi.hostname \"a\" }}desktop{{ else }}unknown{{ end }}"

[projects.dsync]
path = "/home/mflkee/projects/dsync"
branch = "main"
machines = ["notebook", "desktop"]

[state]
tmux = true
tmux_restore = false

{{ if eq .chezmoi.hostname "server" }}
[hub]
bind = "0.0.0.0:42069"
{{ end }}
"#;

#[test]
fn go_template_project_add_and_remove_preserves_blocks() {
    let (out, diff) = patch_template_text(
        GO_TMPL,
        &ConfigPatch::UpsertProject {
            name: "newp".into(),
            proj: ProjectConfig {
                path: "/tmp/newp".into(),
                branch: Some("main".into()),
                machines: Some(vec!["notebook".into()]),
                post_pull: None,
            },
        },
    )
    .unwrap();
    assert!(diff.iter().any(|l| l.contains("+ projects.newp added")));
    assert!(out.contains("[projects.newp]"));
    assert!(out.contains("path = \"/tmp/newp\""));
    // Go-блоки не тронуты
    assert!(out.contains("{{ if eq .chezmoi.hostname \"server\" }}"));
    assert!(out.contains("{{ end }}"));
    // Существующие проекты на месте
    assert!(out.contains("[projects.dsync]"));

    let (out2, diff2) = patch_template_text(
        &out,
        &ConfigPatch::RemoveProject {
            name: "newp".into(),
        },
    )
    .unwrap();
    assert!(diff2.iter().any(|l| l.contains("- projects.newp removed")));
    assert!(!out2.contains("[projects.newp]"));
    assert!(out2.contains("[projects.dsync]"));
    assert!(out2.contains("{{ end }}"));
}

#[test]
fn go_template_state_toggle() {
    let (out, diff) = patch_template_text(
        GO_TMPL,
        &ConfigPatch::State {
            tmux: Some(false),
            tmux_restore: None,
            opencode_projects: None,
        },
    )
    .unwrap();
    assert!(diff.iter().any(|l| l.contains("tmux")));
    assert!(out.contains("tmux = false"));
    assert!(out.contains("tmux_restore = false"));
    assert!(out.contains("{{ if eq .chezmoi.hostname \"server\" }}"));
}

#[test]
fn validation_errors_still_apply() {
    let (path, dir) = tmp_file("valid", "[machine]\nname = \"x\"\n");
    let mut ed = base_editor(path.clone());
    assert!(ed
        .add_project("", "/tmp/x", None, &[], None, false)
        .is_err());
    assert!(ed.add_remote("m2", "1.2.3.4", 0, "u", false).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_target_strips_home() {
    let home = crate::infra::paths::home_dir().unwrap();
    let live = home.join(".config/dsync/dsync/config.toml");
    let t = live_target(&live).unwrap();
    assert_eq!(t, ".config/dsync/dsync/config.toml");
}
