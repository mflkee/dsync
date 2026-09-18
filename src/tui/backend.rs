//! Фоновый поток с собственным tokio runtime: общение с хабом по QUIC,
//! периодический status-poll, push/pull, доктор, добавление проектов/машин.
//!
//! Паттерн — как `worker.rs` в esp32-tui, но на tokio: UI-поток остаётся
//! синхронным (ratatui-цикл), всё сетевое крутится здесь и приходит в UI
//! событиями через crossbeam-канал.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};

use super::cfg::{ConfigEditor, CfgSummary};
use crate::config::Config;
use crate::protocol::MachineStatus;

/// Команды из UI в backend (tokio mpsc: UI шлёт через `try_send`).
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Обновить снимок статуса прямо сейчас.
    Poll,
    /// Отправить локальное состояние в хаб (все проекты).
    Push { target: Option<String> },
    /// Забрать состояние из хаба (SSH-pull проектов).
    Pull { target: Option<String> },
    /// Запустить доктора.
    Doctor,
    /// Добавить проект в конфиг и перечитать.
    AddProject {
        name: String,
        path: String,
        branch: Option<String>,
        machines: Vec<String>,
        post_pull: Option<String>,
    },
    /// Удалить проект из конфига.
    RemoveProject { name: String },
    /// Добавить remote-машину в конфиг.
    AddRemote {
        name: String,
        host: String,
        port: u16,
        user: String,
    },
    /// Удалить remote-машину из конфига.
    RemoveRemote { name: String },
}

pub type CmdSender = tokio::sync::mpsc::Sender<Cmd>;

/// События из backend в UI.
#[derive(Debug)]
pub enum Event {
    /// Свежий снимок: машины + локальные проекты + статус хаба.
    Snapshot {
        machines: HashMap<String, MachineStatus>,
        projects: Vec<crate::protocol::ProjectState>,
        hub_ok: bool,
    },
    /// Строка в лог: level 0=info 1=ok 2=warn 3=err.
    Log { level: u8, text: String },
    /// Ошибка получения снимка (хаб недоступен и т.п.) — для дашборда.
    SnapshotError(String),
    /// Снимок запущен (опрос хаба начался) — для индикатора «⟳ refresh…».
    Refreshing,
    /// Завершение фоновой задачи (push/pull).
    ActionDone { label: String, ok: bool, text: String },
    /// Результаты доктора.
    Doctor(Vec<CheckItem>),
    /// Конфиг изменился — перечитай снимок.
    ConfigChanged { summary: CfgSummary },
}

#[derive(Debug, Clone)]
pub struct CheckItem {
    pub label: String,
    pub level: u8, // 0=ok 1=warn 2=skip
    pub detail: String,
}

/// Запускает backend-поток, возвращает (приёмник событий, отправитель команд).
pub fn spawn(editor: ConfigEditor) -> (Receiver<Event>, CmdSender) {
    let (ev_tx, ev_rx) = bounded(256);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(16);

    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime for dsync-tui backend");
        rt.block_on(run_backend(editor, cmd_rx, ev_tx));
    });

    (ev_rx, cmd_tx)
}

