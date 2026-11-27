use super::*;

pub(super) fn draw_doctor(frame: &mut Frame, app: &mut App, area: Rect) {
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
