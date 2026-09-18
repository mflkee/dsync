//! Рендеринг интерфейса: шапка (табы + статус), подвал (подсказки),
//! и по функции на вкладку. По образцу esp32-tui/src/ui.rs.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph, Tabs};
use ratatui::Frame;

use crate::tui::app::{fmt_ago, fmt_civil, App, Tab};

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
        Tab::Log => draw_log(frame, app, body),
        Tab::Help => draw_help(frame, app, body),
    }
}

fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let [tabs_area, status_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).areas(area);

    // Строка вкладок.
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

    // Статус-строка под табами.
    let online = app.machines.list.iter().filter(|(_, s)| s.online).count();
    let mut status = format!(
        " {} — machines online: {}/{}  |  projects: {}",
        app.tab.title(),
        online,
        app.machines.list.len(),
        app.projects.len(),
    );
    if let Some((label, ok, text)) = &app.last_action {
        let mark = if *ok { "✓" } else { "✗" };
        status.push_str(&format!("  |  last: {} {} {}", label, mark, text));
    }

    // Индикатор фоновой задачи с анимированным спиннером.
    let busy_style = if let Some(label) = &app.busy {
        app.spin = app.spin.wrapping_add(1);
        const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let c = SPINNER[app.spin % SPINNER.len()];
        status.push_str(&format!("  ⏳ {} {}…", c, label));
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let block = Block::bordered().title(Span::styled(
        " dsync ",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
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

    let global = vec![
        Span::styled(" [q] quit  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Tab] next  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[p] push  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[l] pull  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[r] refresh", Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(Paragraph::new(Line::from(global)), global_area);

    let hint = format!(
        "   {}",
        match app.tab {
            Tab::Dashboard => "[↑↓] select machine  [p] push  [l] pull  [r] refresh",
            Tab::Projects => "[↑↓/wheel] scroll  [p] push  [l] pull  [r] refresh",
            Tab::Log => "[↑↓/wheel] scroll  [enter] clear",
            Tab::Help => "[↑↓/wheel] scroll",
        }
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            hint,
            Style::default().fg(Color::Yellow),
        )])),
        hint_area,
    );
}

// --- DASHBOARD ---

