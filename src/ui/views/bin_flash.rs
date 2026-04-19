use crate::app::App;
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
    let compact = area.width < 90;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(12),
            Constraint::Length(if compact { 2 } else { 3 }),
        ])
        .split(area);

    let file_ready = app.bin_firmware_data.is_some();
    let radio_ready = app.protocol_port_name.is_some();
    let flash_ready = file_ready && radio_ready;
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

    let title = if flash_ready {
        "Ready To Flash"
    } else {
        "Flash Firmware"
    };
    let subtitle = if flash_ready {
        "The image and radio are ready. Follow the last step to start."
    } else if !file_ready {
        "Import the firmware first, then follow the bootloader steps."
    } else {
        "The firmware is loaded. Put the radio into bootloader mode next."
    };

    let workflow = Paragraph::new(vec![
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
                Span::raw(" once the image and radio are both ready"),
            ],
        ),
        Line::from(""),
        Line::from(Span::styled(
            if flash_ready {
                "Then hold PTT, power on, and press f."
            } else if !file_ready {
                "Start with step 1."
            } else if radio_ready {
                "The remaining step is bootloader mode."
            } else {
                "Connect the radio, then enter bootloader mode."
            },
            Style::default().fg(COLOR_DIM),
        )),
    ])
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

    let footer = Paragraph::new(Line::from(vec![
        render_shortcut("i"),
        Span::raw(" import firmware | "),
        render_shortcut("f"),
        Span::raw(" flash"),
    ]))
    .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(footer, chunks[1]);
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
