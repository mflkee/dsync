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
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::DefaultTerminal;

use crate::config::Config;
use crate::tui::app::{App, ConfirmAction, Form, FormField};
use crate::tui::backend::Cmd;

pub fn run(cfg: Config) -> Result<()> {
    let _ = cfg;
    // raw mode + alternate screen — как ratatui::init, но явно: нужен ещё
    // mouse capture для скролла колёсиком.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    // mouse capture может не получиться (минимальный терминал) — не беда:
    // идём дальше, скролл остаётся клавиатурным (spec: graceful degrade).
    let _ = execute!(stdout, EnterAlternateScreen, EnableMouseCapture);
    let mut terminal: DefaultTerminal = ratatui::Terminal::new(CrosstermBackend::new(stdout))?;

    // Паника до restore оставляла «сломанный» терминал (raw mode + alt screen):
    // hook восстанавливает терминал до печати backtrace.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    let res = run_loop(&mut terminal);
    restore_terminal();
    res
}

/// Выход из TUI: отключить mouse capture, покинуть alternate screen, raw mode off.
fn restore_terminal() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
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
        match &app.cfg.chezmoi_template {
            Some(t) => app.log(
                2,
                format!("config is chezmoi-managed: each save is confirmed and written to template {t} + chezmoi apply"),
            ),
            None => app.log(
                3,
                "config is chezmoi-managed but no source template found: config edits will be refused"
                    .to_string(),
            ),
        }
    }

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => handle_key(&mut app, key),
                TermEvent::Mouse(m) if app.form.is_none() && !app.show_pull_details => {
                    match m.kind {
                        MouseEventKind::ScrollDown => scroll_down(&mut app),
                        MouseEventKind::ScrollUp => scroll_up(&mut app),
                        _ => {}
                    }
                }
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
    // Оверлей «failed pulls»: [e]/[Esc] закрывают, [↑↓]/колесо скроллят.
    if app.show_pull_details {
        match key.code {
            KeyCode::Char('e') | KeyCode::Esc => app.show_pull_details = false,
            _ => {}
        }
        return;
    }

    if app.form.is_some() {
        handle_form_key(app, key);
        return;
    }

    use KeyCode::*;

    match key.code {
        Char('q') | Esc => app.should_quit = true,
        Tab => app.switch_tab(app.tab.next()),
        BackTab => app.switch_tab(app.tab.prev()),
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
            app::Tab::State => app.send(Cmd::StateStatus),
            _ => app.send(Cmd::Poll),
        },
        Char('e') => match app.tab {
            app::Tab::Dashboard | app::Tab::Machines => {
                app.show_pull_details = true;
                app.pull_details_scroll = 0;
            }
            _ => {}
        },
        Char('n') => match app.tab {
            app::Tab::Projects => open_add_project_form(app),
            app::Tab::Machines => open_add_remote_form(app),
            app::Tab::State => open_opencode_form(app),
            _ => {}
        },
        Char('t') if app.tab == app::Tab::State => {
            let new = !app.cfg.state.zellij;
            app.form = Some(Form::confirm(
                " toggle zellij ",
                format!("zellij sync: {} → {new}", app.cfg.state.zellij),
                ConfirmAction::StateToggle {
                    zellij: Some(new),
                    zellij_restore: None,
                },
            ));
        }
        Char('T') if app.tab == app::Tab::State => {
            let new = !app.cfg.state.zellij_restore;
            app.form = Some(Form::confirm(
                " toggle zellij_restore ",
                format!("zellij auto-restore: {} → {new}", app.cfg.state.zellij_restore),
                ConfirmAction::StateToggle {
                    zellij: None,
                    zellij_restore: Some(new),
                },
            ));
        }
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
            app::Tab::Machines if app.remotes_sel.len > 0 => {
                app.remotes_sel.idx = app.remotes_sel.len - 1;
            }
            _ => {}
        },
        _ => {}
    }
}

fn scroll_up(app: &mut App) {
    if app.show_pull_details {
        app.pull_details_scroll = app.pull_details_scroll.saturating_sub(1);
        return;
    }
    match app.tab {
        app::Tab::Log => app.log_scroll = app.log_scroll.saturating_sub(1),
        app::Tab::Help => app.help_scroll = app.help_scroll.saturating_sub(1),
        app::Tab::Doctor => app.doctor_scroll = app.doctor_scroll.saturating_sub(1),
        _ => {}
    }
}

fn scroll_down(app: &mut App) {
    if app.show_pull_details {
        app.pull_details_scroll += 1;
        return;
    }
    match app.tab {
        app::Tab::Log => app.log_scroll += 1,
        app::Tab::Help => app.help_scroll += 1,
        app::Tab::Doctor => app.doctor_scroll += 1,
        _ => {}
    }
}

// --- ФОРМЫ ---

impl Form {
    pub fn project(name: &str, path: &str, branch: &str, machines: &str, post_pull: &str) -> Self {
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
            fields: vec![FormField::new("confirm", detail)],
            cursor: 0,
            action: Some(action),
        }
    }

    /// Форма списка проектов opencode (одно поле, через запятую).
    pub fn opencode(list: &str) -> Self {
        Self {
            title: " opencode projects ",
            fields: vec![FormField::new("projects (comma-separated paths)", list)],
            cursor: 0,
            action: None,
        }
    }
}

