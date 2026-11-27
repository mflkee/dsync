use super::*;

pub(super) fn draw_machine_detail(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_machines(frame: &mut Frame, app: &mut App, area: Rect) {
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
                    match &app.cfg.chezmoi_template {
                        Some(t) => format!("managed — saves via template {t} + chezmoi apply"),
                        None => "managed — no template; config edits refused".to_string(),
                    }
                } else {
                    "not managed".to_string()
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

// --- STATE (не-git состояние флота) ---

pub(super) fn draw_pull_details(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut fails: Vec<(String, String, &crate::protocol::PullOutcome)> = Vec::new();
    for (machine, s) in &app.machines.list {
        for (project, o) in &s.pulls {
            if !o.ok {
                fails.push((machine.clone(), project.clone(), o));
            }
        }
    }
    fails.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let popup = centered_rect(88, 78, area);
    frame.render_widget(Clear, popup);

    let page_h = popup.height.saturating_sub(2) as usize;
    let start = app
        .pull_details_scroll
        .min(fails.len().saturating_sub(page_h));
    let mut text: Vec<Line> = Vec::new();
    if fails.is_empty() {
        text.push(Line::from(""));
        text.push(Line::from(Span::styled(
            "   Нет упавших pulls на текущем снимке флота ✓",
            Style::default().fg(Color::Green),
        )));
    } else {
        for (machine, project, o) in fails.iter().skip(start).take(page_h) {
            let mut spans = vec![Span::styled(
                format!(" {machine}/{project}"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )];
            spans.push(Span::styled(
                format!(
                    "  ✗ attempts={}  finished {}",
                    o.attempts,
                    if o.finished_at > 0 {
                        fmt_ago(o.finished_at)
                    } else {
                        "—".into()
                    }
                ),
                Style::default().fg(Color::DarkGray),
            ));
            text.push(Line::from(spans));
            if let Some(e) = &o.error {
                text.push(Line::from(Span::styled(
                    format!("      {e}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }

    let block = Block::bordered().title(Span::styled(
        format!(
            " Failed pulls ({}) — [e]/[Esc] close  [↑↓/wheel] scroll{} ",
            fails.len(),
            if start > 0 {
                format!(
                    " [{}..{}]",
                    start,
                    start + text.len().min(page_h.min(fails.len()))
                )
            } else {
                String::new()
            }
        ),
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(text).block(block), popup);
}

// --- DOCTOR ---
