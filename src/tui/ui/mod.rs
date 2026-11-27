//! Рендеринг интерфейса: шапка (табы + статус), подвал (подсказки),
//! и по функции на вкладку + модальная форма. По образцу esp32-tui/src/ui.rs.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::tui::app::{fmt_ago, fmt_civil, fmt_clock, App, ConfirmAction, Tab};

/// Высота шапки: строка вкладок + 2 строки статуса.
pub const HEADER_ROWS: u16 = 3;
/// Высота подвала: 2 строки подсказок + рамка.
pub const FOOTER_ROWS: u16 = 4;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(HEADER_ROWS),
        Constraint::Min(0),
        Constraint::Length(FOOTER_ROWS),
    ])
    .areas(area);

    draw_header(frame, app, header);
    draw_footer(frame, app, footer);

    match app.tab {
        Tab::Dashboard => draw_dashboard(frame, app, body),
        Tab::Projects => draw_projects(frame, app, body),
        Tab::Machines => draw_machines(frame, app, body),
        Tab::State => draw_state(frame, app, body),
        Tab::Doctor => draw_doctor(frame, app, body),
        Tab::Log => draw_log(frame, app, body),
        Tab::Help => draw_help(frame, app, body),
    }

    if app.form.is_some() {
        draw_form(frame, app, body);
    }
    if app.show_pull_details {
        draw_pull_details(frame, app, body);
    }
}

mod dashboard;
mod doctor;
mod forms;
mod help;
mod log;
mod machines;
mod projects;
mod state;

use dashboard::draw_dashboard;
use doctor::draw_doctor;
use forms::draw_form;
use help::draw_help;
use log::draw_log;
use machines::{draw_machine_detail, draw_machines, draw_pull_details};
use projects::draw_projects;
use state::draw_state;

fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let [tabs_area, status_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).areas(area);

    let titles: Vec<Line> = Tab::ALL.iter().map(|t| Line::from(t.title())).collect();
    let tabs = Tabs::new(titles)
        .select(Tab::ALL.iter().position(|t| t == &app.tab).unwrap_or(0))
        .divider("  ")
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, tabs_area);

    let online = app.machines.list.iter().filter(|(_, s)| s.online).count();
    let mut status = format!(
        " {} — machines: {}/{}  |  projects: {}  |  remotes: {}",
        app.tab.title(),
        online,
        app.machines.list.len(),
        app.projects.len(),
        app.cfg.remotes.len(),
    );
    if let Some((label, ok, text)) = &app.last_action {
        let mark = if *ok { "✓" } else { "✗" };
        status.push_str(&format!("  |  last: {label} {mark} {text}"));
    }
    // Индикатор опроса хаба + время последнего снимка.
    if app.refreshing {
        status.push_str("  |  ⟳ refresh…");
    } else if app.last_snapshot_ts > 0 {
        status.push_str(&format!("  |  ⟳ {}", fmt_clock(app.last_snapshot_ts)));
    }
    if app.form.is_some() {
        status.push_str("  |  [form]");
    }

    let busy_style = if let Some(label) = &app.busy {
        app.spin = app.spin.wrapping_add(1);
        const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let c = SPINNER[app.spin % SPINNER.len()];
        status.push_str(&format!("  ⏳ {c} {label}…"));
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let block = Block::bordered().title(Span::styled(
        " dsync ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(status, busy_style))).block(block),
        status_area,
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered();
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [global_area, hint_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    // Глобальные клавиши — один раз, серым. Таб-специфичные — отдельной
    // строкой ниже (оранжевым), без дублирования p/l/r.
    let global = vec![
        Span::styled(" [q] quit  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Tab] tabs  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[p] push all  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[l] pull all  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[r] refresh", Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(Paragraph::new(Line::from(global)), global_area);

    let hint = if app.form.is_some() {
        let confirm = app
            .form
            .as_ref()
            .map(|f| f.action.is_some())
            .unwrap_or(false);
        if confirm {
            "   [Enter] confirm  [Esc] cancel".to_string()
        } else {
            // Подсказка редактирования + позиция курсора активного поля.
            let col = app
                .form
                .as_ref()
                .and_then(|f| f.fields.get(f.cursor))
                .map(|f| f.cursor)
                .unwrap_or(0);
            format!(
                "   [Tab/↑↓] field  [←/→/Home/End] cursor  [⌫/Del] edit  [Ctrl-U] clear  \
                 [Enter] save  [Esc] cancel  · insert at column {col}"
            )
        }
    } else {
        format!(
            "   {}",
            match app.tab {
                Tab::Dashboard =>
                    "[↑↓] select  [P] push selected  [L] pull selected  [e] failed pulls",
                Tab::Projects => "[↑↓] select  [n] add project  [d] delete",
                Tab::Machines => "[↑↓] select  [n] add machine  [d] delete  [e] failed pulls",
                Tab::State => "[t] tmux  [T] auto-restore  [n] opencode projects  [r] refresh",
                Tab::Doctor => "[r] run checks  [↑↓/PgUp/PgDn] scroll",
                Tab::Log => "[↑↓/PgUp/PgDn] scroll  [Enter] clear",
                Tab::Help => "[↑↓/PgUp/PgDn] scroll",
            }
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            hint,
            Style::default().fg(Color::Yellow),
        )])),
        hint_area,
    );
}

// --- DASHBOARD ---

pub(super) fn centered_rect(w_pct: u16, h_pct: u16, area: Rect) -> Rect {
    let side_w = (100 - w_pct) / 2;
    let side_h = (100 - h_pct) / 2;
    let [_, wb, _] = Layout::horizontal([
        Constraint::Percentage(side_w),
        Constraint::Percentage(w_pct),
        Constraint::Percentage(side_w),
    ])
    .areas(area);
    let [_, hb, _] = Layout::vertical([
        Constraint::Percentage(side_h),
        Constraint::Percentage(h_pct),
        Constraint::Percentage(side_h),
    ])
    .areas(wb);
    hb
}

// Состояния списков для render_stateful_widget.
impl App {
    pub(crate) fn machines_state(&mut self) -> ratatui::widgets::ListState {
        let mut s = ratatui::widgets::ListState::default();
        if !self.machines.list.is_empty() {
            s.select(Some(self.machines.selected));
        }
        s
    }

    pub(crate) fn projects_state(&mut self) -> ratatui::widgets::ListState {
        let mut s = ratatui::widgets::ListState::default();
        s.select(Some(self.projects_sel.idx));
        s
    }

    pub(crate) fn remotes_state(&mut self, len: usize) -> ratatui::widgets::ListState {
        let mut s = ratatui::widgets::ListState::default();
        if len > 0 {
            s.select(Some(self.remotes_sel.idx));
        }
        s
    }
}