async fn run_backend(
    mut editor: ConfigEditor,
    mut cmd_rx: tokio::sync::mpsc::Receiver<Cmd>,
    ev: Sender<Event>,
) {
    // Периодический status-poll: каждые 30 секунд.
    let mut poll = tokio::time::interval(Duration::from_secs(30));
    poll.tick().await; // первый тик сразу

    // Локальные проекты показываем мгновенно, не дожидаясь ответа хаба.
    local_snapshot(&editor.cfg, &ev).await;

    // Снимок хаба — фоновая задача, чтобы команды (push/pull/doctor)
    // не ждали ~30с ретраев connect при упавшем хабе.
    let snapshot_in_flight = Arc::new(AtomicBool::new(false));
    // Последний успешный список машин: при падении хаба и при изменении
    // конфига показанный список не обнуляется (раньше события слали
    // пустой HashMap и дашборд терял машины до следующего poll).
    let last_machines =
        Arc::new(std::sync::Mutex::new(HashMap::<String, MachineStatus>::new()));

    loop {
        tokio::select! {
            _ = poll.tick() => spawn_snapshot(&editor, &ev, &snapshot_in_flight, &last_machines),
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Poll) => spawn_snapshot(&editor, &ev, &snapshot_in_flight, &last_machines),
                Some(Cmd::Push { target }) => {
                    let cfg = editor.cfg.clone();
                    let ev2 = ev.clone();
                    tokio::spawn(async move {
                        run_action(&ev2, "push", crate::client::push(cfg, target)).await;
                    });
                }
                Some(Cmd::Pull { target }) => {
                    let cfg = editor.cfg.clone();
                    let ev2 = ev.clone();
                    tokio::spawn(async move {
                        run_action(&ev2, "pull", crate::client::pull(cfg, target)).await;
                    });
                }
                Some(Cmd::Doctor) => {
                    let cfg = editor.cfg.clone();
                    let info = editor.summary();
                    let ev2 = ev.clone();
                    tokio::spawn(async move { run_doctor(cfg, info, ev2).await; });
                }
                Some(Cmd::AddProject { name, path, branch, machines, post_pull }) => {
                    let res = editor.add_project(
                        &name, &path, branch.as_deref(), &machines, post_pull.as_deref(),
                    );
                    match res {
                        Ok(()) => {
                            let _ = ev.send(Event::Log { level: 1, text: format!("project {name:?} added") });
                            let _ = ev.send(Event::ConfigChanged { summary: editor.summary() });
                            // пересканируем проекты; список машин не трогаем
                            let cfg2 = editor.cfg.clone();
                            let projects = cfg2.projects.as_ref()
                                .map(|p| crate::projects::status::scan(p).unwrap_or_default())
                                .unwrap_or_default();
                            let machines = last_machines.lock().unwrap_or_else(|p| p.into_inner()).clone();
                            let _ = ev.send(Event::Snapshot { machines, projects, hub_ok: false });
                        }
                        Err(e) => { let _ = ev.send(Event::Log { level: 3, text: format!("add project: {e}") }); }
                    }
                }
                Some(Cmd::RemoveProject { name }) => {
                    match editor.remove_project(&name) {
                        Ok(()) => {
                            let _ = ev.send(Event::Log { level: 1, text: format!("project {name:?} removed") });
                            let _ = ev.send(Event::ConfigChanged { summary: editor.summary() });
                            let cfg2 = editor.cfg.clone();
                            let projects = cfg2.projects.as_ref()
                                .map(|p| crate::projects::status::scan(p).unwrap_or_default())
                                .unwrap_or_default();
                            let machines = last_machines.lock().unwrap_or_else(|p| p.into_inner()).clone();
                            let _ = ev.send(Event::Snapshot { machines, projects, hub_ok: false });
                        }
                        Err(e) => { let _ = ev.send(Event::Log { level: 3, text: format!("remove project: {e}") }); }
                    }
                }
                Some(Cmd::AddRemote { name, host, port, user }) => {
                    match editor.add_remote(&name, &host, port, &user) {
                        Ok(()) => {
                            let _ = ev.send(Event::Log { level: 1, text: format!("remote {name:?} added ({host}:{port})") });
                            let _ = ev.send(Event::ConfigChanged { summary: editor.summary() });
                        }
                        Err(e) => { let _ = ev.send(Event::Log { level: 3, text: format!("add remote: {e}") }); }
                    }
                }
                Some(Cmd::RemoveRemote { name }) => {
                    match editor.remove_remote(&name) {
                        Ok(()) => {
                            let _ = ev.send(Event::Log { level: 1, text: format!("remote {name:?} removed") });
                            let _ = ev.send(Event::ConfigChanged { summary: editor.summary() });
                        }
                        Err(e) => { let _ = ev.send(Event::Log { level: 3, text: format!("remove remote: {e}") }); }
                    }
                }
                None => break,
            }
        }
    }
}

/// Фоновая задача снимка хаба (одновременно не больше одной).
fn spawn_snapshot(
    editor: &ConfigEditor,
    ev: &Sender<Event>,
    in_flight: &Arc<AtomicBool>,
    last: &Arc<std::sync::Mutex<HashMap<String, MachineStatus>>>,
) {
    if !in_flight.swap(true, Ordering::SeqCst) {
        let cfg = editor.cfg.clone();
        let ev2 = ev.clone();
        let flag = in_flight.clone();
        let last2 = last.clone();
        tokio::spawn(async move {
            try_snapshot(cfg, ev2, last2).await;
            flag.store(false, Ordering::SeqCst);
        });
    }
}

/// Мгновенный снимок локальных проектов (без хаба).
async fn local_snapshot(cfg: &Config, ev: &Sender<Event>) {
    let projects = scan_projects(cfg);
    let _ = ev.send(Event::Snapshot {
        machines: HashMap::new(),
        projects,
        hub_ok: false,
    });
}

fn scan_projects(cfg: &Config) -> Vec<crate::protocol::ProjectState> {
    cfg.projects
        .as_ref()
        .map(|p| crate::projects::status::scan(p).unwrap_or_default())
        .unwrap_or_default()
}