fn draw_dashboard(frame: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .areas(area);

    // Слева: машины, известные хабу.
    let rows: Vec<ListItem> = app
        .machines
        .list
        .iter()
        .map(|(name, s)| {
            let (mark, col) = if s.online {
                ("●", Color::Green)
            } else {
                ("○", Color::Red)
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::default().fg(col).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!(" {}", name),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("   push: {}", fmt_ago(s.last_push)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  seen: {}", fmt_ago(s.last_seen)),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let block = Block::bordered().title(Span::styled(
        format!(" Machines ({}) ", app.machines.list.len()),
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ));
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, left, &mut app.machines_state());

    // Справа: детали выбранной машины.
    draw_machine_detail(frame, app, right);
}

fn draw_machine_detail(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = vec![Line::from("")];
    match app.machines.selected() {
        None => {
            lines.push(Line::from(Span::styled(
                " No machines yet.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                " Нажмите [r], чтобы опросить хаб (нужен работающий",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                " dsync hub — archlinux-server:42069).",
                Style::default().fg(Color::DarkGray),
            )));
        }
        Some((name, s)) => {
            let (state, col) = if s.online {
                ("ONLINE", Color::Green)
            } else {
                ("OFFLINE", Color::Red)
            };
            lines.push(Line::from(vec![
                Span::styled(" Machine  : ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    name.as_str(),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" Status   : ", Style::default().fg(Color::DarkGray)),
                Span::styled(state, Style::default().fg(col).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" Last push: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} ({})", fmt_ago(s.last_push), fmt_civil(s.last_push)),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" Last seen: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} ({})", fmt_ago(s.last_seen), fmt_civil(s.last_seen)),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(" Проекты [{}] — см. вкладку Projects", app.projects.len()),
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::bordered().title(Span::styled(
        " Machine detail ",
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

// --- PROJECTS ---

fn draw_projects(frame: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<ListItem> = app
        .projects
        .iter()
        .map(|p| {
            let (mark, col) = if p.dirty {
                ("◈", Color::Yellow)
            } else {
                ("·", Color::Green)
            };
            let short = p.commit_hash.chars().take(8).collect::<String>();
            let status = if p.dirty { "dirty" } else { "clean" };
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::default().fg(col).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!(" {}", p.name),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  [{}]", p.branch), Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("  {}", status),
                    Style::default().fg(if p.dirty { Color::Yellow } else { Color::Green }),
                ),
                Span::styled(
                    format!("  ↑{} ↓{}", p.ahead, p.behind),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  {}  {}", short, fmt_ago(p.last_commit_time)),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let block = Block::bordered().title(Span::styled(
        format!(" Projects ({}) ", app.projects.len()),
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(List::new(rows).block(block), area);
}

// --- LOG ---

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let page_h = area.height.saturating_sub(2) as usize;
    let total = app.logs.len();
    let start = app.log_scroll.min(total.saturating_sub(page_h));
    let rows: Vec<ListItem> = app
        .logs
        .iter()
        .skip(start)
        .take(page_h)
        .map(|l| {
            let color = match l.level {
                0 => Color::Gray,
                1 => Color::Green,
                2 => Color::Yellow,
                _ => Color::Red,
            };
            ListItem::new(Line::from(Span::styled(&l.text, Style::default().fg(color))))
        })
        .collect();
    let block = Block::bordered().title(format!(
        " Log ({}){} ",
        total,
        if start > 0 {
            format!(" [scrolled {}..{}]", start, start + rows.len())
        } else {
            String::new()
        }
    ));
    frame.render_widget(List::new(rows).block(block), area);
}

// --- HELP ---

fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
    let text = help_lines();
    let page_h = area.height.saturating_sub(2) as usize;
    let start = app.help_scroll.min(text.len().saturating_sub(page_h));
    let slice = &text[start..(start + page_h).min(text.len())];
    let block = if start > 0 {
        Block::bordered().title(" Help [scrolled] ↑↓/wheel ")
    } else {
        Block::bordered()
    };
    frame.render_widget(Paragraph::new(slice.to_vec()).block(block), area);
}

fn help_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(Span::styled(
            " GLOBAL",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from("   [Tab]/[Shift-Tab]  — switch tab"),
        Line::from("   [q]/[Esc]          — quit"),
        Line::from("   [p]                — push local state (dotfiles + projects) to hub"),
        Line::from("   [l]                — pull state from hub"),
        Line::from("   [r]                — refresh status snapshot now"),
        Line::from(""),
        Line::from(Span::styled(
            " DASHBOARD",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from("   [↑/↓]  or [k/j]    — select machine (● online / ○ offline)"),
        Line::from("   Details of the selected machine are on the right panel."),
        Line::from(""),
        Line::from(Span::styled(
            " PROJECTS",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from("   Shows local git state per configured project:"),
        Line::from("   ◈ dirty / · clean, branch, ahead/behind origin, short commit"),
        Line::from(""),
        Line::from(Span::styled(
            " LOG",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from("   [Enter]            — clear log"),
        Line::from(""),
        Line::from(Span::styled(
            " Architecture",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        )),
        Line::from("   UI-поток (ratatui) синхронный; все сетевые операции идут в"),
        Line::from("   фоновом потоке с собственным tokio runtime (src/tui/backend.rs)."),
        Line::from("   События — через crossbeam-канал, как worker.rs в esp32-tui."),
    ]
}

// Состояние списка для render_stateful_widget.
impl App {
    pub(crate) fn machines_state(&mut self) -> ratatui::widgets::ListState {
        let mut s = ratatui::widgets::ListState::default();
        if !self.machines.list.is_empty() {
            s.select(Some(self.machines.selected));
        }
        s
    }
}