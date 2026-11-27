use super::*;

pub(super) fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn help_lines() -> Vec<Line<'static>> {
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
        Line::from("   [e]                — failed pulls overlay (все машины флота)"),
        Line::from(""),
        Line::from(Span::styled(
            " STATE (tmux / opencode)",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [t]                — toggle tmux-resurrect sync (подтверждение)"),
        Line::from("   [T]                — toggle tmux auto-restore (подтверждение)"),
        Line::from("   [n]                — edit opencode project list (пусто = все projects.*)"),
        Line::from("   [r]                — refresh hub state summary"),
        Line::from("   Правая панель — сводка с хаба; старый хаб → «unavailable»."),
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
            " FORMS",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("   [Tab]/[↑↓]         — switch field"),
        Line::from("   [←/→] [Home]/[End] — move cursor"),
        Line::from("   [⌫]/[Del] [Ctrl-U] — backspace / delete / clear field"),
        Line::from("   На chezmoi-managed конфиге каждая запись конфига подтверждается"),
        Line::from("   и пишется в шаблон, затем `chezmoi apply` регенерирует живой файл."),
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
        Line::from("   Конфиг правится точечными патчами (toml_edit): комментарии"),
        Line::from("   и неизвестные секции сохраняются; chezmoi-правки идут через"),
        Line::from("   шаблон (в т.ч. Go-template файлы — текстовый движок по регионам)."),
        Line::from("   Колесо мыши скроллит списки/лог/доктора (если поддержано)."),
    ]
}
