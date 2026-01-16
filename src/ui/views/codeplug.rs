use crate::app::App;
use crate::ui::render_shortcut;
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub fn render_codeplug_view(f: &mut Frame, app: &mut App, area: Rect) {
    if app.codeplug_data.is_none() && app.channels.is_empty() {
        // Show empty state hint
        let hint =
            Paragraph::new("\n\nNO CODEPLUG LOADED\n\nPress 'i' to import a codeplug file (.nfw)")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
        f.render_widget(hint, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // Main content area
    let mut content_lines = vec![];

    // Show loaded codeplug info
    if let Some(path) = &app.codeplug_path {
        let path_str = path.to_string_lossy().into_owned();
        content_lines.push(Line::from(""));
        content_lines.push(Line::from(vec![
            Span::styled(
                "  Loaded File: ",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(path_str, Style::default().fg(Color::White)),
        ]));
        content_lines.push(Line::from(""));

        if let Some(data) = &app.codeplug_data {
            let size_kb = data.len() as f64 / 1024.0;
            content_lines.push(Line::from(vec![
                Span::styled("  Size: ", Style::default().fg(COLOR_ACCENT)),
                Span::styled(
                    format!("{:.1} KB (8192 bytes)", size_kb),
                    Style::default().fg(Color::White),
                ),
            ]));
            content_lines.push(Line::from(""));
            content_lines.push(Line::from(vec![
                Span::styled("  Status: ", Style::default().fg(COLOR_ACCENT)),
                Span::styled("Ready to write", Style::default().fg(COLOR_SUCCESS)),
            ]));
        }
    } else {
        content_lines.push(Line::from(""));
        content_lines.push(Line::from(vec![
            Span::styled("  Status: ", Style::default().fg(COLOR_ACCENT)),
            Span::raw("No codeplug loaded"),
        ]));
    }

    content_lines.push(Line::from(""));
    content_lines.push(Line::from(""));
    content_lines.push(Line::from(vec![Span::styled(
        "  Actions:",
        Style::default()
            .fg(COLOR_PRIMARY)
            .add_modifier(Modifier::BOLD),
    )]));
    content_lines.push(Line::from(""));

    content_lines.push(Line::from(vec![
        Span::raw("    "),
        render_shortcut("i"),
        Span::raw("  Import codeplug from .nfw file"),
    ]));

    if !app.channels.is_empty() && app.settings.is_some() {
        content_lines.push(Line::from(vec![
            Span::raw("    "),
            render_shortcut("e"),
            Span::raw("  Export current config to .nfw file"),
        ]));
    }

    if app.codeplug_data.is_some() {
        content_lines.push(Line::from(vec![
            Span::raw("    "),
            render_shortcut("w"),
            Span::styled(
                "  Write codeplug to radio",
                Style::default().fg(COLOR_SUCCESS),
            ),
        ]));
    }

    let content = Paragraph::new(content_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_PRIMARY))
            .title(" CODEPLUG MANAGER "),
    );
    f.render_widget(content, chunks[0]);

    // Hints footer (matching other tabs' style)
    let mut hints = vec![render_shortcut("i"), Span::raw(" import")];

    if !app.channels.is_empty() && app.settings.is_some() {
        hints.push(Span::raw(" | "));
        hints.push(render_shortcut("e"));
        hints.push(Span::raw(" export"));
    }

    if app.codeplug_data.is_some() {
        hints.push(Span::raw(" | "));
        hints.push(render_shortcut("w"));
        hints.push(Span::raw(" write"));
    }

    let hint_line = Paragraph::new(Line::from(hints))
        .style(Style::default().bg(Color::Rgb(40, 40, 40)))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_PRIMARY)),
        );
    f.render_widget(hint_line, chunks[1]);
}
