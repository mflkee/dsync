//! Состояние приложения и обработка событий (по образцу esp32-tui/src/app.rs).

use std::collections::VecDeque;

use crossbeam_channel::Receiver;

use crate::protocol::{MachineStatus, ProjectState};

use super::backend::{CheckItem, Cmd, CmdSender, Event};
use super::cfg::CfgSummary;

/// Вкладки интерфейса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Projects,
    Machines,
    State,
    Doctor,
    Log,
    Help,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Dashboard,
        Tab::Projects,
        Tab::Machines,
        Tab::State,
        Tab::Doctor,
        Tab::Log,
        Tab::Help,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Projects => "Projects",
            Tab::Machines => "Machines",
            Tab::State => "State",
            Tab::Doctor => "Doctor",
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
    pub level: u8,
}

/// Список машин (name, status) с курсором — для дашборда.
#[derive(Debug, Default)]
pub struct MachineList {
    pub list: Vec<(String, MachineStatus)>,
    pub selected: usize,
}

impl MachineList {
    pub fn update(&mut self, machines: std::collections::HashMap<String, MachineStatus>) {
        let mut list: Vec<(String, MachineStatus)> = machines.into_iter().collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        self.list = list;
        self.clamp();
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

    fn clamp(&mut self) {
        if self.selected >= self.list.len() && !self.list.is_empty() {
            self.selected = self.list.len() - 1;
        }
    }
}

/// Универсальный «выбранный индекс» — используется Projects, Machines (remote), Doctor.
#[derive(Debug, Default, Clone, Copy)]
pub struct SelIndex {
    pub idx: usize,
    pub len: usize,
}

impl SelIndex {
    pub fn new(len: usize) -> Self {
        Self { idx: 0, len }
    }
    pub fn sel_next(&mut self) {
        if self.len > 0 {
            self.idx = (self.idx + 1).min(self.len - 1);
        }
    }
    pub fn sel_prev(&mut self) {
        if self.len > 0 {
            self.idx = self.idx.saturating_sub(1);
        }
    }
}

/// Полное состояние TUI.
pub struct App {
    pub tab: Tab,
    pub should_quit: bool,
    pub machines: MachineList,
    pub projects: Vec<ProjectState>,
    pub logs: VecDeque<LogLine>,
    /// Активная фоновая задача (спиннер в шапке).
    pub busy: Option<String>,
    /// Последняя завершённая задача.
    pub last_action: Option<(String, bool, String)>,
    /// Последняя ошибка связи с хабом.
    pub last_error: Option<String>,
    /// Идёт ли опрос хаба (индикатор «⟳ refresh…»).
    pub refreshing: bool,
    /// Время последнего снимка (unix ts, 0 = ещё не было).
    pub last_snapshot_ts: i64,
    /// Счётчик тиков спиннера.
    pub spin: usize,

    // -- Selection indices per tab --
    pub projects_sel: SelIndex,
    pub remotes_sel: SelIndex,
    pub doctor_scroll: usize,
    pub log_scroll: usize,
    pub help_scroll: usize,

    // -- Config summary (из backend) --
    pub cfg: CfgSummary,
    // -- Doctor results --
    pub doctor: Vec<CheckItem>,

    // -- State tab --
    pub state_channels: Vec<crate::protocol::ChannelStatus>,
    /// Ошибка запроса state_status («unavailable» на старом хабе и т.п.).
    pub state_status_error: Option<String>,
    /// Время последней успешной сводки состояния.
    pub state_status_ts: i64,

    // -- Pull details (Dashboard/Machines) --
    pub show_pull_details: bool,
    pub pull_details_scroll: usize,

    // -- Forms --
    pub form: Option<Form>,

    pub events: Receiver<Event>,
    cmd_tx: CmdSender,
}

/// Модальная форма добавления или подтверждения.
#[derive(Debug, Clone)]
pub struct Form {
    pub title: &'static str,
    pub fields: Vec<FormField>,
    pub cursor: usize,
    /// Some — это форма подтверждения (Enter = выполнить действие).
    pub action: Option<ConfirmAction>,
}

#[derive(Debug, Clone)]
pub struct FormField {
    pub label: &'static str,
    pub value: String,
    /// Позиция курсора внутри поля (в символах), для [←]/[→]/Home/End/⌫/Del.
    pub cursor: usize,
}

impl FormField {
    pub fn new(label: &'static str, value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self {
            label,
            value,
            cursor,
        }
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Число символов в значении.
    pub fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    /// Байтовый индекс позиции курсора (для вставки/удаления в String).
    pub fn byte_idx(&self) -> usize {
        char_pos_to_byte(&self.value, self.cursor)
    }

    /// Вставить символ на позицию курсора и сдвинуть курсор.
    pub fn insert_char(&mut self, c: char) {
        let bi = self.byte_idx();
        self.value.insert(bi, c);
        self.cursor += 1;
    }

    /// Удалить символ перед курсором (Backspace).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before: String = self.value.chars().take(self.cursor.saturating_sub(1)).collect();
        let after: String = self.value.chars().skip(self.cursor).collect();
        self.value = format!("{before}{after}");
        self.cursor -= 1;
    }

    /// Удалить символ под курсором (Delete).
    pub fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let before: String = self.value.chars().take(self.cursor).collect();
        let after: String = self.value.chars().skip(self.cursor + 1).collect();
        self.value = format!("{before}{after}");
    }
}

/// Байтовый индекс начала символа на позиции `pos` (0-based, в символах);
/// pos ≥ числа символов → конец строки.
pub fn char_pos_to_byte(s: &str, pos: usize) -> usize {
    for (count, (byte, _)) in s.char_indices().enumerate() {
        if count == pos {
            return byte;
        }
    }
    s.len()
}

