use crate::app::{App, AppMode};
use crate::ble::parse_ble_device_uri;
use crate::ui::editors::render_progress_overlay;
use crate::ui::render_shortcut;
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub fn render_bin_flash_view(f: &mut Frame, app: &mut App, area: Rect) {
    if app.simulator_mode {
        let file = app.bin_file_path.as_ref().map_or_else(
            || "No firmware file loaded".to_string(),
            |path| format!("Local file: {}", path.display()),
        );
        f.render_widget(
            Paragraph::new(vec![
                Line::from("Simulator: flashing unsupported"),
                Line::from(""),
                Line::from("Firmware files can be loaded locally for inspection."),
                Line::from("Flash, bootloader entry and reboot are disabled."),
                Line::from("No physical radio is accessed."),
                Line::from(""),
                Line::from(file),
                Line::from(format!(
                    "{} bytes loaded",
                    app.bin_firmware_data.as_ref().map_or(0, Vec::len)
                )),
                Line::from(""),
                Line::from(vec![
                    render_shortcut("i"),
                    Span::raw(" import local firmware"),
                ]),
            ])
            .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Flash ")
                    .border_style(Style::default().fg(COLOR_BORDER)),
            ),
            area,
        );
        return;
    }
    let compact = area.width < 90;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(12),
            Constraint::Length(if compact { 2 } else { 3 }),
        ])
        .split(area);

    let blocked = flash_block_reason(app);
    let card_width = if compact {
        chunks[0].width.saturating_sub(4).clamp(48, 64)
    } else {
        chunks[0].width.saturating_sub(12).clamp(58, 78)
    };
    let card_height = if compact { 13 } else { 15 };
    let card_col = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(card_width),
            Constraint::Fill(1),
        ])
        .split(chunks[0]);
    let card_row = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(card_height.min(chunks[0].height.saturating_sub(1))),
            Constraint::Fill(1),
        ])
        .split(card_col[1]);

    let title = if blocked.is_none() {
        "File + USB Selected"
    } else {
        "Flash Firmware"
    };
    let subtitle = blocked.unwrap_or("Bootloader state not verified. Follow steps before f.");

    let file_summary = app.bin_file_path.as_ref().map_or_else(
        || "No firmware file loaded".to_string(),
        |path| {
            format!(
                "File: {} | {} bytes",
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy(),
                app.bin_firmware_data.as_ref().map_or(0, Vec::len)
            )
        },
    );
    let workflow = Paragraph::new(if compact {
        vec![
            Line::styled(
                title,
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Line::styled(subtitle, Style::default().fg(COLOR_DIM)),
            workflow_step(1, vec![Span::raw("i import .bin firmware")]),
            workflow_step(2, vec![Span::raw("Power the radio fully off")]),
            workflow_step(3, vec![Span::raw("Hold PTT while powering on")]),
            workflow_step(
                4,
                vec![Span::raw(if blocked.is_some() {
                    "f disabled until requirements met"
                } else {
                    "f flash after bootloader entry"
                })],
            ),
            Line::styled(file_summary, Style::default().fg(COLOR_DIM)),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                title,
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(subtitle, Style::default().fg(COLOR_DIM))),
            Line::from(""),
            workflow_step(
                1,
                vec![
                    Span::raw("Press "),
                    render_shortcut("i"),
                    Span::raw(" to import the firmware "),
                    Span::styled(".bin", Style::default().fg(COLOR_ACCENT)),
                ],
            ),
            workflow_step(2, vec![Span::raw("Power the radio fully off")]),
            workflow_step(
                3,
                vec![Span::raw(
                    "Hold PTT while powering on to enter bootloader mode",
                )],
            ),
            workflow_step(
                4,
                vec![
                    Span::raw("Press "),
                    render_shortcut("f"),
                    Span::raw(if blocked.is_some() {
                        " disabled until requirements met"
                    } else {
                        " only after bootloader entry"
                    }),
                ],
            ),
            Line::from(""),
            Line::from(Span::styled(file_summary, Style::default().fg(COLOR_DIM))),
        ]
    })
    .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Flash ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(workflow, card_row[1]);

    let mut hints = vec![render_shortcut("i"), Span::raw(" import firmware")];
    if blocked.is_none() {
        hints.extend([Span::raw(" | "), render_shortcut("f"), Span::raw(" flash")]);
    } else {
        hints.push(Span::styled(
            " | f disabled",
            Style::default().fg(COLOR_DIM),
        ));
    }
    let footer = Paragraph::new(Line::from(hints))
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(footer, chunks[1]);
}

fn flash_block_reason(app: &App) -> Option<&'static str> {
    if app.demo_mode {
        Some("Demo: hardware flashing disabled.")
    } else if app.activity_active() {
        Some("Stop Activity; wait for cleanup.")
    } else if app.remote_active || app.remote_tx.is_some() {
        Some("Stop Remote; wait for cleanup.")
    } else if app.read_all_request.is_some()
        || matches!(
            app.mode,
            AppMode::Reading | AppMode::Writing | AppMode::BinFlashing
        )
    {
        Some("Wait for the current radio operation.")
    } else if app
        .bin_firmware_data
        .as_ref()
        .is_none_or(|data| data.is_empty() || data.len() > 0xF800)
    {
        Some("Import .bin firmware: 1..=63488 bytes required.")
    } else if app.ble_transport_selected()
        || app
            .protocol_port_name
            .as_deref()
            .is_some_and(|port| parse_ble_device_uri(port).is_some())
    {
        Some("USB required; BLE flashing disabled.")
    } else if app.protocol_port_name.is_none() {
        Some("Select a USB radio target first.")
    } else {
        None
    }
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

fn workflow_step(number: usize, content: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{number}. "),
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    spans.extend(content);
    Line::from(spans)
}
