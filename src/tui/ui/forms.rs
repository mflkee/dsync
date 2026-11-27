use super::*;

// --- FORM OVERLAY ---

/// Разрез значения поля по позиции курсора (символы → байтовый индекс).
pub(super) fn split_around(s: &str, pos: usize) -> (String, String) {
    let bi = crate::tui::app::char_pos_to_byte(s, pos);
    (s[..bi].to_string(), s[bi..].to_string())
}

pub(super) fn draw_form(frame: &mut Frame, app: &App, area: Rect) {
    let Some(form) = &app.form else { return };
    let is_confirm = form.action.is_some();

    let (w, h) = if is_confirm { (60, 30) } else { (70, 55) };
    let popup = centered_rect(w, h, area);
    frame.render_widget(Clear, popup);

    let mut lines: Vec<Line> = Vec::new();
    if is_confirm {
        let detail = &form.fields[0].value;
        let lead = match form.action.as_ref() {
            Some(ConfirmAction::RemoveProject(_)) | Some(ConfirmAction::RemoveRemote(_)) => {
                "   Это изменит конфиг dsync:"
            }
            Some(ConfirmAction::ChezmoiApply(_)) => {
                "   chezmoi-managed config — запись в шаблон + `chezmoi apply`:"
            }
            Some(ConfirmAction::StateToggle { .. }) => "   Переключение [state]:",
            None => "   Подтверждение:",
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            lead,
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
        for dl in detail.lines() {
            lines.push(Line::from(Span::styled(
                format!("   {dl}"),
                Style::default().fg(Color::White),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "   [Enter] — подтвердить   [Esc] — отмена",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(""));
        for (i, f) in form.fields.iter().enumerate() {
            let active = i == form.cursor;
            let arrow = if active { "▸ " } else { "  " };
            // Значение активного поля рендерится с кареткой на позиции курсора.
            let value: Vec<Span> = if active {
                let (before, after) = split_around(&f.value, f.cursor);
                vec![
                    Span::styled(before, Style::default().fg(Color::White)),
                    Span::styled(
                        "│",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(after, Style::default().fg(Color::White)),
                ]
            } else {
                vec![Span::styled(
                    f.value.clone(),
                    Style::default().fg(Color::White),
                )]
            };
            lines.push(Line::from(
                std::iter::once(Span::styled(arrow, Style::default().fg(Color::Cyan)))
                    .chain(std::iter::once(Span::styled(
                        format!("{}: ", f.label),
                        if active {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    )))
                    .chain(value)
                    .collect::<Vec<_>>(),
            ));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "   [Tab/↑↓] поле   [←/→/Home/End] курсор   [⌫/Del] правка   [Ctrl-U] очистить",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "   [Enter] следующее / сохранить   [Esc] отмена",
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
