use super::*;

pub(super) fn draw_log(frame: &mut Frame, app: &mut App, area: Rect) {
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
