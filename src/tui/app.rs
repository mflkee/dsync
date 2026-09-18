//! Состояние приложения и обработка событий (по образцу esp32-tui/src/app.rs).

use std::collections::{HashMap, VecDeque};

use crossbeam_channel::Receiver;

use crate::protocol::{MachineStatus, ProjectState};

use super::backend::{Cmd, CmdSender, Event};

/// Вкладки интерфейса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Projects,
    Log,
    Help,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Dashboard, Tab::Projects, Tab::Log, Tab::Help];

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Projects => "Projects",
            Tab::Log => "Log",
            Tab::Help => "Help",
        }
    }

    pub fn next(&self) -> Tab {
        let idx = Tab::ALL.iter().position(|t| t == self).unwrap();
        Tab::ALL[(idx + 1) % Tab::ALL.len()]
    }

    pub fn prev(&self) -> Tab {
        let idx = Tab::ALL.iter().position(|t| t == self).unwrap();
        Tab::ALL[(idx + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// Строка лога.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub text: String,
    pub level: u8, // 0=info 1=ok 2=warn 3=err
}

/// Список машин (name, status) с курсором.
#[derive(Debug, Default)]
pub struct Machines {
    pub list: Vec<(String, MachineStatus)>,
    pub selected: usize,
}

impl Machines {
    pub fn update(&mut self, machines: HashMap<String, MachineStatus>) {
        let mut list: Vec<(String, MachineStatus)> = machines.into_iter().collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        self.list = list;
        if self.selected >= self.list.len() && !self.list.is_empty() {
            self.selected = self.list.len() - 1;
        }
    }

    pub fn sel_next(&mut self) {
        if !self.list.is_empty() {
            self.selected = (self.selected + 1).min(self.list.len() - 1);
        }
    }

    pub fn sel_prev(&mut self) {
        if !self.list.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub fn selected(&self) -> Option<&(String, MachineStatus)> {
        self.list.get(self.selected)
    }
}

/// Полное состояние TUI.
pub struct App {
    pub tab: Tab,
    pub should_quit: bool,
    pub machines: Machines,
    pub projects: Vec<ProjectState>,
    pub logs: VecDeque<LogLine>,
    /// Активная фоновая задача (показывается спиннером в шапке).
    pub busy: Option<String>,
    /// Последняя завершённая задача (label, ok, текст) — для статус-строки.
    pub last_action: Option<(String, bool, String)>,
    /// Последняя ошибка получения снимка (показывается на дашборде).
    pub last_error: Option<String>,
    /// Счётчик тиков (для анимации спиннера).
    pub spin: usize,
    /// Прокрутка лога (вкладка Log).
    pub log_scroll: usize,
    /// Прокрутка справки (вкладка Help).
    pub help_scroll: usize,
    pub events: Receiver<Event>,
    cmd_tx: CmdSender,
}

impl App {
    pub fn new(events: Receiver<Event>, cmd_tx: CmdSender) -> Self {
        Self {
            tab: Tab::Dashboard,
            should_quit: false,
            machines: Machines::default(),
            projects: Vec::new(),
            logs: VecDeque::new(),
            busy: None,
            last_action: None,
            last_error: None,
            spin: 0,
            log_scroll: 0,
            help_scroll: 0,
            events,
            cmd_tx,
        }
    }

    pub fn log(&mut self, level: u8, text: String) {
        self.logs.push_back(LogLine { text, level });
        if self.logs.len() > 300 {
            self.logs.pop_front();
        }
    }

    /// Отправить команду в backend (push/pull/poll). Канал bounded (16),
    /// трафик редкий — blocking_send не блокирует практически никогда.
    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmd_tx.blocking_send(cmd);
    }

    /// Запустить push/pull: пока busy — повторные запуски игнорируем.
    pub fn run_action(&mut self, label: &str, cmd: Cmd) {
        if self.busy.is_some() {
            self.log(2, format!("{label}: another task is running"));
            return;
        }
        self.busy = Some(label.to_string());
        self.log(0, format!("⏳ {label}…"));
        self.send(cmd);
    }

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Snapshot { machines, projects } => {
                self.machines.update(machines);
                self.projects = projects;
                self.last_error = None;
            }
            Event::Log { level, text } => self.log(level, text),
            Event::SnapshotError(err) => self.last_error = Some(err),
            Event::ActionDone { label, ok, text } => {
                self.busy = None;
                self.last_action = Some((label, ok, text));
                // После push/pull состояние могло измениться — берём свежий снимок.
                self.send(Cmd::Poll);
            }
        }
    }
}

/// Относительное время «N d/h/m/s ago» — для статус-строк.
pub fn fmt_ago(ts: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = now - ts;
    if diff < 0 {
        return "soon".into();
    }
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

/// Абсолютное локальное время «YYYY-MM-DD HH:MM».
pub fn fmt_civil(ts: i64) -> String {
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => format!("{ts}"),
    }
}