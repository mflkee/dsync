//! Интерактивный TUI поверх dsync: ratatui + crossterm.
//!
//! Главный цикл — ровно как в esp32-tui (init → draw → poll → drain событий),
//! только вместо `worker`-потоков с mpsc тут `backend`-поток с собственным
//! tokio runtime (см. `backend.rs`).

pub mod app;
pub mod backend;
pub mod cfg;
pub mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::config::Config;
use crate::tui::app::{App, ConfirmAction, Form, FormField};
use crate::tui::backend::Cmd;

pub fn run(cfg: Config) -> Result<()> {
    let _ = cfg;
    let mut terminal = ratatui::init();
    let res = run_loop(&mut terminal);
    ratatui::restore();
    res
}

fn run_loop(terminal: &mut DefaultTerminal) -> Result<()> {
    let editor = cfg::ConfigEditor::load()?;
    let summary = editor.summary();
    let (events, cmd_tx) = backend::spawn(editor);
    let mut app = App::new(events, cmd_tx, summary);
    app.log(
        0,
        "dsync TUI — [Tab] tabs  [p] push  [l] pull  [r] refresh  [q] quit".to_string(),
    );
    if app.cfg.chezmoi_managed {
        app.log(
            2,
            "config is chezmoi-managed: changes apply to the live file; update the template separately".to_string(),
        );
    }

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => handle_key(&mut app, key),
                _ => {}
            }
        }

        while let Ok(ev) = app.events.try_recv() {
            app.handle_event(ev);
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    if app.form.is_some() {
        handle_form_key(app, key);
        return;
    }

    use KeyCode::*;

    match key.code {
        Char('q') | Esc => app.should_quit = true,
        Tab => app.tab = app.tab.next(),
        BackTab => app.tab = app.tab.prev(),
        // push/pull: lowercase — все машины, P/L — выбранную (на Dashboard).
        Char('p') => app.run_action("push", Cmd::Push { target: None }),
        Char('l') => app.run_action("pull", Cmd::Pull { target: None }),
        Char('P') => {
            let target = app.machines.selected().map(|(n, _)| n.clone());
            app.run_action("push machine", Cmd::Push { target });
        }
        Char('L') => {
            let target = app.machines.selected().map(|(n, _)| n.clone());
            app.run_action("pull machine", Cmd::Pull { target });
        }
        Char('r') => match app.tab {
            app::Tab::Doctor => app.send(Cmd::Doctor),
            _ => app.send(Cmd::Poll),
        },
        Char('n') => match app.tab {
            app::Tab::Projects => open_add_project_form(app),
            app::Tab::Machines => open_add_remote_form(app),
            _ => {}
        },
        Char('d') => match app.tab {
            app::Tab::Projects => {
                if let Some(p) = app.projects.get(app.projects_sel.idx).cloned() {
                    app.form = Some(Form::confirm(
                        "Delete project?",
                        format!("{} — {}", p.name, p.path),
                        ConfirmAction::RemoveProject(p.name),
                    ));
                }
            }
            app::Tab::Machines => {
                if let Some(r) = app.cfg.remotes.get(app.remotes_sel.idx).cloned() {
                    app.form = Some(Form::confirm(
                        "Delete machine?",
                        format!("{} — {}@{}:{}", r.name, r.user, r.host, r.port),
                        ConfirmAction::RemoveRemote(r.name),
                    ));
                }
            }
            _ => {}
        },
        Enter if app.tab == app::Tab::Log => {
            app.logs.clear();
            app.log(0, "Log cleared".to_string());
        }
        Up | Char('k') => match app.tab {
            app::Tab::Dashboard => app.machines.sel_prev(),
            app::Tab::Projects => app.projects_sel.sel_prev(),
            app::Tab::Machines => app.remotes_sel.sel_prev(),
            _ => scroll_up(app),
        },
        Down | Char('j') => match app.tab {
            app::Tab::Dashboard => app.machines.sel_next(),
            app::Tab::Projects => app.projects_sel.sel_next(),
            app::Tab::Machines => app.remotes_sel.sel_next(),
            _ => scroll_down(app),
        },
        PageUp => scroll_up(app),
        PageDown => scroll_down(app),
        Home => match app.tab {
            app::Tab::Dashboard => app.machines.selected = 0,
            app::Tab::Projects => app.projects_sel.idx = 0,
            app::Tab::Machines => app.remotes_sel.idx = 0,
            _ => {}
        },
        End => match app.tab {
            app::Tab::Dashboard => {
                if !app.machines.list.is_empty() {
                    app.machines.selected = app.machines.list.len() - 1;
                }
            }
            app::Tab::Projects => {
                if app.projects_sel.len > 0 {
                    app.projects_sel.idx = app.projects_sel.len - 1;
                }
            }
            app::Tab::Machines => {
                if app.remotes_sel.len > 0 {
                    app.remotes_sel.idx = app.remotes_sel.len - 1;
                }
            }
            _ => {}
        },
        _ => {}
    }
}

