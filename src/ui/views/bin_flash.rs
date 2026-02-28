use crate::app::App;
use crate::ui::editors::render_progress_overlay;
use crate::ui::render_shortcut;
use crate::ui::theme::*;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render_bin_flash_view(f: &mut Frame, app: &mut App, area: Rect) {
    let mut content_lines = vec![];

    content_lines.push(Line::from(vec![Span::styled(
        " BIN FIRMWARE FLASHER ",
        Style::default()
            .fg(COLOR_PRIMARY)
            .add_modifier(Modifier::BOLD),
    )]));
    content_lines.push(Line::from(""));

    content_lines.push(Line::from(vec![Span::styled(
        " INSTRUCTIONS: ",
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD),
    )]));
    content_lines.push(Line::from(""));
    content_lines.push(Line::from(vec![Span::raw(" 1. Turn OFF your radio")]));
    content_lines.push(Line::from(vec![Span::raw(" 2. Hold PTT button")]));
    content_lines.push(Line::from(vec![Span::raw(
        " 3. While holding the button, turn ON the radio",
    )]));
    content_lines.push(Line::from(vec![Span::raw(
        " 4. Select a .bin firmware file and press F to flash",
    )]));
    content_lines.push(Line::from(""));

    if let Some(path) = &app.bin_file_path {
        let path_str = path.to_string_lossy().into_owned();
        content_lines.push(Line::from(vec![
            Span::styled(
                " Selected File: ",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(path_str, Style::default().fg(Color::White)),
        ]));

        if let Some(data) = &app.bin_firmware_data {
            let size_kb = data.len() as f64 / 1024.0;
            let blocks = data.len() / 32;
            content_lines.push(Line::from(vec![
                Span::styled(" Size: ", Style::default().fg(COLOR_ACCENT)),
                Span::styled(
                    format!("{:.1} KB ({} blocks)", size_kb, blocks),
                    Style::default().fg(Color::White),
                ),
            ]));
            content_lines.push(Line::from(""));
        }
    } else {
        content_lines.push(Line::from(vec![Span::raw(" No firmware file selected")]));
        content_lines.push(Line::from(""));
    }

    let connected = app.protocol_port_name.is_some();
    content_lines.push(Line::from(vec![
        Span::styled(" Connection: ", Style::default().fg(COLOR_ACCENT)),
        if connected {
            Span::styled("Connected", Style::default().fg(COLOR_SUCCESS))
        } else {
            Span::styled("Not Connected", Style::default().fg(Color::Red))
        },
    ]));

    content_lines.push(Line::from(""));
    content_lines.push(Line::from(vec![Span::styled(
        " ACTIONS: ",
        Style::default()
            .fg(COLOR_PRIMARY)
            .add_modifier(Modifier::BOLD),
    )]));
    content_lines.push(Line::from(""));

    content_lines.push(Line::from(vec![
        Span::raw("    "),
        render_shortcut("i"),
        Span::raw("  Select .bin firmware file"),
    ]));

    if app.bin_firmware_data.is_some() && app.protocol_port_name.is_some() {
        content_lines.push(Line::from(vec![
            Span::raw("    "),
            render_shortcut("f"),
            Span::styled("  Start Flashing", Style::default().fg(COLOR_SUCCESS)),
        ]));
    }

    let status_text = if app.bin_firmware_data.is_some() && app.protocol_port_name.is_none() {
        Span::styled(
            " (Connect to radio first)",
            Style::default().fg(Color::Yellow),
        )
    } else if app.bin_firmware_data.is_none() {
        Span::styled(
            " (Select firmware file first)",
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::raw("")
    };

    if app.bin_firmware_data.is_some() && app.protocol_port_name.is_some() {
        content_lines.push(Line::from(status_text));
    } else {
        content_lines.push(Line::from(""));
        content_lines.push(Line::from(status_text));
    }

    let content = Paragraph::new(content_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_PRIMARY)),
    );
    f.render_widget(content, area);
}

pub fn render_bin_flash_overlay(f: &mut Frame, app: &App, area: Rect) {
    render_progress_overlay(
        f,
        app,
        area,
        "FLASHING IN PROGRESS",
        Some("1. Turn OFF radio\n2. Hold PTT, turn ON radio"),
    )
}
