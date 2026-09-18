//! Интерактивный TUI поверх dsync: ratatui + crossterm.
//!
//! Главный цикл — ровно как в esp32-tui (init → draw → poll → drain событий),
//! только вместо `worker`-потоков с mpsc тут `backend`-поток с собственным
//! tokio runtime (см. `backend.rs`).

pub mod app;
pub mod backend;
pub mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::config::Config;

pub fn run(cfg: Config) -> Result<()> {
    let mut terminal = ratatui::init();
    let res = run_loop(&mut terminal, cfg);
    ratatui::restore();
    res
}

fn run_loop(terminal: &mut DefaultTerminal, cfg: Config) -> Result<()> {
    let (events, cmd_tx) = backend::spawn(cfg);
    let mut app = app::App::new(events, cmd_tx);
    app.log(
        0,
        "dsync TUI started — [p] push  [l] pull  [r] refresh  [q] quit".to_string(),
    );

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => handle_key(&mut app, key),
                _ => {}
            }
        }

        // Дреним события из backend, как esp32-tui дренит events_rx.
        while let Ok(ev) = app.events.try_recv() {
            app.handle_event(ev);
        }
    }
    Ok(())
}

fn handle_key(app: &mut app::App, key: crossterm::event::KeyEvent) {
    use KeyCode::*;

    match key.code {
        Char('q') | Esc => app.should_quit = true,
        Tab => app.tab = app.tab.next(),
        BackTab => app.tab = app.tab.prev(),
        Char('p') => app.run_action("push", backend::Cmd::Push { target: None }),
        Char('l') => app.run_action("pull", backend::Cmd::Pull { target: None }),
        Char('r') => app.send(backend::Cmd::Poll),
        Up | Char('k') => app.machines.sel_prev(),
        Down | Char('j') => app.machines.sel_next(),
        Enter if app.tab == app::Tab::Log => {
            app.logs.clear();
            app.log(0, "Log cleared".to_string());
        }
        _ => {}
    }
}