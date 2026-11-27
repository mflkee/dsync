use super::*;

pub(super) fn draw_dashboard(frame: &mut Frame, app: &mut App, area: Rect) {
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
