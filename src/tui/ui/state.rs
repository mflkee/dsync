use super::*;

pub(super) fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

pub(super) fn draw_state(frame: &mut Frame, app: &mut App, area: Rect) {
    let [config_area, health_area] =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).areas(area);

    // Слева — эффективная конфигурация [state].
    let mut cfg_lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(" tmux sync      : ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                on_off(app.cfg.state.tmux),
                Style::default().fg(if app.cfg.state.tmux {
                    Color::Green
                } else {
                    Color::Red
                }),
            ),
            Span::styled("   [t]", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled(" auto-restore   : ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                on_off(app.cfg.state.tmux_restore),
                Style::default().fg(if app.cfg.state.tmux_restore {
                    Color::Green
                } else {
                    Color::Red
                }),
            ),
            Span::styled("   [T]", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " opencode sessions",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   [n]", Style::default().fg(Color::Yellow)),
        ]),
    ];
    match &app.cfg.state.opencode_projects {
        Some(list) => {
            for p in list {
                cfg_lines.push(Line::from(Span::styled(
                    format!("   · {p}"),
                    Style::default().fg(Color::White),
                )));
            }
        }
        None => {
            cfg_lines.push(Line::from(Span::styled(
                "   · all [projects.*] (не задан список)",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    if !app.cfg.state.configured {
        cfg_lines.push(Line::from(Span::styled(
            "   (секции [state] нет — дефолты: tmux on, restore off)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    if app.cfg.chezmoi_managed {
        cfg_lines.push(Line::from(""));
        cfg_lines.push(Line::from(Span::styled(
            " config under chezmoi: changes are confirmed and applied",
            Style::default().fg(Color::Yellow),
        )));
    }
    let block = Block::bordered().title(Span::styled(
        " State config ",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(cfg_lines).block(block), config_area);

    // Справа — сводка с хаба (state_status). «unavailable» — хаб старый/недоступен.
    let mut health: Vec<Line> = Vec::new();
    if let Some(err) = &app.state_status_error {
        health.push(Line::from(Span::styled(
            " ⚠ state_status: unavailable",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        health.push(Line::from(Span::styled(
            format!("   {err}"),
            Style::default().fg(Color::Red),
        )));
        health.push(Line::from(Span::styled(
            "   (нужен dsync-hub с поддержкой state_status; [r] — retry)",
            Style::default().fg(Color::DarkGray),
        )));
    } else if app.state_channels.is_empty() {
        health.push(Line::from(Span::styled(
            if app.state_status_ts > 0 {
                " No state synced yet (каналы пусты)."
            } else {
                " ⟳ ждём сводку с хаба… ([r] обновить)"
            },
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for ch in &app.state_channels {
            let mut line = vec![Span::styled(
                format!(" {} ", ch.channel),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )];
            if let Some(err) = &ch.error {
                line.push(Span::styled("✗", Style::default().fg(Color::Red)));
                line.push(Span::styled(
                    format!("  {err}"),
                    Style::default().fg(Color::Red),
                ));
            } else {
                line.push(Span::styled("✓", Style::default().fg(Color::Green)));
            }
            health.push(Line::from(line));
            health.push(Line::from(Span::styled(
                format!(
                    "     items: {}   last: {}  by {}",
                    ch.item_count,
                    if ch.last_updated > 0 {
                        format!(
                            "{} ({})",
                            fmt_ago(ch.last_updated),
                            fmt_civil(ch.last_updated)
                        )
                    } else {
                        "—".into()
                    },
                    if ch.last_origin.is_empty() {
                        "—"
                    } else {
                        &ch.last_origin
                    },
                ),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    let block = Block::bordered().title(Span::styled(
        format!(
            " Hub state status{} ",
            if app.state_status_ts > 0 {
                format!(" ({} refreshed)", fmt_clock(app.state_status_ts))
            } else {
                String::new()
            }
        ),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(health).block(block), health_area);
}

// --- FAILED PULLS (оверлей) ---