/// Запрашивает статус у хаба + сканирует локальные проекты.
/// Проекты сканируются всегда (даже если хаб лёг) — дашборд показывает
/// конфиг-сводку и список проектов вне зависимости от хаба.
async fn try_snapshot(
    cfg: Config,
    ev: Sender<Event>,
    last: Arc<std::sync::Mutex<HashMap<String, MachineStatus>>>,
) {
    let _ = ev.send(Event::Refreshing);
    // Сканируем проекты локально (git статус) — быстро, блокирует ~50-200мс.
    let projects = scan_projects(&cfg);

    let conn = match crate::client::connect::connect_with_retry(&cfg).await {
        Ok(c) => c,
        Err(e) => {
            let text = format!("hub connect: {e}");
            let _ = ev.send(Event::Log { level: 3, text: text.clone() });
            let _ = ev.send(Event::SnapshotError(text));
            // Хаб лёг — показываем последний известный список машин
            // (не обнуляем дашборд) + свежие локальные проекты.
            let machines = last.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let _ = ev.send(Event::Snapshot { machines, projects, hub_ok: false });
            return;
        }
    };
    let req = crate::protocol::StatusRequest {
        machine: cfg.machine.name.clone(),
    };
    match crate::client::connect::send_status(&conn, &req).await {
        Ok(resp) => {
            *last.lock().unwrap_or_else(|p| p.into_inner()) = resp.machines.clone();
            let _ = ev.send(Event::Snapshot {
                machines: resp.machines,
                projects,
                hub_ok: true,
            });
        }
        Err(e) => {
            let text = format!("status: {e}");
            let _ = ev.send(Event::Log { level: 3, text: text.clone() });
            let _ = ev.send(Event::SnapshotError(text));
            let machines = last.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let _ = ev.send(Event::Snapshot { machines, projects, hub_ok: false });
        }
    }
}

/// Выполняет push/pull и шлёт результат в UI.
async fn run_action<F>(ev: &Sender<Event>, label: &str, fut: F)
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

// --- ДОКТОР ---

async fn run_doctor(cfg: Config, info: CfgSummary, ev: Sender<Event>) {
    let mut items: Vec<CheckItem> = Vec::new();

    // Путь к конфигу
    items.push(CheckItem {
        label: "config path".into(),
        level: if std::path::Path::new(&info.config_path).exists() { 0 } else { 1 },
        detail: info.config_path.clone(),
    });

    // chezmoi-managed?
    if info.chezmoi_managed {
        items.push(CheckItem {
            label: "chezmoi".into(),
            level: 2,
            detail: "live config managed — edits may be overwritten by chezmoi apply".into(),
        });
    }

    // machine name
    items.push(CheckItem {
        label: "machine name".into(),
        level: if cfg.machine.name.is_empty() { 1 } else { 0 },
        detail: cfg.machine.name.clone(),
    });

    // hub_connect
    match &cfg.hub_connect {
        Some(h) if !h.address.is_empty() => items.push(CheckItem {
            label: "hub_connect".into(),
            level: 0,
            detail: h.address.clone(),
        }),
        _ => items.push(CheckItem {
            label: "hub_connect".into(),
            level: 1,
            detail: "not configured".into(),
        }),
    }

    // ssh key
    let ssh_key = dirs::home_dir()
        .map(|h| h.join(".ssh/id_ed25519"))
        .unwrap_or_default();
    items.push(CheckItem {
        label: "ssh key".into(),
        level: if ssh_key.exists() { 0 } else { 1 },
        detail: ssh_key.display().to_string(),
    });

    // netbird route
    let has_route = std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|s| {
            s.connect("100.89.0.1:1").ok()?;
            s.local_addr().ok()
        })
        .is_some();
    items.push(CheckItem {
        label: "netbird route".into(),
        level: if has_route { 0 } else { 1 },
        detail: if has_route { "100.89.x.x reachable".into() } else { "no route to 100.89.x.x".into() },
    });

    // projects
    if let Some(projects) = &cfg.projects {
        for (name, p) in projects {
            let path = crate::projects::status::expand_user_path(&p.path);
            let detail = path.display().to_string();
            if !path.exists() {
                items.push(CheckItem { label: format!("project {name}"), level: 1, detail });
            } else if !path.join(".git").exists() {
                items.push(CheckItem { label: format!("project {name}"), level: 1, detail: format!("{detail} (not a git repo)") });
            } else {
                items.push(CheckItem { label: format!("project {name}"), level: 0, detail });
            }
        }
    }

    // hub connectivity (быстро — 4 попытки ~30с; делаем в фоне, шлём по готовности)
    items.push(CheckItem { label: "hub ping".into(), level: 0, detail: "testing…".into() });
    let _ = ev.send(Event::Doctor(items.clone()));

    // Тяжёлая проверка — отдельно
    match crate::client::connect::connect_with_retry(&cfg).await {
        Ok(conn) => {
            let req = crate::protocol::StatusRequest { machine: cfg.machine.name.clone() };
            match crate::client::connect::send_status(&conn, &req).await {
                Ok(resp) => {
                    let detail = resp.machines.iter()
                        .map(|(n, s)| format!("{n} online={}", s.online))
                        .collect::<Vec<_>>()
                        .join(", ");
                    items.last_mut().unwrap().detail = detail;
                    items.last_mut().unwrap().level = 0;
                }
                Err(e) => {
                    items.last_mut().unwrap().detail = format!("status request failed: {e}");
                    items.last_mut().unwrap().level = 1;
                }
            }
        }
        Err(e) => {
            items.last_mut().unwrap().detail = format!("connect failed: {e}");
            items.last_mut().unwrap().level = 1;
        }
    }
    let _ = ev.send(Event::Doctor(items));
}