fn scroll_up(app: &mut App) {
    match app.tab {
        app::Tab::Log => app.log_scroll = app.log_scroll.saturating_sub(1),
        app::Tab::Help => app.help_scroll = app.help_scroll.saturating_sub(1),
        app::Tab::Doctor => app.doctor_scroll = app.doctor_scroll.saturating_sub(1),
        _ => {}
    }
}

fn scroll_down(app: &mut App) {
    match app.tab {
        app::Tab::Log => app.log_scroll += 1,
        app::Tab::Help => app.help_scroll += 1,
        app::Tab::Doctor => app.doctor_scroll += 1,
        _ => {}
    }
}

// --- ФОРМЫ ---

impl Form {
    pub fn project(
        name: &str,
        path: &str,
        branch: &str,
        machines: &str,
        post_pull: &str,
    ) -> Self {
        Self {
            title: " Add project ",
            fields: vec![
                FormField::new("name", name),
                FormField::new("path", path),
                FormField::new("branch", branch),
                FormField::new("machines (comma)", machines),
                FormField::new("post_pull", post_pull),
            ],
            cursor: 0,
            action: None,
        }
    }

    pub fn remote(name: &str, host: &str, port: &str, user: &str) -> Self {
        Self {
            title: " Add machine ",
            fields: vec![
                FormField::new("name", name),
                FormField::new("host", host),
                FormField::new("port", port),
                FormField::new("user", user),
            ],
            cursor: 0,
            action: None,
        }
    }

    pub fn confirm(title: &'static str, detail: String, action: ConfirmAction) -> Self {
        Self {
            title,
            fields: vec![FormField::new("confirm", &detail)],
            cursor: 0,
            action: Some(action),
        }
    }
}

impl FormField {
    pub fn new(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.to_string(),
        }
    }
}

fn open_add_project_form(app: &mut App) {
    // Префилл из конфига (name/path/branch/machines/post_pull),
    // а не из скана git-статуса (там этих полей нет).
    let cfg_proj = app
        .cfg
        .projects
        .get(app.projects_sel.idx)
        .cloned();
    let (name, path, branch, mut machines, post) = match cfg_proj {
        Some(p) => (
            p.name,
            p.path,
            p.branch.unwrap_or_default(),
            p.machines.join(","),
            p.post_pull.unwrap_or_default(),
        ),
        None => (
            "myproject".to_string(),
            "/home/mflkee/projects/myproject".to_string(),
            "main".to_string(),
            String::new(),
            String::new(),
        ),
    };
    if machines.is_empty() {
        machines = app
            .cfg
            .remotes
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>()
            .join(",");
    }
    app.form = Some(Form::project(&name, &path, &branch, &machines, &post));
}

fn open_add_remote_form(app: &mut App) {
    let name = format!("machine-{}", app.cfg.remotes.len() + 1);
    app.form = Some(Form::remote(&name, "100.89.0.0", "22", "mflkee"));
}

/// Обрабатывает клавиши внутри формы (форма забирается из app, обрабатывается,
/// затем возвращается на место либо закрывается).
fn handle_form_key(app: &mut App, key: crossterm::event::KeyEvent) {
    let mut form = match app.form.take() {
        Some(f) => f,
        None => return,
    };
    let is_confirm = form.action.is_some();
    let last_idx = form.fields.len() - 1;

    use KeyCode::*;
    match key.code {
        Esc => return, // отмена: форма просто не возвращается на место
        Tab | Down | Char('j') if !is_confirm => {
            form.cursor = (form.cursor + 1) % form.fields.len();
        }
        BackTab | Up | Char('k') if !is_confirm => {
            form.cursor = (form.cursor + form.fields.len() - 1) % form.fields.len();
        }
        Enter => {
            // Подтверждение или последнее поле — сабмит.
            if is_confirm || form.cursor == last_idx {
                submit_form(app, &form);
                return; // форма закрыта
            }
            form.cursor += 1;
        }
        Backspace if !is_confirm => {
            form.fields[form.cursor].value.pop();
        }
        Char(c) if !is_confirm && !c.is_control() => {
            form.fields[form.cursor].value.push(c);
        }
        _ => {}
    }
    app.form = Some(form);
}

/// Отправить данные формы в backend.
fn submit_form(app: &mut App, form: &Form) {
    if let Some(action) = &form.action {
        match action {
            ConfirmAction::RemoveProject(name) => app.send(Cmd::RemoveProject { name: name.clone() }),
            ConfirmAction::RemoveRemote(name) => app.send(Cmd::RemoveRemote { name: name.clone() }),
        }
        return;
    }
    if form.title.contains("project") {
        let machines: Vec<String> = form.fields[3]
            .value
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        app.send(Cmd::AddProject {
            name: form.fields[0].value.clone(),
            path: form.fields[1].value.clone(),
            branch: Some(form.fields[2].value.clone()).filter(|s| !s.is_empty()),
            machines,
            post_pull: Some(form.fields[4].value.clone()).filter(|s| !s.is_empty()),
        });
    } else if form.title.contains("machine") {
        let port: u16 = form.fields[2].value.trim().parse().unwrap_or(22);
        app.send(Cmd::AddRemote {
            name: form.fields[0].value.clone(),
            host: form.fields[1].value.clone(),
            port,
            user: form.fields[3].value.clone(),
        });
    }
}