//! Рендеринг интерфейса: шапка (табы + статус), подвал (подсказки),
//! и по функции на вкладку + модальная форма. По образцу esp32-tui/src/ui.rs.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::tui::app::{fmt_ago, fmt_civil, fmt_clock, App, Tab};

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
        Tab::Doctor => draw_doctor(frame, app, body),
        Tab::Log => draw_log(frame, app, body),
        Tab::Help => draw_help(frame, app, body),
    }

    if app.form.is_some() {
        draw_form(frame, app, body);
    }
}

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
            "   [Enter] delete  [Esc] cancel".to_string()
        } else {
            "   [Tab/↑↓] field  [Enter] save  [Esc] cancel".to_string()
        }
    } else {
        format!(
            "   {}",
            match app.tab {
                Tab::Dashboard => "[↑↓] select  [P] push selected  [L] pull selected",
                Tab::Projects => "[↑↓] select  [n] add project  [d] delete",
                Tab::Machines => "[↑↓] select  [n] add machine  [d] delete",
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

fn draw_dashboard(frame: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);

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
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("   push: {}", fmt_ago(s.last_push)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  seen: {}", fmt_ago(s.last_seen)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    {
                        let ok = s.pulls.values().filter(|o| o.ok).count();
                        let total = s.pulls.len();
                        if total == 0 {
                            String::new()
                        } else {
                            format!("  pulls: {ok}✓/{}✗", total - ok)
                        }
                    },
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let block = Block::bordered().title(Span::styled(
        format!(" Machines ({}) ", app.machines.list.len()),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let list = List::new(rows)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, left, &mut app.machines_state());

    draw_machine_detail(frame, app, right);
}

fn draw_machine_detail(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = vec![Line::from("")];
    match app.machines.selected() {
        None => {
            if app.refreshing {
                lines.push(Line::from(Span::styled(
                    " ⟳ Опроса хаба… (ожидание ответа)",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(""));
            }
            if let Some(err) = &app.last_error {
                lines.push(Line::from(Span::styled(
                    " ⚠ Hub недоступен — снимок машин пуст.",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(
                    format!("   {err}"),
                    Style::default().fg(Color::Red),
                )));
                lines.push(Line::from(""));
            } else {
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
            // Локальная сводка — чтобы «что-то было видно» даже с мёртвым хабом.
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Config ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(vec![
                Span::styled(" machine  : ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    app.cfg.machine.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" hub      : ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    app.cfg.hub_connect.clone().unwrap_or_else(|| "-".into()),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" projects : ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} configured", app.cfg.projects.len()),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" remotes  : ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} machines", app.cfg.remotes.len()),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        Some((name, s)) => {
            if let Some(err) = &app.last_error {
                lines.push(Line::from(Span::styled(
                    " ⚠ Hub недоступен — показан прошлый снимок.",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(
                    format!("   {err}"),
                    Style::default().fg(Color::Red),
                )));
                lines.push(Line::from(""));
            }
            let (state, col) = if s.online {
                ("ONLINE", Color::Green)
            } else {
                ("OFFLINE", Color::Red)
            };
            lines.push(Line::from(vec![
                Span::styled(" Machine  : ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    name.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
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
            if !s.pulls.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " SSH pulls:",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )));
                let mut pulls: Vec<_> = s.pulls.iter().collect();
                pulls.sort_by_key(|(p, _)| (*p).clone());
                for (project, o) in pulls {
                    let (mark, col) = if o.ok {
                        ("✓", Color::Green)
                    } else {
                        ("✗", Color::Red)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("   {mark} {project}"),
                            Style::default().fg(col).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {} attempt(s)  {}", o.attempts, fmt_ago(o.finished_at)),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(" [P] push — {name}   [L] pull — {name}"),
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(Span::styled(
                " online = hub received a push < 35 min ago",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let block = Block::bordered().title(Span::styled(
        " Machine detail ",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

// --- PROJECTS ---

fn draw_projects(frame: &mut Frame, app: &mut App, area: Rect) {
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
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  [{}]", p.branch),
                    Style::default().fg(Color::Cyan),
                ),
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

    let empty_hint = if app.cfg.projects.is_empty() {
        " Проектов нет — нажмите [n], чтобы добавить."
    } else if app.projects.is_empty() {
        " Сканирование git состояния… (ждите авто-poll или [r])"
    } else {
        ""
    };

    let block = Block::bordered().title(Span::styled(
        format!(" Projects ({}) ", app.projects.len()),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let list = if app.projects.is_empty() {
        List::new(vec![ListItem::new(Line::from(Span::styled(
            empty_hint,
            Style::default().fg(Color::DarkGray),
        )))])
        .block(block)
    } else {
        List::new(rows)
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ")
    };
    frame.render_stateful_widget(list, area, &mut app.projects_state());
}

// --- MACHINES (конфиг) ---

fn draw_machines(frame: &mut Frame, app: &mut App, area: Rect) {
    let [summary_area, list_area] =
        Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).areas(area);

    let mut sum = vec![
        Line::from(vec![
            Span::styled(" machine  : ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.cfg.machine.as_str(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   hub: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.cfg.hub_connect.clone().unwrap_or_else(|| "-".into()),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled(" config   : ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.cfg.config_path.as_str(),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled(" chezmoi  : ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if app.cfg.chezmoi_managed {
                    "managed — edits hit the live file, template is separate"
                } else {
                    "not managed"
                },
                Style::default().fg(if app.cfg.chezmoi_managed {
                    Color::Yellow
                } else {
                    Color::Green
                }),
            ),
        ]),
    ];
    if app.cfg.has_hub_section {
        sum.push(Line::from(vec![
            Span::styled(" [hub]    : ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "this machine runs the hub",
                Style::default().fg(Color::Magenta),
            ),
        ]));
    }

    let block = Block::bordered().title(Span::styled(
        " Config ",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(sum).block(block), summary_area);

    let rows: Vec<ListItem> = app
        .cfg
        .remotes
        .iter()
        .map(|r| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {}", r.name),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}@{}:{}", r.user, r.host, r.port),
                    Style::default().fg(Color::Cyan),
                ),
            ]))
        })
        .collect();
    let block = Block::bordered().title(Span::styled(
        format!(
            " Remote machines ({}) — [n] add  [d] delete ",
            app.cfg.remotes.len()
        ),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let list = if rows.is_empty() {
        List::new(vec![ListItem::new(Line::from(Span::styled(
            " Нет машин в [remote]. Нажмите [n], чтобы добавить (host/port/user).",
            Style::default().fg(Color::DarkGray),
        )))])
        .block(block)
    } else {
        List::new(rows)
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ")
    };
    frame.render_stateful_widget(
        list,
        list_area,
        &mut app.remotes_state(app.cfg.remotes.len()),
    );
}

// --- DOCTOR ---

fn draw_doctor(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.doctor.is_empty() {
        let block = Block::bordered().title(Span::styled(
            " Doctor ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                " Нажмите [r], чтобы запустить проверки:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  × config path, machine name, hub_connect, ssh key,",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  × netbird route, git-репозитории проектов, ping хаба.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let page_h = area.height.saturating_sub(2) as usize;
    let total = app.doctor.len();
    let start = app.doctor_scroll.min(total.saturating_sub(page_h));
    let rows: Vec<ListItem> = app
        .doctor
        .iter()
        .skip(start)
        .take(page_h)
        .map(|c| {
            let (sym, col) = match c.level {
                0 => ("✓", Color::Green),
                1 => ("✗", Color::Yellow),
                _ => ("∼", Color::DarkGray),
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {sym} {} ", c.label),
                    Style::default().fg(col).add_modifier(Modifier::BOLD),
                ),
                Span::styled(c.detail.clone(), Style::default().fg(Color::White)),
            ]))
        })
        .collect();
    let block = Block::bordered().title(Span::styled(
        format!(
            " Doctor [r] rerun ({}){} ",
            total,
            if start > 0 {
                format!(" [scrolled {}..{}]", start, start + rows.len())
            } else {
                String::new()
            }
        ),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(List::new(rows).block(block), area);
}

// --- LOG ---

fn draw_log(frame: &mut Frame, app: &mut App, area: Rect) {
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
            ListItem::new(Line::from(Span::styled(
                &l.text,
                Style::default().fg(color),
            )))
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
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [Tab]/[Shift-Tab]  — switch tab"),
        Line::from("   [q]/[Esc]          — quit"),
        Line::from("   [p]                — push local state to hub (all machines)"),
        Line::from("   [P] (Dashboard)    — push state restricted to selected machine"),
        Line::from("   [l]                — pull state from hub (all machines)"),
        Line::from("   [L] (Dashboard)    — pull state restricted to selected machine"),
        Line::from("   [r]                — refresh snapshot now (Doctor: rerun checks)"),
        Line::from(""),
        Line::from(Span::styled(
            " DASHBOARD",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [↑/↓]  or [k/j]    — select machine (● online / ○ offline)"),
        Line::from("   Right panel: details; with hub down — local config summary."),
        Line::from(""),
        Line::from(Span::styled(
            " PROJECTS",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [↑/↓]              — select project"),
        Line::from(
            "   [n]                — add project to config (name/path/branch/machines/post_pull)",
        ),
        Line::from("   [d]                — delete selected project from config"),
        Line::from("   Rows: ◈ dirty / · clean, branch, ahead/behind origin, commit"),
        Line::from(""),
        Line::from(Span::styled(
            " MACHINES (config)",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [↑/↓]              — select remote machine"),
        Line::from("   [n]                — add machine ([remote.<name>]: host/port/user)"),
        Line::from("   [d]                — delete selected machine from config"),
        Line::from(""),
        Line::from(Span::styled(
            " DOCTOR",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [r]                — run checks: config, ssh key, netbird, git, hub ping"),
        Line::from(""),
        Line::from(Span::styled(
            " LOG",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [Enter]            — clear log"),
        Line::from(""),
        Line::from(Span::styled(
            " Architecture",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   UI-поток (ratatui) синхронный; сеть/файлы — в фоне"),
        Line::from("   (src/tui/backend.rs) с собственным tokio runtime."),
        Line::from("   Конфиг редактируется на живой файл; chezmoi применяй"),
        Line::from("   отдельно (шаблон в dotfiles)."),
    ]
}

// --- FORM OVERLAY ---

fn draw_form(frame: &mut Frame, app: &App, area: Rect) {
    let Some(form) = &app.form else { return };
    let is_confirm = form.action.is_some();

    let (w, h) = if is_confirm { (60, 30) } else { (70, 55) };
    let popup = centered_rect(w, h, area);
    frame.render_widget(Clear, popup);

    let mut lines: Vec<Line> = Vec::new();
    if is_confirm {
        let detail = &form.fields[0].value;
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "   Это удалит запись из конфига:",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("   {detail}"),
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "   [Enter] — удалить   [Esc] — отмена",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(""));
        for (i, f) in form.fields.iter().enumerate() {
            let active = i == form.cursor;
            let arrow = if active { "▸ " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(arrow, Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("{}: ", f.label),
                    if active {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(
                    f.value.clone(),
                    Style::default().fg(Color::White).bg(if active {
                        Color::DarkGray
                    } else {
                        Color::Reset
                    }),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "   [Tab/↑↓] поле   [Enter] следующее / сохранить   [Esc] отмена",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let block = Block::bordered().title(Span::styled(
        form.title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let par = if is_confirm {
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true })
    } else {
        Paragraph::new(lines).block(block)
    };
    frame.render_widget(par, popup);
}

fn centered_rect(w_pct: u16, h_pct: u16, area: Rect) -> Rect {
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