fn open_opencode_form(app: &mut App) {
    let prefill = app
        .cfg
        .state
        .opencode_projects
        .as_ref()
        .map(|p| p.join(", "))
        .unwrap_or_default();
    app.form = Some(Form::opencode(&prefill));
}

fn open_add_project_form(app: &mut App) {
    // Префилл из конфига (name/path/branch/machines/post_pull),
    // а не из скана git-статуса (там этих полей нет).
    let cfg_proj = app.cfg.projects.get(app.projects_sel.idx).cloned();
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
/// затем возвращается на место либо закрывается). Полноценное редактирование:
/// курсор [←]/[→], Home/End, Backspace, Delete, Ctrl-U очистка поля.
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
        // Переключение поля — только Tab/BackTab/↑↓ (символы j/k теперь
        // вводятся как текст: раньше их тоже нельзя было напечатать в форме).
        Tab | Down if !is_confirm => {
            form.cursor = (form.cursor + 1) % form.fields.len();
        }
        BackTab | Up if !is_confirm => {
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
        Left if !is_confirm => {
            let f = &mut form.fields[form.cursor];
            f.cursor = f.cursor.saturating_sub(1);
        }
        Right if !is_confirm => {
            let f = &mut form.fields[form.cursor];
            f.cursor = (f.cursor + 1).min(f.char_len());
        }
        Home if !is_confirm => form.fields[form.cursor].cursor = 0,
        End if !is_confirm => form.fields[form.cursor].cursor = form.fields[form.cursor].char_len(),
        Backspace if !is_confirm => form.fields[form.cursor].backspace(),
        Delete if !is_confirm => form.fields[form.cursor].delete(),
        // Ctrl-U: очистить поле (crossterm шлёт Char('u') + CONTROL).
        Char('u') if !is_confirm && key.modifiers.contains(KeyModifiers::CONTROL) => {
            form.fields[form.cursor].clear();
        }
        Char(c) if !is_confirm && !c.is_control() => {
            form.fields[form.cursor].insert_char(c);
        }
        _ => {}
    }
    app.form = Some(form);
}

/// Отправить данные формы в backend. Конфиг-мутации на chezmoi-managed
/// конфиге уходят через шаблон (см. route_config_cmd).
fn submit_form(app: &mut App, form: &Form) {
    if let Some(action) = &form.action {
        match action.clone() {
            ConfirmAction::RemoveProject(name) => {
                route_config_cmd(app, Cmd::RemoveProject { name, use_template: false });
            }
            ConfirmAction::RemoveRemote(name) => {
                route_config_cmd(app, Cmd::RemoveRemote { name, use_template: false });
            }
            ConfirmAction::ChezmoiApply(mut cmd) => {
                set_use_template(&mut cmd, true);
                app.send(cmd);
            }
            ConfirmAction::StateToggle { zellij, zellij_restore } => {
                route_config_cmd(
                    app,
                    Cmd::SetState {
                        zellij,
                        zellij_restore,
                        opencode_projects: None,
                        use_template: false,
                    },
                );
            }
        }
        return;
    }
    let cmd = match form.title {
        " Add project " => Cmd::AddProject {
            name: form.fields[0].value.clone(),
            path: form.fields[1].value.clone(),
            branch: Some(form.fields[2].value.clone()).filter(|s| !s.is_empty()),
            machines: split_csv(&form.fields[3].value),
            post_pull: Some(form.fields[4].value.clone()).filter(|s| !s.is_empty()),
            use_template: false,
        },
        " Add machine " => {
            let port: u16 = form.fields[2].value.trim().parse().unwrap_or(22);
            Cmd::AddRemote {
                name: form.fields[0].value.clone(),
                host: form.fields[1].value.clone(),
                port,
                user: form.fields[3].value.clone(),
                use_template: false,
            }
        }
        " opencode projects " => Cmd::SetState {
            zellij: None,
            zellij_restore: None,
            opencode_projects: Some(split_csv(&form.fields[0].value)),
            use_template: false,
        },
        _ => return,
    };
    route_config_cmd(app, cmd);
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// Конфиг-мутация: на chezmoi-managed конфиге требует явного подтверждения
/// записи в шаблон + `chezmoi apply`; без шаблона — отказ с инструкцией.
fn route_config_cmd(app: &mut App, mut cmd: Cmd) {
    if !app.cfg.chezmoi_managed {
        app.send(cmd);
        return;
    }
    match app.cfg.chezmoi_template.clone() {
        Some(src) if std::path::Path::new(&src).exists() => {
            set_use_template(&mut cmd, true);
            app.log(
                2,
                format!("config is chezmoi-managed: confirm writing to template {src}"),
            );
            app.form = Some(Form::confirm(
                " chezmoi apply ",
                format!("Write change to template\n{src}\nand run `chezmoi apply`?"),
                ConfirmAction::ChezmoiApply(cmd),
            ));
        }
        _ => {
            app.log(
                3,
                format!(
                    "refusing config change: chezmoi-managed but no source template found — \
                     edit {} in ~/dotfiles manually",
                    app.cfg.config_path
                ),
            );
        }
    }
}

fn set_use_template(cmd: &mut Cmd, v: bool) {
    match cmd {
        Cmd::AddProject { use_template, .. }
        | Cmd::RemoveProject { use_template, .. }
        | Cmd::AddRemote { use_template, .. }
        | Cmd::RemoveRemote { use_template, .. }
        | Cmd::SetState { use_template, .. } => *use_template = v,
        _ => {}
    }
}
