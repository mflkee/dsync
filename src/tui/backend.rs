//! Фоновый поток с собственным tokio runtime: общение с хабом по QUIC,
//! периодический status-poll и push/pull-задачи.
//!
//! Паттерн — как `worker.rs` в esp32-tui, но на tokio: UI-поток остаётся
//! синхронным (ratatui-цикл), всё сетевое крутится здесь и приходит в UI
//! событиями через crossbeam-канал.

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::config::Config;
use crate::protocol::{MachineStatus, ProjectState};

/// Команды из UI в backend (tokio mpsc: UI шлёт через `try_send` —
/// `blocking_send` паникует на рабочем потоке tokio).
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Обновить снимок статуса прямо сейчас.
    Poll,
    /// Отправить локальное состояние в хаб (все проекты).
    Push { target: Option<String> },
    /// Забрать состояние из хаба (SSH-pull проектов по всем машинам).
    Pull { target: Option<String> },
}

pub type CmdSender = tokio::sync::mpsc::Sender<Cmd>;

/// События из backend в UI.
#[derive(Debug)]
pub enum Event {
    /// Свежий снимок: машины + локальные проекты.
    Snapshot {
        machines: HashMap<String, MachineStatus>,
        projects: Vec<ProjectState>,
    },
    /// Строка в лог: level 0=info 1=ok 2=warn 3=err.
    Log { level: u8, text: String },
    /// Ошибка получения снимка (хаб недоступен и т.п.) — для дашборда.
    SnapshotError(String),
    /// Завершение фоновой задачи (push/pull).
    ActionDone { label: String, ok: bool, text: String },
}

/// Запускает backend-поток, возвращает (приёмник событий, отправитель команд).
pub fn spawn(cfg: Config) -> (Receiver<Event>, CmdSender) {
    let (ev_tx, ev_rx) = bounded(256);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(16);

    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime for dsync-tui backend");
        rt.block_on(run_backend(cfg, cmd_rx, ev_tx));
    });

    (ev_rx, cmd_tx)
}

async fn run_backend(cfg: Config, mut cmd_rx: tokio::sync::mpsc::Receiver<Cmd>, ev: Sender<Event>) {
    // Периодический status-poll: каждые 30 секунд держим экран свежим.
    let mut poll = tokio::time::interval(Duration::from_secs(30));
    poll.tick().await; // первый тик сразу — статус появляется мгновенно

    loop {
        tokio::select! {
            _ = poll.tick() => try_snapshot(&cfg, &ev).await,
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Poll) => try_snapshot(&cfg, &ev).await,
                Some(Cmd::Push { target }) => {
                    run_action(&cfg, &ev, "push", crate::client::push(cfg.clone(), target)).await;
                }
                Some(Cmd::Pull { target }) => {
                    run_action(&cfg, &ev, "pull", crate::client::pull(cfg.clone(), target)).await;
                }
                // Канал закрыт — UI-поток завершился (App упал). Выходим.
                None => break,
            }
        }
    }
}

/// Запрашивает статус у хаба + сканирует локальные проекты.
async fn try_snapshot(cfg: &Config, ev: &Sender<Event>) {
    let conn = match crate::client::connect::connect_with_retry(cfg).await {
        Ok(c) => c,
        Err(e) => {
            let text = format!("hub connect: {e}");
            let _ = ev.send(Event::Log { level: 3, text: text.clone() });
            let _ = ev.send(Event::SnapshotError(text));
            return;
        }
    };
    let req = crate::protocol::StatusRequest {
        machine: cfg.machine.name.clone(),
    };
    match crate::client::connect::send_status(&conn, &req).await {
        Ok(resp) => {
            let projects = cfg
                .projects
                .as_ref()
                .map(|p| crate::projects::status::scan(p).unwrap_or_default())
                .unwrap_or_default();
            let _ = ev.send(Event::Snapshot {
                machines: resp.machines,
                projects,
            });
        }
        Err(e) => {
            let text = format!("status: {e}");
            let _ = ev.send(Event::Log { level: 3, text: text.clone() });
            let _ = ev.send(Event::SnapshotError(text));
        }
    }
}

/// Выполняет push/pull и шлёт прогресс/результат в UI (по образцу
/// `spawn_task` в esp32-tui — UI не замирает, в шапке крутится спиннер).
async fn run_action<F>(_cfg: &Config, ev: &Sender<Event>, label: &str, fut: F)
where
    F: std::future::Future<Output = anyhow::Result<Vec<String>>>,
{
    let _ = ev.send(Event::Log {
        level: 0,
        text: format!("⏳ {label}…"),
    });
    match fut.await {
        Ok(lines) => {
            let _ = ev.send(Event::Log {
                level: 1,
                text: format!("✓ {label} done"),
            });
            for l in lines {
                let _ = ev.send(Event::Log { level: 1, text: l });
            }
            let _ = ev.send(Event::ActionDone {
                label: label.into(),
                ok: true,
                text: "ok".into(),
            });
        }
        Err(e) => {
            let _ = ev.send(Event::Log {
                level: 3,
                text: format!("✗ {label}: {e}"),
            });
            let _ = ev.send(Event::ActionDone {
                label: label.into(),
                ok: false,
                text: e.to_string(),
            });
        }
    }
}