/// Действие подтверждения удаления.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    RemoveProject(String),
    RemoveRemote(String),
    /// Подтверждено: сохранить конфиг через chezmoi-шаблон (см. pending_cmd).
    ChezmoiApply(Cmd),
    /// Подтверждено переключение [state]: отправить Cmd::SetState.
    StateToggle {
        tmux: Option<bool>,
        tmux_restore: Option<bool>,
    },
}

impl App {
    pub fn new(events: Receiver<Event>, cmd_tx: CmdSender, cfg: CfgSummary) -> Self {
        let projects_sel = SelIndex::new(cfg.projects.len());
        let remotes_sel = SelIndex::new(cfg.remotes.len());
        Self {
            tab: Tab::Dashboard,
            should_quit: false,
            machines: MachineList::default(),
            projects: Vec::new(),
            logs: VecDeque::new(),
            busy: None,
            last_action: None,
            last_error: None,
            refreshing: false,
            last_snapshot_ts: 0,
            spin: 0,
            projects_sel,
            remotes_sel,
            doctor_scroll: 0,
            log_scroll: 0,
            help_scroll: 0,
            cfg,
            doctor: Vec::new(),
            state_channels: Vec::new(),
            state_status_error: None,
            state_status_ts: 0,
            show_pull_details: false,
            pull_details_scroll: 0,
            form: None,
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

    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmd_tx.try_send(cmd);
    }

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
            Event::Snapshot {
                machines,
                projects,
                hub_ok,
            } => {
                self.machines.update(machines);
                self.projects = projects;
                self.projects.sort_by(|a, b| a.name.cmp(&b.name));
                self.projects_sel.len = self.projects.len();
                self.projects_sel.idx = self
                    .projects_sel
                    .idx
                    .min(self.projects_sel.len.saturating_sub(1));
                self.refreshing = false;
                self.last_snapshot_ts = unix_now();
                if hub_ok {
                    self.last_error = None;
                }
            }
            Event::Log { level, text } => self.log(level, text),
            Event::SnapshotError(err) => {
                self.refreshing = false;
                self.last_error = Some(err);
            }
            Event::Refreshing => self.refreshing = true,
            Event::ActionDone { label, ok, text } => {
                self.busy = None;
                self.last_action = Some((label, ok, text));
                self.send(Cmd::Poll);
                self.re_query_state_if_active();
            }
            Event::Doctor(items) => {
                self.doctor = items;
                self.doctor_scroll = 0;
            }
            Event::ConfigChanged { summary } => {
                self.cfg = summary;
                self.projects_sel.len = self.cfg.projects.len();
                self.remotes_sel.len = self.cfg.remotes.len();
            }
            Event::StateStatus { channels, err } => {
                let ok = err.is_none();
                self.state_channels = channels;
                self.state_status_error = err;
                self.state_status_ts = if ok { unix_now() } else { 0 };
            }
        }
    }

    /// Переключить активную вкладку; при входе на State — запросить сводку.
    pub fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::State {
            self.send(Cmd::StateStatus);
        }
    }

    /// После завершения push/pull обновляем и state-сводку (если открыт State).
    pub fn re_query_state_if_active(&mut self) {
        if self.tab == Tab::State {
            self.send(Cmd::StateStatus);
        }
    }
}

/// Относительное время «N d/h/m/s ago».
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

/// Короткое локальное время «HH:MM:SS» — для статус-строки.
pub fn fmt_clock(ts: i64) -> String {
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string(),
        None => "-".to_string(),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_insert_at_cursor_middle() {
        let mut f = FormField::new("name", "mashroomwars");
        // курсор ставим на 2-й символ «s» (после "ma")
        f.cursor = 2;
        f.insert_char('s');
        assert_eq!(f.value, "masshroomwars");
        assert_eq!(f.cursor, 3);
    }

    #[test]
    fn field_backspace_delete_and_clear() {
        let mut f = FormField::new("path", "/home/mflkee");
        f.cursor = 5; // перед 'e'
        f.backspace();
        assert_eq!(f.value, "/hom/mflkee");
        assert_eq!(f.cursor, 4);

        let mut g = FormField::new("path", "abc");
        g.cursor = 1;
        g.delete();
        assert_eq!(g.value, "ac");

        let mut h = FormField::new("x", "hello world");
        h.clear();
        assert!(h.value.is_empty());
        assert_eq!(h.cursor, 0);
    }

    #[test]
    fn field_unicode_never_panics_and_counts_chars() {
        let mut f = FormField::new("machine", "архив-машина-01");
        assert_eq!(f.char_len(), "архив-машина-01".chars().count());
        f.cursor = 3;
        f.insert_char('X');
        assert_eq!(f.value, "архXив-машина-01");
        assert_eq!(f.cursor, 4);

        // Вырезка/вставка на границах символов не режет UTF-8.
        let mut g = FormField::new("x", "привет");
        g.cursor = g.char_len();
        g.backspace();
        assert_eq!(g.value, "приве");

        let mut h = FormField::new("x", "привет");
        h.cursor = 1;
        h.delete();
        assert_eq!(h.value, "пивет");
    }

    #[test]
    fn char_pos_clamps_out_of_range() {
        assert_eq!(char_pos_to_byte("abc", 0), 0);
        assert_eq!(char_pos_to_byte("abc", 3), "abc".len());
        assert_eq!(char_pos_to_byte("abc", 99), "abc".len());
        assert_eq!(char_pos_to_byte("", 2), 0);
        // граница по символам, а не по байтам
        assert_eq!(char_pos_to_byte("абв", 2), "абв".char_indices().nth(2).unwrap().0);
    }
}
