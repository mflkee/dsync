use super::*;

pub(super) fn draw_projects(frame: &mut Frame, app: &mut App, area: Rect) {
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
