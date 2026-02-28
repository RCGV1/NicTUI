use crate::app::App;
use crate::ui::theme::*;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
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

    // Show channel count
    let channel_count = app.channels.len();
    let deleted_count = app.deleted_channels.len();
    content_lines.push(Line::from(vec![
        Span::styled("  Channels: ", Style::default().fg(COLOR_ACCENT)),
        Span::styled(
            format!("{} (+ {} deleted)", channel_count, deleted_count),
            Style::default().fg(Color::White),
        ),
    ]));

    // Show settings info
    if app.settings.is_some() {
        content_lines.push(Line::from(vec![
            Span::styled("  Radio Settings: ", Style::default().fg(COLOR_ACCENT)),
            Span::styled("Loaded", Style::default().fg(COLOR_SUCCESS)),
        ]));
    }

    // Show import/export hints
    content_lines.push(Line::from(""));
    content_lines.push(Line::from(vec![
        Span::styled("  Hint: ", Style::default().fg(COLOR_DIM)),
        Span::raw("Press 'i' to import, 'e' to export, 'w' to write to radio"),
    ]));

    let content = Paragraph::new(content_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_PRIMARY))
            .title(" CODEPLUG MANAGER "),
    );
    f.render_widget(content, area);
}
