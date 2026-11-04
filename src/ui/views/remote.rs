use crate::app::{App, RemoteScreen};
use crate::protocol::RemotePacket;
use crate::remote::{RemoteEvidenceKind, RemoteSessionFailureKind};
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_ERROR, COLOR_PRIMARY, COLOR_SECONDARY,
    COLOR_SUCCESS, COLOR_SURFACE_0, COLOR_SURFACE_1, COLOR_SURFACE_2, COLOR_SURFACE_3, COLOR_TEXT,
    COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct RemoteKey {
    label: &'static str,
    shortcut: &'static str,
    code: u8,
}

const fn key(label: &'static str, shortcut: &'static str, code: u8) -> Option<RemoteKey> {
    Some(RemoteKey {
        label,
        shortcut,
        code,
    })
}

const REMOTE_KEYS: [Option<RemoteKey>; 20] = [
    key("Menu", "m", 0x0B),
    key("1", "1", 0x02),
    key("2", "2", 0x03),
    key("3", "3", 0x04),
    key("*", "*", 0x0F),
    key("Up", "u", 0x0C),
    key("4", "4", 0x05),
    key("5", "5", 0x06),
    key("6", "6", 0x07),
    key("0", "0", 0x01),
    key("Down", "d", 0x0D),
    key("7", "7", 0x08),
    key("8", "8", 0x09),
    key("9", "9", 0x0A),
    key("#", "#", 0x10),
    key("Exit", "e/Esc", 0x0E),
    None,
    key("F", "f", 0x12),
    key("B", "v/b", 0x11),
    key("PTT", "a", 0x13),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteSessionVerdict {
    badge: &'static str,
    headline: &'static str,
    detail: String,
    accent: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteScreenLayout {
    display: Rect,
    keypad: Rect,
    keypad_help: Rect,
    overview: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteDiagnosticsLayout {
    overview: Rect,
    preview: Rect,
    status: Rect,
    activity: Rect,
    controls: Rect,
}

pub fn render_remote_screen(f: &mut Frame, app: &App, area: Rect) {
    if !app.remote_screen.show_diagnostics {
        let layout = remote_screen_layout(area);
        render_lcd_preview(f, app, layout.display);
        render_keypad(f, app, layout.keypad);
        render_keypad_help(f, app, layout.keypad_help);
        render_compact_overview(f, app, layout.overview);
        return;
    }

    let layout = remote_diagnostics_layout(area);

    render_remote_overview(f, app, layout.overview);
    render_lcd_preview(f, app, layout.preview);
    render_status_sidebar(f, app, layout.status);
    render_activity(f, app, layout.activity);
    render_controls(f, app, layout.controls);
}

fn remote_screen_layout(area: Rect) -> RemoteScreenLayout {
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let keypad_width = if area.width < 54 {
        area.width / 2
    } else {
        (area.width / 3).clamp(27, 37)
    };
    let body =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(keypad_width)]).split(rows[0]);
    let keypad = Layout::vertical([Constraint::Length(18), Constraint::Min(0)]).split(body[1]);
    RemoteScreenLayout {
        display: body[0],
        keypad: keypad[0],
        keypad_help: keypad[1],
        overview: rows[1],
    }
}

/// Hit-test the mirror keypad using the remote content rect, not the terminal rect.
/// Callers must suppress keypad input while the diagnostics view is showing.
pub fn remote_key_at(area: Rect, col: u16, row: u16) -> Option<u8> {
    keypad_cells(remote_screen_layout(area).keypad)
        .into_iter()
        .zip(REMOTE_KEYS)
        .find_map(|(cell, key)| {
            key.filter(|_| cell.contains((col, row).into()))
                .map(|key| key.code)
        })
}

fn keypad_cells(area: Rect) -> [Rect; 20] {
    let inner = Block::bordered().inner(area);
    std::array::from_fn(|index| {
        let col = index as u32 % 5;
        let row = index as u32 / 5;
        let x = u32::from(inner.width) * col / 5;
        let y = u32::from(inner.height) * row / 4;
        Rect::new(
            inner.x + x as u16,
            inner.y + y as u16,
            (u32::from(inner.width) * (col + 1) / 5 - x) as u16,
            (u32::from(inner.height) * (row + 1) / 4 - y) as u16,
        )
    })
}

fn render_keypad(f: &mut Frame, app: &App, area: Rect) {
    let controls_ready = app.remote_controls_ready();
    f.render_widget(
        Block::bordered()
            .title(" Keypad: may save state ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_0),
        area,
    );
    let cells = keypad_cells(area);
    // Use the smallest cell so rounding never mixes button styles within a grid.
    let bordered = cells.iter().all(|cell| cell.width >= 6 && cell.height >= 4);
    let underlined = cells.iter().all(|cell| cell.width >= 5 && cell.height >= 3);
    for (cell, key) in cells.into_iter().zip(REMOTE_KEYS) {
        let Some(key) = key else { continue };
        let accent = match key.code {
            0x0B => COLOR_PRIMARY,
            0x0E => COLOR_ERROR,
            0x13 => COLOR_WARNING,
            _ => COLOR_BORDER,
        };
        let block = if bordered {
            Block::bordered()
        } else if underlined {
            Block::default().borders(Borders::BOTTOM)
        } else {
            Block::default()
        }
        .border_style(Style::default().fg(accent))
        .bg(COLOR_SURFACE_1);
        let inner = block.inner(cell);
        f.render_widget(block, cell);
        let mut lines = vec![Line::from(Span::styled(
            key.label,
            Style::default().fg(COLOR_TEXT).bold(),
        ))];
        if inner.height >= 2 {
            lines.push(Line::from(Span::styled(
                if key.label == key.shortcut {
                    ""
                } else {
                    key.shortcut
                },
                Style::default().fg(COLOR_DIM),
            )));
        }
        let height = (lines.len() as u16).min(inner.height);
        let centered = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(height) / 2,
            inner.width,
            height,
        );
        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .style(Style::default().bg(COLOR_SURFACE_1))
                .add_modifier(if controls_ready {
                    Modifier::empty()
                } else {
                    Modifier::DIM
                }),
            centered,
        );
    }
}

fn render_keypad_help(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT)
        .border_style(Style::default().fg(COLOR_BORDER));
    let inner = block.inner(area);
    if inner.height == 0 || inner.width < 16 {
        return;
    }
    let lines = [
        Line::from("PTT can transmit"),
        Line::from("Click or type a shortcut"),
        Line::from(format!("Battery: {}", battery_label(app))),
        Line::from(format!("Status: {}", status_label(app))),
    ]
    .into_iter()
    .filter(|line| line.width() <= usize::from(inner.width))
    .take(usize::from(inner.height))
    .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(COLOR_DIM).bg(COLOR_SURFACE_0))
            .block(block),
        area,
    );
}

fn render_compact_overview(f: &mut Frame, app: &App, area: Rect) {
    let verdict = remote_session_verdict(app.remote_active, &app.remote_screen);
    let freshness = freshness_summary(app);
    let (badge, headline, accent) =
        if !app.remote_active && app.remote_screen.last_failure.is_some() {
            (verdict.badge, verdict.headline, verdict.accent)
        } else if !app.remote_active && app.remote_screen.has_display_updates {
            (" CACHED ", "session stopped", COLOR_DIM)
        } else if !app.remote_active {
            (" INACTIVE ", "session stopped", COLOR_DIM)
        } else {
            match app.remote_screen.phase {
                crate::remote::RemoteSessionPhase::Opening => {
                    (" OPENING ", "waiting for radio", COLOR_PRIMARY)
                }
                crate::remote::RemoteSessionPhase::Recovering => {
                    (" RETRY ", "reconnecting", COLOR_WARNING)
                }
                _ if app.remote_screen.has_display_updates => {
                    (" PARTIAL ", "new updates only", COLOR_PRIMARY)
                }
                _ => (" WAITING ", "for display updates", COLOR_PRIMARY),
            }
        };
    let mut spans = vec![
        Span::styled(
            badge,
            Style::default().fg(COLOR_SURFACE_0).bg(accent).bold(),
        ),
        Span::styled(format!(" {headline}"), Style::default().fg(accent)),
    ];
    if let Some(report) = &app.remote_screen.last_control_report {
        let color = if !report.success || report.evidence == RemoteEvidenceKind::CommandFailed {
            COLOR_ERROR
        } else if report.evidence == RemoteEvidenceKind::ControlConfirmed {
            COLOR_SUCCESS
        } else {
            COLOR_WARNING
        };
        spans.extend([
            Span::styled(" | Last: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                truncate_line(
                    &report.label,
                    area.width.saturating_sub(45).clamp(4, 24) as usize,
                ),
                Style::default().fg(COLOR_TEXT).bold(),
            ),
            Span::styled(
                format!(
                    " ({})",
                    match (report.success, report.evidence) {
                        (false, _) | (_, RemoteEvidenceKind::CommandFailed) => "failed",
                        (true, RemoteEvidenceKind::ControlConfirmed) => "confirmed",
                        _ => "not confirmed",
                    }
                ),
                Style::default().fg(color),
            ),
        ]);
    } else {
        spans.push(Span::styled(
            format!(" | {}", freshness.1),
            Style::default().fg(COLOR_DIM),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans))
            .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1)),
        area,
    );
}

fn remote_diagnostics_layout(area: Rect) -> RemoteDiagnosticsLayout {
    let summary_height = if area.height <= 30 {
        4
    } else if area.width < 100 {
        5
    } else {
        4
    };
    let detail_height = if area.height <= 24 {
        6
    } else if area.height <= 30 {
        7
    } else if area.width < 110 {
        10
    } else {
        8
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Min(if area.height < 24 { 5 } else { 12 }),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let body = if chunks[1].width < 76 && chunks[1].height >= 20 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Length(8)])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(chunks[1])
    };

    let detail_split = if chunks[2].width < 76 && chunks[2].height >= 9 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(5)])
            .split(chunks[2])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(chunks[2])
    };

    RemoteDiagnosticsLayout {
        overview: chunks[0],
        preview: body[0],
        status: body[1],
        activity: detail_split[0],
        controls: detail_split[1],
    }
}

fn render_lcd_preview(f: &mut Frame, app: &App, area: Rect) {
    let has_updates = app.remote_screen.has_display_updates;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(if has_updates && !app.remote_active {
            " Radio Display: cached "
        } else if has_updates {
            " Radio Display: updates only "
        } else {
            " Radio Display "
        })
        .border_style(if app.remote_active {
            Style::default().fg(COLOR_SUCCESS)
        } else {
            Style::default().fg(COLOR_BORDER)
        })
        .bg(COLOR_SURFACE_0);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(inner);
    let leds =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[0]);
    for (area, name, state) in [
        (leds[0], "Left", app.remote_screen.leds & 3),
        (leds[1], "Right", (app.remote_screen.leds >> 2) & 3),
    ] {
        let name = if area.width < 18 {
            if name == "Left" { "L" } else { "R" }
        } else {
            name
        };
        let (label, color) = if app.remote_screen.last_led_update.is_some() {
            led_visual(state)
        } else {
            ("unknown", COLOR_DIM)
        };
        f.render_widget(
            Paragraph::new(if area.width < 18 {
                format!("{name}: {label}")
            } else {
                format!("{name} LED: {label}")
            })
            .alignment(Alignment::Center)
            .style(Style::default().fg(color)),
            area,
        );
    }
    let mut completeness = if has_updates {
        "Screen may be incomplete"
    } else if !app.remote_active {
        "Remote is not running"
    } else if rows[1].width >= 36 {
        "Only new display updates are sent"
    } else {
        "Only new updates are sent"
    }
    .to_string();
    if has_updates
        && (app.remote_screen.battery_text.is_some() || app.remote_screen.battery_level.is_some())
    {
        let battery = battery_label(app);
        completeness.push_str(&format!(" | {battery}"));
        if completeness.chars().count() > usize::from(rows[1].width) {
            completeness = format!("May be incomplete | {battery}");
        }
    }
    f.render_widget(
        Paragraph::new(completeness)
            .alignment(Alignment::Center)
            .style(Style::default().fg(COLOR_DIM)),
        rows[1],
    );
    if has_updates {
        app.remote_screen.display.render(f, rows[2]);
    } else {
        let lines = if app.remote_active {
            vec![
                Line::from(Span::styled(
                    "Waiting for radio display updates",
                    Style::default().fg(COLOR_TEXT).bold(),
                )),
                Line::from(""),
                Line::from("An idle radio may not redraw."),
            ]
        } else {
            vec![
                Line::from("No screen received"),
                Line::from(""),
                Line::from(vec![
                    render_shortcut("o"),
                    Span::styled(
                        " Start Remote to receive display updates",
                        Style::default().fg(COLOR_TEXT),
                    ),
                ]),
            ]
        };
        let height = lines
            .iter()
            .map(|line| (line.width().max(1) as u16).div_ceil(rows[2].width.max(1)))
            .sum::<u16>()
            .saturating_add(1)
            .min(rows[2].height);
        let placeholder = Rect::new(
            rows[2].x,
            rows[2].y + rows[2].height.saturating_sub(height) / 2,
            rows[2].width,
            height,
        );
        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(COLOR_DIM)),
            placeholder,
        );
    }
}

fn led_visual(state: u8) -> (&'static str, Color) {
    match state & 3 {
        1 => ("green", COLOR_SUCCESS),
        2 => ("red", COLOR_ERROR),
        3 => ("yellow", COLOR_WARNING),
        _ => ("off", COLOR_DIM),
    }
}

fn render_status_sidebar(f: &mut Frame, app: &App, area: Rect) {
    if area.width < 36 || area.height < 18 {
        let signal = app.remote_screen.last_signal_update.map_or_else(
            || "Signal: unknown".to_string(),
            |_| format!("Signal: {} raw", app.remote_screen.signal_strength),
        );
        let noise = app.remote_screen.last_noise_update.map_or_else(
            || "Noise: unknown".to_string(),
            |_| format!("Noise: {} raw", app.remote_screen.noise_level),
        );
        let freshness = freshness_summary(app);
        let lines = [
            signal,
            noise,
            format!("Battery: {}", battery_label(app)),
            format!("Status: {}", status_label(app)),
            format!("Updates: {}", freshness.0),
            freshness.1,
            "Raw values, not %".to_string(),
        ]
        .into_iter()
        .map(|line| Line::from(truncate_line(&line, area.width.saturating_sub(2) as usize)))
        .collect::<Vec<_>>();
        f.render_widget(
            Paragraph::new(lines)
                .style(Style::default().fg(COLOR_TEXT))
                .block(
                    Block::bordered()
                        .title(" Radio Status ")
                        .border_style(Style::default().fg(COLOR_BORDER)),
                ),
            area,
        );
        return;
    }
    let metric_height = if area.height < 18 {
        4
    } else if area.height < 24 {
        5
    } else {
        6
    };
    let gauge_min = if area.height < 18 { 2 } else { 3 };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(metric_height),
            Constraint::Length(metric_height),
            Constraint::Length(metric_height),
            Constraint::Min(gauge_min),
        ])
        .split(area);

    let top_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vertical[0]);
    let mid_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vertical[1]);
    let bottom_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vertical[2]);

    let signal = app.remote_screen.signal_strength;
    let noise = app.remote_screen.noise_level;
    let battery_label = battery_label(app);
    let (battery_level, battery_color) = battery_visuals(app);
    let status_label = status_label(app);
    let session_verdict = remote_session_verdict(app.remote_active, &app.remote_screen);
    let verdict_value = session_verdict.badge.trim();
    let verdict_detail = session_summary(app.remote_active, app.remote_screen.phase);
    let link_detail = freshness_summary(app);

    render_metric_card(
        f,
        top_row[0],
        "Verdict",
        verdict_value,
        &verdict_detail,
        session_verdict.accent,
        if app.remote_active { "◉" } else { "○" },
    );
    render_metric_card(
        f,
        top_row[1],
        "Link",
        &link_detail.0,
        &link_detail.1,
        link_detail.2,
        "⟐",
    );
    render_metric_card(
        f,
        mid_row[0],
        "Signal",
        &app.remote_screen
            .last_signal_update
            .map_or_else(|| "unknown".to_string(), |_| format!("{signal} raw")),
        "uncalibrated radio value",
        COLOR_PRIMARY,
        "⌁",
    );
    render_metric_card(
        f,
        mid_row[1],
        "Noise Floor",
        &app.remote_screen
            .last_noise_update
            .map_or_else(|| "unknown".to_string(), |_| format!("{noise} raw")),
        "uncalibrated radio value",
        COLOR_SECONDARY,
        "≈",
    );
    render_metric_card(
        f,
        bottom_row[0],
        "Battery",
        &battery_label,
        &if app.remote_screen.battery_text.is_some() || app.remote_screen.battery_level.is_some() {
            format!("{} battery reading", battery_meter(battery_level))
        } else {
            "no battery reading".to_string()
        },
        battery_color,
        "▣",
    );
    render_metric_card(
        f,
        bottom_row[1],
        "Status",
        &status_label,
        &status_detail(app),
        COLOR_SECONDARY,
        "⟡",
    );

    if app.remote_screen.last_signal_update.is_some() {
        render_signal_gauge(
            f,
            vertical[3],
            "Signal Raw (0..255)",
            signal,
            COLOR_PRIMARY,
            COLOR_SURFACE_1,
        );
    }
}

fn render_signal_gauge(f: &mut Frame, area: Rect, title: &str, value: u8, fg: Color, bg: Color) {
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "))
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_2),
        )
        .gauge_style(Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD))
        .label(format!("{value} raw"))
        .ratio(f64::from(value) / 255.0);
    f.render_widget(gauge, area);
}

fn render_activity(f: &mut Frame, app: &App, area: Rect) {
    let lines = if app.remote_screen.elements.is_empty() && app.remote_screen.has_display_updates {
        vec![
            Line::from("Activity history is empty."),
            Line::from("Received display updates are preserved above."),
        ]
    } else if app.remote_screen.elements.is_empty() {
        vec![
            Line::from(Span::styled(
                "No radio screen updates yet.",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Start a session and this view will show signal, battery, and visible screen changes.",
                Style::default().fg(COLOR_DIM),
            )),
            Line::from(Span::styled(
                "Passive updates do not confirm control.",
                Style::default().fg(COLOR_DIM),
            )),
        ]
    } else {
        app.remote_screen
            .elements
            .iter()
            .rev()
            .take(8)
            .map(render_packet_line)
            .collect::<Vec<_>>()
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Live Activity ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        )
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1));
    f.render_widget(paragraph, area);
}

fn render_controls(f: &mut Frame, app: &App, area: Rect) {
    let chunks = if area.height >= 9 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(4)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(0)])
            .split(area)
    };

    render_last_control_panel(f, app, chunks[0]);
    if chunks.len() < 2 || chunks[1].height == 0 {
        return;
    }

    let lines = if app.remote_active {
        vec![
            Line::from(vec![
                render_shortcut("0-9"),
                Span::raw(": Digits | "),
                render_shortcut("m"),
                Span::raw(": Menu | "),
                render_shortcut("e / Esc"),
                Span::raw(": Exit"),
            ]),
            Line::from(vec![
                render_shortcut("u / d"),
                Span::raw(": Nav | "),
                render_shortcut("* / #"),
                Span::raw(": Keys | "),
                render_shortcut("a"),
                Span::raw(": PTT"),
            ]),
            Line::from(vec![
                render_shortcut("f"),
                Span::raw(": F | "),
                render_shortcut("v/b"),
                Span::raw(": B | "),
                render_shortcut("Tab"),
                Span::raw(": Leave remote"),
            ]),
        ]
    } else {
        vec![
            Line::from(vec![
                render_shortcut("o"),
                Span::raw(": Start session | "),
                render_shortcut("Esc"),
                Span::raw(": Back"),
            ]),
            Line::from(vec![
                render_shortcut("Tab"),
                Span::raw(": Next tab | "),
                render_shortcut("Shift-Tab"),
                Span::raw(": Previous tab"),
            ]),
            Line::from(vec![render_shortcut("0-9"), Span::raw(": Switch tabs")]),
        ]
    };

    let controls = Paragraph::new(lines)
        .style(Style::default().fg(COLOR_TEXT))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Controls ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(controls, chunks[1]);
}

fn render_remote_overview(f: &mut Frame, app: &App, area: Rect) {
    let session_verdict = remote_session_verdict(app.remote_active, &app.remote_screen);
    let session_style = Style::default()
        .fg(COLOR_SURFACE_0)
        .bg(session_verdict.accent)
        .add_modifier(Modifier::BOLD);
    let signal = app.remote_screen.signal_strength;
    let noise = app.remote_screen.noise_level;
    let battery = battery_label(app);
    let status = status_label(app);
    let freshness = freshness_summary(app);

    let text = vec![
        Line::from(vec![
            Span::styled("D: mirror | ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(
                "Remote",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(session_verdict.badge, session_style),
            Span::raw("  "),
            Span::styled(
                session_verdict.headline,
                Style::default()
                    .fg(session_verdict.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Result ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                truncate_line(
                    &session_verdict.detail,
                    area.width.saturating_sub(7) as usize,
                ),
                Style::default().fg(COLOR_TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Signal ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(
                app.remote_screen
                    .last_signal_update
                    .map_or_else(|| "unknown  ".to_string(), |_| format!("{signal} raw  ")),
                Style::default().fg(COLOR_PRIMARY),
            ),
            Span::styled("Noise ", Style::default().fg(COLOR_WARNING)),
            Span::styled(
                app.remote_screen
                    .last_noise_update
                    .map_or_else(|| "unknown  ".to_string(), |_| format!("{noise} raw  ")),
                Style::default().fg(COLOR_SECONDARY),
            ),
            Span::styled("Battery ", Style::default().fg(COLOR_ACCENT)),
            Span::styled(
                format!("{battery}  "),
                Style::default().fg(battery_visuals(app).1),
            ),
            Span::styled("State ", Style::default().fg(COLOR_SECONDARY)),
            Span::styled(status, Style::default().fg(COLOR_TEXT)),
            Span::raw("  "),
            Span::styled("Freshness ", Style::default().fg(COLOR_DIM)),
            Span::styled(freshness.0, Style::default().fg(freshness.2)),
        ]),
    ];

    let summary = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(summary, area);
}

fn remote_session_verdict(remote_active: bool, screen: &RemoteScreen) -> RemoteSessionVerdict {
    let telemetry_seen = telemetry_seen(screen);
    if !remote_active && let Some(failure) = &screen.last_failure {
        return RemoteSessionVerdict {
            badge: " FAIL ",
            headline: remote_failure_headline(failure.kind),
            detail: remote_failure_detail(failure.kind).to_string(),
            accent: COLOR_ERROR,
        };
    }

    if !remote_active {
        return RemoteSessionVerdict {
            badge: " IDLE ",
            headline: "session idle",
            detail: "Press o to start; passive updates do not confirm control.".to_string(),
            accent: COLOR_SURFACE_3,
        };
    }

    if let Some(report) = &screen.last_control_report {
        if !report.success || report.evidence == RemoteEvidenceKind::CommandFailed {
            return RemoteSessionVerdict {
                badge: " ERROR ",
                headline: "control failed",
                detail: truncate_line(
                    &format!("{} failed: {}", report.label, control_user_detail(report)),
                    60,
                ),
                accent: COLOR_ERROR,
            };
        }

        if report.evidence == RemoteEvidenceKind::ControlConfirmed {
            return RemoteSessionVerdict {
                badge: " CONTROL ",
                headline: "control confirmed",
                detail: "Command confirmation recorded.".to_string(),
                accent: COLOR_SUCCESS,
            };
        }

        return RemoteSessionVerdict {
            badge: " CHECK ",
            headline: "no control proof",
            detail: if let Some(reaction) = &report.reaction {
                format!(
                    "{} update(s), {} change(s); command not confirmed.",
                    reaction.surfaced_packets, reaction.deltas
                )
            } else {
                "Command sent; control not confirmed.".to_string()
            },
            accent: COLOR_WARNING,
        };
    }

    if telemetry_seen {
        return RemoteSessionVerdict {
            badge: " RADIO ",
            headline: "radio updates seen",
            detail: "Passive radio updates do not confirm control.".to_string(),
            accent: COLOR_WARNING,
        };
    }

    match screen.phase {
        crate::remote::RemoteSessionPhase::Opening => RemoteSessionVerdict {
            badge: " OPEN ",
            headline: "opening session",
            detail: "waiting for the radio to accept remote control".to_string(),
            accent: COLOR_PRIMARY,
        },
        crate::remote::RemoteSessionPhase::Recovering => RemoteSessionVerdict {
            badge: " RETRY ",
            headline: "reconnecting",
            detail: "restoring the remote session".to_string(),
            accent: COLOR_WARNING,
        },
        crate::remote::RemoteSessionPhase::Armed
        | crate::remote::RemoteSessionPhase::Probing
        | crate::remote::RemoteSessionPhase::Stopped
        | crate::remote::RemoteSessionPhase::Live => RemoteSessionVerdict {
            badge: " ARMED ",
            headline: "ready for command",
            detail: phase_detail_from_phase(remote_active, screen.phase),
            accent: COLOR_PRIMARY,
        },
    }
}

fn telemetry_seen(screen: &RemoteScreen) -> bool {
    !screen.elements.is_empty()
        || screen.unknown_packet_count > 0
        || [
            screen.last_signal_update,
            screen.last_noise_update,
            screen.last_battery_update,
            screen.last_text_update,
            screen.last_status_update,
            screen.last_led_update,
        ]
        .into_iter()
        .flatten()
        .next()
        .is_some()
}

fn phase_detail_from_phase(
    remote_active: bool,
    phase: crate::remote::RemoteSessionPhase,
) -> String {
    match phase {
        crate::remote::RemoteSessionPhase::Opening => "opening session".to_string(),
        crate::remote::RemoteSessionPhase::Armed => {
            "Session open; control not confirmed.".to_string()
        }
        crate::remote::RemoteSessionPhase::Probing => "sending command".to_string(),
        crate::remote::RemoteSessionPhase::Live => {
            "radio status is updating; command response still pending".to_string()
        }
        crate::remote::RemoteSessionPhase::Recovering => "reconnecting to the radio".to_string(),
        crate::remote::RemoteSessionPhase::Stopped => {
            if remote_active {
                "waiting for the first radio update".to_string()
            } else {
                "press o to open the session".to_string()
            }
        }
    }
}

fn session_summary(remote_active: bool, phase: crate::remote::RemoteSessionPhase) -> String {
    if !remote_active {
        "session stopped".to_string()
    } else if matches!(phase, crate::remote::RemoteSessionPhase::Live) {
        "radio updates received".to_string()
    } else {
        format!("session {phase}")
    }
}

fn remote_failure_headline(kind: RemoteSessionFailureKind) -> &'static str {
    match kind {
        RemoteSessionFailureKind::OpenFailed => "connection failed",
        RemoteSessionFailureKind::BootstrapHandshakeFailed => "setup failed",
        RemoteSessionFailureKind::RemoteOnAckFailed => "radio did not respond",
        RemoteSessionFailureKind::StreamLost => "connection lost",
    }
}

fn remote_failure_detail(kind: RemoteSessionFailureKind) -> &'static str {
    match kind {
        RemoteSessionFailureKind::OpenFailed => "NicTUI could not open the radio connection.",
        RemoteSessionFailureKind::BootstrapHandshakeFailed => {
            "The radio did not complete remote setup."
        }
        RemoteSessionFailureKind::RemoteOnAckFailed => {
            "The radio did not confirm remote control mode."
        }
        RemoteSessionFailureKind::StreamLost => "The radio connection stopped unexpectedly.",
    }
}

#[cfg(test)]
fn control_transport_detail(report: &crate::remote::RemoteControlReport) -> String {
    let detail = if report.reaction.is_some() {
        report
            .detail
            .split(" | reaction ")
            .next()
            .unwrap_or(report.detail.as_str())
    } else {
        report.detail.as_str()
    };
    truncate_line(detail.trim(), 48)
}

fn render_metric_card(
    f: &mut Frame,
    area: Rect,
    title: &str,
    value: &str,
    detail: &str,
    accent: Color,
    icon: &str,
) {
    let lines = if area.height <= 4 {
        vec![
            Line::from(vec![
                Span::styled(
                    icon,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(title, Style::default().fg(COLOR_DIM)),
            ]),
            Line::from(vec![
                Span::styled(
                    truncate_line(value, 14),
                    Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(truncate_line(detail, 18), Style::default().fg(accent)),
            ]),
        ]
    } else {
        vec![
            Line::from(vec![
                Span::styled(
                    icon,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(title, Style::default().fg(COLOR_DIM)),
            ]),
            Line::from(Span::styled(
                value,
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(detail, Style::default().fg(accent))),
        ]
    };

    let card = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_2),
    );
    f.render_widget(card, area);
}

fn render_packet_line(packet: &RemotePacket) -> Line<'static> {
    let (icon, accent) = match packet {
        RemotePacket::DisplayText { .. } => ("TXT", COLOR_PRIMARY),
        RemotePacket::DrawRectangle { .. } => ("BOX", COLOR_SECONDARY),
        RemotePacket::DrawSymbol { .. } => ("SYM", COLOR_ACCENT),
        RemotePacket::SignalStrength { .. } => ("SIG", COLOR_SUCCESS),
        RemotePacket::NoiseLevel { .. } => ("NOI", COLOR_WARNING),
        RemotePacket::SignalBarPos { .. } => ("BAR", COLOR_SECONDARY),
        RemotePacket::SmallStatus { .. } => ("STS", COLOR_SECONDARY),
        RemotePacket::UnknownFrame { .. } => ("NEW", COLOR_ERROR),
    };

    Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(packet_outcome(packet), Style::default().fg(COLOR_TEXT)),
    ])
}

fn packet_outcome(packet: &RemotePacket) -> String {
    match packet {
        RemotePacket::DisplayText { text, .. } => {
            format!("Screen text: {}", truncate_line(text, 36))
        }
        RemotePacket::DrawRectangle { .. } => "Screen area changed".to_string(),
        RemotePacket::DrawSymbol { .. } => "Screen icon changed".to_string(),
        RemotePacket::SignalStrength { strength, mode, .. } => {
            format!("Signal raw {strength} (mode {mode})")
        }
        RemotePacket::NoiseLevel { level, mode } => format!("Noise raw {level} (mode {mode})"),
        RemotePacket::SignalBarPos { .. } => "Signal meter moved".to_string(),
        RemotePacket::SmallStatus { .. } => "Radio status changed".to_string(),
        RemotePacket::UnknownFrame { .. } => {
            "Radio sent an update NicTUI cannot show yet".to_string()
        }
    }
}

fn battery_label(app: &App) -> String {
    app.remote_screen
        .battery_text
        .clone()
        .or_else(|| {
            app.remote_screen
                .battery_level
                .map(|level| format!("{level}%"))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn status_label(app: &App) -> String {
    app.remote_screen
        .last_small_status
        .map(|_| "updated".to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn status_detail(app: &App) -> String {
    match app.remote_screen.last_small_status {
        Some(_) => "status bytes received".to_string(),
        None => "waiting for update".to_string(),
    }
}

fn battery_visuals(app: &App) -> (u8, Color) {
    if let Some(text) = &app.remote_screen.battery_text
        && let Some(volts) = parse_voltage_text(text)
    {
        let normalized = (((volts - 5.8) / 1.0) * 100.0).clamp(0.0, 100.0) as u8;
        return (normalized, battery_color(normalized));
    }

    let fallback = app.remote_screen.battery_level.unwrap_or(0).min(100);
    (fallback, battery_color(fallback))
}

fn battery_meter(level: u8) -> String {
    let filled = match level {
        0..=15 => 1,
        16..=35 => 2,
        36..=60 => 3,
        61..=85 => 4,
        _ => 5,
    };
    let mut meter = String::new();
    for idx in 0..5 {
        meter.push(if idx < filled { '▰' } else { '▱' });
    }
    meter
}

fn battery_color(level: u8) -> Color {
    match level {
        0..=20 => COLOR_ERROR,
        21..=45 => COLOR_WARNING,
        _ => COLOR_SUCCESS,
    }
}

fn freshness_summary(app: &App) -> (String, String, Color) {
    let last = app.remote_screen.last_received;

    if !app.remote_active {
        return (
            "stopped".to_string(),
            last.map_or_else(
                || "no radio frames received".to_string(),
                |instant| format!("last receive {}", age_label(instant)),
            ),
            COLOR_DIM,
        );
    }

    match last {
        Some(instant) => {
            let elapsed = instant.elapsed();
            let freshness = if elapsed <= Duration::from_secs(2) {
                ("fresh", COLOR_SUCCESS)
            } else if elapsed <= Duration::from_secs(6) {
                ("warm", COLOR_WARNING)
            } else {
                ("stale", COLOR_ERROR)
            };
            (
                freshness.0.to_string(),
                format!("last receive {}", age_label(instant)),
                freshness.1,
            )
        }
        None => (
            "waiting".to_string(),
            "no radio frames received".to_string(),
            COLOR_DIM,
        ),
    }
}

fn age_label(instant: Instant) -> String {
    let elapsed = instant.elapsed();
    if elapsed < Duration::from_secs(1) {
        "just now".to_string()
    } else if elapsed < Duration::from_secs(60) {
        format!("{}s ago", elapsed.as_secs())
    } else {
        format!("{}m ago", elapsed.as_secs() / 60)
    }
}

fn parse_voltage_text(text: &str) -> Option<f32> {
    text.trim().strip_suffix('V')?.parse::<f32>().ok()
}

fn render_last_control_panel(f: &mut Frame, app: &App, area: Rect) {
    let lines = if let Some(report) = &app.remote_screen.last_control_report {
        let outcome = if !report.success {
            "failed".to_string()
        } else {
            control_outcome_label(report.evidence)
        };
        let color = if !report.success || report.evidence == RemoteEvidenceKind::CommandFailed {
            COLOR_ERROR
        } else if report.evidence == RemoteEvidenceKind::ControlConfirmed {
            COLOR_SUCCESS
        } else {
            COLOR_WARNING
        };
        vec![
            Line::from(vec![
                Span::styled(outcome, Style::default().fg(color).bold()),
                Span::raw(" | "),
                Span::styled(&report.label, Style::default().fg(COLOR_TEXT)),
            ]),
            Line::from(vec![
                Span::styled(
                    if report.success {
                        "Sent via "
                    } else {
                        "Tried via "
                    },
                    Style::default().fg(COLOR_DIM),
                ),
                Span::styled(
                    control_method_label(report.strategy),
                    Style::default().fg(COLOR_PRIMARY),
                ),
            ]),
            Line::from(Span::styled(
                report.reaction.as_ref().map_or_else(
                    || "No observation recorded".to_string(),
                    |reaction| {
                        format!(
                            "{} updates / {} changes",
                            reaction.surfaced_packets, reaction.deltas
                        )
                    },
                ),
                Style::default().fg(COLOR_DIM),
            )),
            Line::from(Span::styled(
                control_user_detail(report),
                Style::default().fg(COLOR_DIM),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "No control attempts yet.",
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "D: mirror / keypad",
                Style::default().fg(COLOR_DIM),
            )),
            Line::from(Span::styled(
                "Passive updates are not control proof.",
                Style::default().fg(COLOR_DIM),
            )),
        ]
    };

    let panel = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Last Control ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(panel, area);
}

fn control_method_label(strategy: crate::remote::RemoteControlStrategy) -> &'static str {
    match strategy {
        crate::remote::RemoteControlStrategy::RawKey => "direct key",
        crate::remote::RemoteControlStrategy::Sequence => "guided command",
    }
}

fn control_outcome_label(evidence: RemoteEvidenceKind) -> String {
    match evidence {
        RemoteEvidenceKind::ControlConfirmed => "confirmed".to_string(),
        RemoteEvidenceKind::NoControlEvidence => "not confirmed".to_string(),
        RemoteEvidenceKind::NoTelemetry => "no telemetry".to_string(),
        RemoteEvidenceKind::CommandFailed => "failed".to_string(),
    }
}

fn control_user_detail(report: &crate::remote::RemoteControlReport) -> String {
    if !report.success || report.evidence == RemoteEvidenceKind::CommandFailed {
        "Send failed; check the connection before retrying.".to_string()
    } else if report.evidence == RemoteEvidenceKind::ControlConfirmed {
        "Command confirmation recorded.".to_string()
    } else {
        "Passive updates do not confirm control.".to_string()
    }
}

fn truncate_line(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        text.to_string()
    } else if max_len <= 3 {
        text.chars().take(max_len).collect()
    } else {
        let mut shortened = text
            .chars()
            .take(max_len.saturating_sub(3))
            .collect::<String>();
        shortened.push_str("...");
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{
        RemoteCommandReaction, RemoteControlReport, RemoteControlStrategy, RemoteEvidenceKind,
        RemoteSessionFailure, RemoteSessionFailureKind,
    };
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    const TERMINAL_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (160, 48), (240, 64)];

    fn buffer_text(buffer: &Buffer, area: Rect) -> String {
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn receive_packet(app: &mut App, packet: RemotePacket) {
        app.event_tx
            .send(crate::app::AppEvent::RemotePacket(packet))
            .unwrap();
        assert!(app.update());
    }

    fn preview_snapshot(app: &App, width: u16, height: u16) -> (Buffer, Rect) {
        let sidebar = if width < 100 { 20 } else { 28 };
        let area = Rect::new(sidebar, 3, width - sidebar, height - 5);
        let preview = if app.remote_screen.show_diagnostics {
            remote_diagnostics_layout(area).preview
        } else {
            remote_screen_layout(area).display
        };
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render_remote_screen(frame, app, area))
            .unwrap();
        (terminal.backend().buffer().clone(), preview)
    }

    fn preview_text(buffer: &Buffer, preview: Rect) -> String {
        buffer_text(buffer, Block::bordered().inner(preview))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn assert_preview_canvas_matches(app: &App, buffer: &Buffer, preview: Rect) {
        let inner = Block::bordered().inner(preview);
        let canvas = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        let mut expected =
            Terminal::new(TestBackend::new(buffer.area.width, buffer.area.height)).unwrap();
        expected
            .draw(|frame| {
                frame.render_widget(Block::default().bg(COLOR_SURFACE_0), canvas);
                app.remote_screen.display.render(frame, canvas);
            })
            .unwrap();
        for y in canvas.y..canvas.bottom() {
            for x in canvas.x..canvas.right() {
                assert_eq!(buffer[(x, y)], expected.backend().buffer()[(x, y)]);
            }
        }
    }

    #[test]
    fn ack_only_waits_for_display_without_sending_commands() {
        use crate::app::AppEvent;
        use crate::remote::RemoteSessionPhase;
        use std::sync::{atomic::Ordering, mpsc};

        let mut app = App::new();
        let (tx, rx) = mpsc::channel();
        app.remote_tx = Some(tx);
        for phase in [
            RemoteSessionPhase::Opening,
            RemoteSessionPhase::Armed,
            RemoteSessionPhase::Live,
        ] {
            app.event_tx.send(AppEvent::RemotePhase(phase)).unwrap();
        }
        app.event_tx
            .send(AppEvent::RemoteReceived(Instant::now()))
            .unwrap();
        assert!(app.update());
        // Neither LED status nor meter placement draws the radio screen.
        receive_packet(
            &mut app,
            RemotePacket::SmallStatus {
                id: 0x70,
                value1: 0,
                value2: 0,
            },
        );
        receive_packet(&mut app, RemotePacket::SignalBarPos { y: 11, aux: 0 });
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            for (width, height) in [(80, 24), (160, 45)] {
                let (buffer, preview) = preview_snapshot(&app, width, height);
                let text = preview_text(&buffer, preview);
                assert!(text.contains("Waiting for radio display updates"), "{text}");
                assert!(text.contains("updates are sent"));
                assert!(!text.contains("incomplete"));
                assert!(text.contains("An idle radio may not redraw."), "{text}");
                assert!(!buffer_text(&buffer, preview).contains("updates only"));
            }
        }
        assert!(!app.remote_screen.has_display_updates);
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(!app.remote_stop_signal.load(Ordering::SeqCst));
    }

    #[test]
    fn meter_only_telemetry_keeps_waiting_without_discarding_meter_pixels() {
        let meters = [
            RemotePacket::SignalStrength {
                strength: 36,
                mode: 0,
                battery: 0,
            },
            RemotePacket::NoiseLevel { level: 14, mode: 0 },
        ];
        for packets in [
            vec![meters[0].clone()],
            vec![meters[1].clone()],
            meters.to_vec(),
        ] {
            let mut app = App::new();
            app.remote_active = true;
            let position = RemotePacket::SignalBarPos { y: 11, aux: 0 };
            let mut expected = crate::remote_display::RadioDisplay::default();
            expected.apply(&position);
            receive_packet(&mut app, position);
            for packet in packets {
                expected.apply(&packet);
                receive_packet(&mut app, packet);
            }
            assert!(!app.remote_screen.has_display_updates);
            for y in 0..128 {
                for x in 0..128 {
                    assert_eq!(app.remote_screen.display.pixel(x, y), expected.pixel(x, y));
                }
            }
            assert!(
                app.remote_screen.last_signal_update.is_some()
                    || app.remote_screen.last_noise_update.is_some()
            );
            for diagnostics in [false, true] {
                app.remote_screen.show_diagnostics = diagnostics;
                for (width, height) in [(80, 24), (160, 45)] {
                    let (buffer, preview) = preview_snapshot(&app, width, height);
                    let text = preview_text(&buffer, preview);
                    assert!(text.contains("Waiting for radio display updates"), "{text}");
                    assert!(text.contains("updates are sent"));
                    assert!(!buffer_text(&buffer, preview).contains("updates only"));
                }
            }
            receive_packet(
                &mut app,
                RemotePacket::DrawRectangle {
                    x: 0,
                    y: 100,
                    width: 1,
                    height: 1,
                    color: 0,
                },
            );
            assert!(app.remote_screen.has_display_updates);
            for diagnostics in [false, true] {
                app.remote_screen.show_diagnostics = diagnostics;
                for (width, height) in [(80, 24), (160, 45)] {
                    let (buffer, preview) = preview_snapshot(&app, width, height);
                    assert!(!preview_text(&buffer, preview).contains("Waiting for radio"));
                    assert_preview_canvas_matches(&app, &buffer, preview);
                }
            }
        }
    }

    #[test]
    fn inactive_empty_display_prompts_start_before_and_after_a_session() {
        let mut app = App::new();
        for stopped in [false, true] {
            if stopped {
                app.remote_active = true;
                app.event_tx
                    .send(crate::app::AppEvent::RemoteStopped {
                        message: "Remote stopped".to_string(),
                        failure: None,
                    })
                    .unwrap();
                app.update();
            }
            for diagnostics in [false, true] {
                app.remote_screen.show_diagnostics = diagnostics;
                for (width, height) in [(80, 24), (160, 45)] {
                    let (buffer, preview) = preview_snapshot(&app, width, height);
                    let text = preview_text(&buffer, preview);
                    assert!(
                        text.contains("Start Remote to receive display updates"),
                        "{text}"
                    );
                    assert!(text.contains("Remote is not running"));
                    assert!(!text.contains("Waiting for radio"));
                    assert!(!text.contains("incomplete"));
                    assert!(!text.contains("physically"));
                    assert!(!buffer_text(&buffer, preview).contains("cached"));
                }
            }
        }
    }

    #[test]
    fn stopped_display_is_marked_cached_and_keeps_received_voltage_and_pixels() {
        let mut app = App::new();
        app.remote_active = true;
        receive_packet(
            &mut app,
            RemotePacket::DisplayText {
                font_size: 0,
                x: 103,
                y: 70,
                fg_color: 0xffff,
                bg_color: 0,
                text: "7.4V".to_string(),
            },
        );
        app.event_tx
            .send(crate::app::AppEvent::RemoteStopped {
                message: "Remote stopped".to_string(),
                failure: None,
            })
            .unwrap();
        app.update();
        assert!(!app.remote_active);
        assert!(app.remote_screen.has_display_updates);
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            for (width, height) in [(80, 24), (160, 45)] {
                let (buffer, preview) = preview_snapshot(&app, width, height);
                let text = preview_text(&buffer, preview);
                assert!(buffer_text(&buffer, preview).contains("cached"));
                assert!(text.contains("incomplete"));
                assert!(text.contains("7.4V"));
                assert!(!text.contains("Waiting for radio"));
                assert!(!text.contains("Start Remote"));
                assert_preview_canvas_matches(&app, &buffer, preview);
            }
        }
    }

    #[test]
    fn battery_only_shows_updates_only_and_preserves_received_voltage() {
        let mut app = App::new();
        app.remote_active = true;
        receive_packet(
            &mut app,
            RemotePacket::DisplayText {
                font_size: 0,
                x: 103,
                y: 70,
                fg_color: 0xffff,
                bg_color: 0,
                text: "7.4V".to_string(),
            },
        );
        assert!(app.remote_screen.has_display_updates);
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            for (width, height) in [(80, 24), (160, 45)] {
                let (buffer, preview) = preview_snapshot(&app, width, height);
                let text = preview_text(&buffer, preview);
                assert!(buffer_text(&buffer, preview).contains("updates only"));
                assert!(text.contains("incomplete"));
                assert!(text.contains("7.4V"), "{text}");
                assert!(!text.contains("Waiting for radio"));
                assert_preview_canvas_matches(&app, &buffer, preview);
            }
        }
    }

    #[test]
    fn menu_draw_never_claims_completeness_after_confirmation_and_history_clear() {
        let mut app = App::new();
        app.remote_active = true;
        for packet in [
            RemotePacket::DrawRectangle {
                x: 0,
                y: 0,
                width: 128,
                height: 128,
                color: 0,
            },
            RemotePacket::DisplayText {
                font_size: 1,
                x: 5,
                y: 40,
                fg_color: 0xffff,
                bg_color: 0,
                text: "MENU".to_string(),
            },
        ] {
            receive_packet(&mut app, packet);
        }
        app.event_tx
            .send(crate::app::AppEvent::RemoteControl(RemoteControlReport {
                label: "Menu".to_string(),
                strategy: RemoteControlStrategy::RawKey,
                bytes_hex: "8A FF".to_string(),
                success: true,
                evidence: RemoteEvidenceKind::ControlConfirmed,
                reaction: None,
                detail: String::new(),
            }))
            .unwrap();
        app.update();
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            for (width, height) in [(80, 24), (160, 45)] {
                let (before, preview) = preview_snapshot(&app, width, height);
                let text = preview_text(&before, preview);
                assert!(buffer_text(&before, preview).contains("updates only"));
                assert!(text.contains("incomplete"));
                if !diagnostics || width >= 100 {
                    assert!(text.contains("MENU"), "{text}");
                }
                assert!(!text.contains("Waiting for radio"));
                assert_preview_canvas_matches(&app, &before, preview);
                app.remote_screen.elements.clear();
                let (after, _) = preview_snapshot(&app, width, height);
                for y in preview.y..preview.bottom() {
                    for x in preview.x..preview.right() {
                        assert_eq!(before[(x, y)], after[(x, y)]);
                    }
                }
            }
        }
    }

    #[test]
    fn opening_clears_previous_frame_and_returns_to_waiting() {
        let mut app = App::new();
        app.remote_active = true;
        receive_packet(
            &mut app,
            RemotePacket::DisplayText {
                font_size: 1,
                x: 0,
                y: 0,
                fg_color: 0xffff,
                bg_color: 0,
                text: "OLD MENU".to_string(),
            },
        );
        assert!(app.remote_screen.has_display_updates);
        app.event_tx
            .send(crate::app::AppEvent::RemotePhase(
                crate::remote::RemoteSessionPhase::Opening,
            ))
            .unwrap();
        app.update();
        assert!(!app.remote_screen.has_display_updates);
        assert!(!app.remote_screen.elements.iter().any(|packet| matches!(
            packet, RemotePacket::DisplayText { text, .. } if text == "OLD MENU"
        )));
        for y in 0..128 {
            for x in 0..128 {
                assert_eq!(app.remote_screen.display.pixel(x, y), Color::Rgb(0, 0, 0));
            }
        }
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            for (width, height) in [(80, 24), (160, 45)] {
                let (buffer, preview) = preview_snapshot(&app, width, height);
                let text = preview_text(&buffer, preview);
                assert!(text.contains("Waiting for radio display updates"), "{text}");
                assert!(text.contains("updates are sent"));
                assert!(!text.contains("OLD MENU"));
                assert!(!buffer_text(&buffer, preview).contains("updates only"));
            }
        }
    }

    #[test]
    fn demo_records_drawing_but_workspace_data_never_seeds_the_radio_frame() {
        let mut demo = App::demo();
        assert!(demo.remote_screen.has_display_updates);
        assert_eq!(demo.remote_screen.unknown_packet_count, 1);
        let mut workspace = App::new();
        workspace.channels = demo.channels.clone();
        workspace.settings = demo.settings.clone();
        workspace.remote_active = true;
        assert!(!workspace.remote_screen.has_display_updates);
        for diagnostics in [false, true] {
            demo.remote_screen.show_diagnostics = diagnostics;
            workspace.remote_screen.show_diagnostics = diagnostics;
            for (width, height) in [(80, 24), (160, 45)] {
                let (buffer, preview) = preview_snapshot(&demo, width, height);
                assert!(buffer_text(&buffer, preview).contains("cached"));
                assert!(!preview_text(&buffer, preview).contains("Waiting for radio"));
                assert_preview_canvas_matches(&demo, &buffer, preview);
                let (buffer, preview) = preview_snapshot(&workspace, width, height);
                assert!(
                    preview_text(&buffer, preview).contains("Waiting for radio display updates")
                );
            }
        }
    }

    #[test]
    fn mirror_layout_prioritizes_display_and_bounds_keypad() {
        for (width, height) in TERMINAL_SIZES {
            let area = Rect::new(7, 4, width, height);
            let layout = remote_screen_layout(area);
            let keypad_width = (width / 3).clamp(27, 37);
            assert_eq!(
                layout.display,
                Rect::new(7, 4, width - keypad_width, height - 1)
            );
            assert_eq!(
                layout.keypad,
                Rect::new(7 + width - keypad_width, 4, keypad_width, 18)
            );
            assert!(layout.display.width > layout.keypad.width);
            assert_eq!(layout.keypad_help.y, layout.keypad.bottom());
            assert_eq!(layout.keypad_help.bottom(), layout.display.bottom());
            assert_eq!(layout.overview.height, 1);
            assert_eq!(layout.overview.bottom(), area.bottom());
            for cell in keypad_cells(layout.keypad) {
                assert!(cell.width >= 5);
                assert!(cell.height >= 4);
                assert_eq!(cell.intersection(layout.keypad), cell);
            }
        }
    }

    #[test]
    fn actual_content_rects_reclaim_display_space_at_requested_sizes() {
        for (width, height, sidebar, expected_columns) in [
            (80, 24, 20, 28),
            (100, 30, 22, 40),
            (160, 48, 28, 76),
            (240, 64, 28, 108),
        ] {
            let area = Rect::new(sidebar, 3, width - sidebar, height - 5);
            let layout = remote_screen_layout(area);
            let inner = Block::bordered().inner(layout.display);
            let columns = inner.width.min(inner.height.saturating_sub(2) * 2);
            let old_columns = (area.width / 2 - 2).min((area.height - 10) * 2);
            assert_eq!(columns, expected_columns, "{width}x{height}");
            assert!(columns > old_columns);
            assert!(layout.keypad.width <= 37);
            assert!(layout.keypad.height <= 18);
            assert_eq!(layout.overview.height, 1);
        }
    }

    #[test]
    fn keypad_help_fits_complete_lines_and_prioritizes_ptt_warning() {
        let app = App::new();
        for width in [17, 18, 27, 37] {
            for height in 0..=8 {
                let area = Rect::new(3, 2, width, height);
                let inner = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT)
                    .inner(area);
                let mut terminal = Terminal::new(TestBackend::new(50, 12)).unwrap();
                terminal
                    .draw(|frame| render_keypad_help(frame, &app, area))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let text = buffer_text(buffer, inner);
                let lines = text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>();
                assert!(lines.len() <= usize::from(inner.height));
                for line in &lines {
                    assert!(
                        [
                            "PTT can transmit",
                            "Click or type a shortcut",
                            "Battery: unknown",
                            "Status: none",
                        ]
                        .contains(line),
                        "partial help at {width}x{height}: {text}"
                    );
                }
                if width >= 18 && height > 0 {
                    assert_eq!(lines[0], "PTT can transmit");
                } else {
                    assert!(lines.is_empty());
                }
                assert!(!text.contains("saved state"));
            }
        }
    }

    #[test]
    fn keypad_help_omits_overlong_values_instead_of_clipping_or_wrapping() {
        let mut app = App::new();
        app.remote_screen.battery_text = Some("an unusually long battery reading".to_string());
        let area = Rect::new(0, 0, 27, 6);
        let mut terminal = Terminal::new(TestBackend::new(27, 6)).unwrap();
        terminal
            .draw(|frame| render_keypad_help(frame, &app, area))
            .unwrap();
        let text = buffer_text(terminal.backend().buffer(), area);
        assert!(text.contains("PTT can transmit"));
        assert!(text.contains("Click or type a shortcut"));
        assert!(text.contains("Status: none"));
        assert!(!text.contains("Battery:"));
        assert!(!text.contains("unusually"));
    }

    #[test]
    fn compact_keys_have_uniform_borders_and_aligned_label_rows() {
        let app = App::new();
        for (width, height, sidebar) in [(80, 24, 20), (100, 30, 22), (160, 48, 28), (240, 64, 28)]
        {
            let area = Rect::new(sidebar, 3, width - sidebar, height - 5);
            let cells = keypad_cells(remote_screen_layout(area).keypad);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let bordered = cells.iter().all(|cell| cell.width >= 6 && cell.height >= 4);
            for row in 0..4 {
                let mut label_row = None;
                for index in row * 5..row * 5 + 5 {
                    let Some(key) = REMOTE_KEYS[index] else {
                        continue;
                    };
                    let cell = cells[index];
                    assert_eq!(buffer[(cell.x, cell.y)].symbol() == "┌", bordered);
                    let label_y = (cell.y..cell.bottom())
                        .find(|y| {
                            buffer_text(buffer, Rect::new(cell.x, *y, cell.width, 1))
                                .contains(key.label)
                        })
                        .unwrap();
                    assert_eq!(
                        *label_row.get_or_insert(label_y),
                        label_y,
                        "{} at {width}x{height}",
                        key.label
                    );
                    if key.shortcut != key.label {
                        assert!(
                            buffer_text(buffer, cell).contains(key.shortcut),
                            "{} at {width}x{height}",
                            key.shortcut
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn normal_status_uses_plain_waiting_partial_and_cached_states() {
        use crate::remote::RemoteSessionPhase;
        let mut app = App::new();
        for (active, updates, phase, label) in [
            (false, false, RemoteSessionPhase::Stopped, "INACTIVE"),
            (true, false, RemoteSessionPhase::Opening, "OPENING"),
            (true, false, RemoteSessionPhase::Armed, "WAITING"),
            (true, true, RemoteSessionPhase::Live, "PARTIAL"),
            (false, true, RemoteSessionPhase::Stopped, "CACHED"),
            (true, true, RemoteSessionPhase::Recovering, "RETRY"),
        ] {
            app.remote_active = active;
            app.remote_screen.has_display_updates = updates;
            app.remote_screen.phase = phase;
            for (width, height) in TERMINAL_SIZES {
                let area = Rect::new(0, 0, width, height);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| render_remote_screen(frame, &app, area))
                    .unwrap();
                let status = buffer_text(
                    terminal.backend().buffer(),
                    remote_screen_layout(area).overview,
                );
                assert!(status.contains(label), "{status}");
                for diagnostic in [
                    "proof",
                    "Evidence",
                    "CTRL",
                    "direct key",
                    "Last:",
                    "No keys",
                    "ready for command",
                ] {
                    assert!(!status.contains(diagnostic), "{status}");
                }
            }
        }
    }

    #[test]
    fn empty_states_are_centered_and_offer_start_only_when_inactive() {
        let mut app = App::new();
        for active in [false, true] {
            app.remote_active = active;
            for (width, height) in TERMINAL_SIZES {
                let (buffer, preview) = preview_snapshot(&app, width, height);
                let inner = Block::bordered().inner(preview);
                let canvas = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
                let content_rows = (canvas.y..canvas.bottom())
                    .filter(|y| {
                        !buffer_text(&buffer, Rect::new(canvas.x, *y, canvas.width, 1))
                            .trim()
                            .is_empty()
                    })
                    .collect::<Vec<_>>();
                assert!(
                    content_rows[0] > canvas.y,
                    "empty state not centered at {width}x{height}"
                );
                assert!(*content_rows.last().unwrap() < canvas.bottom() - 1);
                assert_eq!(
                    preview_text(&buffer, preview).contains("Start Remote"),
                    !active
                );
                assert!(!preview_text(&buffer, preview).contains("physically"));
            }
        }
    }

    #[test]
    fn keypad_dimming_follows_shared_control_readiness() {
        use crate::app::{AppMode, MainTab};
        use crate::remote::RemoteSessionPhase;
        let mut app = App::new();
        app.demo_mode = true;
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        let area = Rect::new(0, 0, 100, 30);
        let cell = keypad_cells(remote_screen_layout(area).keypad)[0];
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        for (phase, dialog) in [
            (RemoteSessionPhase::Opening, false),
            (RemoteSessionPhase::Armed, false),
            (RemoteSessionPhase::Armed, true),
            (RemoteSessionPhase::Recovering, false),
        ] {
            app.remote_screen.phase = phase;
            app.dialog_open = dialog;
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let label = (cell.y..cell.bottom())
                .flat_map(|y| (cell.x..cell.right()).map(move |x| (x, y)))
                .find(|pos| buffer[*pos].symbol() == "M")
                .unwrap();
            assert_eq!(
                buffer[label].modifier.contains(Modifier::DIM),
                !app.remote_controls_ready()
            );
        }
        assert!(app.remote_tx.is_none());
        assert!(app.port_candidates.is_empty());
    }

    #[test]
    fn keypad_hit_tests_match_original_rows_and_logical_ids() {
        let expected = [
            Some(0x0B),
            Some(0x02),
            Some(0x03),
            Some(0x04),
            Some(0x0F),
            Some(0x0C),
            Some(0x05),
            Some(0x06),
            Some(0x07),
            Some(0x01),
            Some(0x0D),
            Some(0x08),
            Some(0x09),
            Some(0x0A),
            Some(0x10),
            Some(0x0E),
            None,
            Some(0x12),
            Some(0x11),
            Some(0x13),
        ];
        for (width, height) in TERMINAL_SIZES {
            let area = Rect::new(7, 4, width, height);
            let layout = remote_screen_layout(area);
            let cells = keypad_cells(layout.keypad);
            for (cell, expected) in cells.into_iter().zip(expected) {
                for y in cell.y..cell.bottom() {
                    for x in cell.x..cell.right() {
                        assert_eq!(
                            remote_key_at(area, x, y),
                            expected,
                            "at {x},{y} in {area:?}"
                        );
                    }
                }
            }
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if !cells.iter().any(|cell| cell.contains((x, y).into())) {
                        assert_eq!(remote_key_at(area, x, y), None);
                    }
                }
            }
            for (x, y) in [(0, 0), (area.right(), area.y), (area.x, area.bottom())] {
                assert_eq!(remote_key_at(area, x, y), None);
            }
        }
    }

    #[test]
    fn mirror_renders_readable_keys_leds_and_compact_evidence() {
        let mut app = App::new();
        assert!(!app.remote_screen.show_diagnostics);
        app.remote_active = true;
        app.remote_screen.leds = 1 | (2 << 2);
        app.remote_screen.last_led_update = Some(Instant::now());
        for (width, height) in TERMINAL_SIZES {
            let area = Rect::new(0, 0, width, height);
            let layout = remote_screen_layout(area);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| render_remote_screen(f, &app, area))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text = buffer_text(buffer, area);
            assert!(text.contains("Radio Display"));
            assert!(text.contains("Left LED: green"));
            assert!(text.contains("Right LED: red"));
            assert!(text.contains("WAITING"));
            assert!(!text.contains("control attempts"));
            assert!(!text.contains("D diagnostics"));
            assert!(!text.contains("Tab leave"));
            assert!(!text.contains("p stop"));
            assert!(!text.contains("Connection Quality"));
            assert!(!text.contains("Live sketch"));
            for (cell, key) in keypad_cells(layout.keypad).into_iter().zip(REMOTE_KEYS) {
                let text = buffer_text(buffer, cell);
                if let Some(key) = key {
                    assert!(
                        text.contains(key.label),
                        "{} clipped in {cell:?}",
                        key.label
                    );
                    assert_eq!(
                        remote_key_at(area, cell.x + cell.width / 2, cell.y + cell.height / 2),
                        Some(key.code)
                    );
                } else {
                    assert!(text.chars().all(char::is_whitespace));
                }
            }
        }
    }

    #[test]
    fn diagnostics_remain_available_at_requested_sizes() {
        let mut app = App::new();
        app.remote_screen.show_diagnostics = true;
        for (width, height) in TERMINAL_SIZES {
            let area = Rect::new(0, 0, width, height);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| render_remote_screen(f, &app, area))
                .unwrap();
            let text = buffer_text(terminal.backend().buffer(), area);
            assert!(text.contains("Radio Display"));
            assert!(text.contains("Live Activity"));
            assert!(text.contains("Last Control"));
            assert!(!text.contains("Keypad"));
        }
    }

    #[test]
    fn keypad_labels_fit_inside_main_layout_at_small_terminal_sizes() {
        let mut app = App::new();
        app.remote_active = true;
        for (width, height, sidebar) in [(80, 24, 20), (100, 30, 22), (160, 45, 28)] {
            let area = Rect::new(sidebar, 3, width - sidebar, height - 5);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| render_remote_screen(f, &app, area))
                .unwrap();
            assert!(buffer_text(terminal.backend().buffer(), area).contains("may save state"));
            for (cell, key) in keypad_cells(remote_screen_layout(area).keypad)
                .into_iter()
                .zip(REMOTE_KEYS)
            {
                if let Some(key) = key {
                    let text = buffer_text(terminal.backend().buffer(), cell);
                    assert!(
                        text.contains(key.label),
                        "{} clipped at {width}x{height}: {text:?}",
                        key.label
                    );
                }
            }
        }
    }

    #[test]
    fn compact_control_does_not_equate_transport_success_with_control_proof() {
        let mut app = App::new();
        app.remote_active = true;
        app.remote_screen.last_control_report = Some(RemoteControlReport {
            label: "Menu".to_string(),
            strategy: RemoteControlStrategy::RawKey,
            bytes_hex: "8A FF".to_string(),
            success: true,
            evidence: RemoteEvidenceKind::NoControlEvidence,
            reaction: Some(RemoteCommandReaction {
                window_ms: 250,
                rx_first_ms: Some(31),
                surfaced_packets: 2,
                unknown_packets: 0,
                deltas: 0,
            }),
            detail: "Transport succeeded without a visible response".to_string(),
        });
        for (width, height) in TERMINAL_SIZES {
            let area = Rect::new(0, 0, width, height);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| render_remote_screen(f, &app, area))
                .unwrap();
            let text = buffer_text(terminal.backend().buffer(), area);
            assert!(text.contains("Last: Menu (not confirmed)"));
            assert!(!text.contains("no control proof"));
            assert!(!text.contains("direct key"));
            assert!(!text.contains("control confirmed"));
        }
    }

    #[test]
    fn mirror_does_not_replay_recent_activity_as_a_display_canvas() {
        let mut app = App::new();
        app.remote_screen
            .elements
            .push_back(RemotePacket::DisplayText {
                font_size: 1,
                x: 0,
                y: 0,
                fg_color: 0xFFFF,
                bg_color: 0,
                text: "HISTORY GHOST".to_string(),
            });
        let area = Rect::new(0, 0, 80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| render_remote_screen(f, &app, area))
            .unwrap();
        assert!(
            !buffer_text(
                terminal.backend().buffer(),
                remote_screen_layout(area).display
            )
            .contains("HISTORY GHOST")
        );
    }

    #[test]
    fn persistent_frame_is_visible_after_activity_history_is_cleared() {
        let mut app = App::new();
        let area = Rect::new(0, 0, 80, 24);
        let display = remote_screen_layout(area).display;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| render_remote_screen(f, &app, area))
            .unwrap();
        let empty = terminal.backend().buffer().clone();
        let packet = RemotePacket::DrawRectangle {
            x: 0,
            y: 0,
            width: 128,
            height: 128,
            color: 0xF800,
        };
        receive_packet(&mut app, packet);
        terminal
            .draw(|f| render_remote_screen(f, &app, area))
            .unwrap();
        let painted = terminal.backend().buffer().clone();
        assert!(
            (display.y..display.bottom()).any(|y| {
                (display.x..display.right()).any(|x| empty[(x, y)] != painted[(x, y)])
            })
        );
        app.remote_screen.elements.clear();
        terminal
            .draw(|f| render_remote_screen(f, &app, area))
            .unwrap();
        for y in display.y..display.bottom() {
            for x in display.x..display.right() {
                assert_eq!(painted[(x, y)], terminal.backend().buffer()[(x, y)]);
            }
        }
    }

    #[test]
    fn rendering_handles_collapsed_and_tiny_content_rects() {
        let mut app = App::new();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            for (width, height) in [(0, 0), (1, 1), (20, 8), (60, 18)] {
                let area = Rect::new(3, 2, width, height);
                terminal
                    .draw(|f| render_remote_screen(f, &app, area))
                    .unwrap();
            }
        }
        assert_eq!(remote_key_at(Rect::new(3, 2, 0, 0), 3, 2), None);
    }

    #[test]
    fn led_states_match_original_colors() {
        assert_eq!(led_visual(0), ("off", COLOR_DIM));
        assert_eq!(led_visual(1), ("green", COLOR_SUCCESS));
        assert_eq!(led_visual(2), ("red", COLOR_ERROR));
        assert_eq!(led_visual(3), ("yellow", COLOR_WARNING));
    }

    #[test]
    fn packet_activity_does_not_invent_percentages_or_battery_readings() {
        let signal = packet_outcome(&RemotePacket::SignalStrength {
            strength: 230,
            mode: 1,
            battery: 0,
        });
        assert_eq!(signal, "Signal raw 230 (mode 1)");
        assert!(!signal.contains("battery"));
        assert!(!signal.contains('%'));
        assert_eq!(
            packet_outcome(&RemotePacket::NoiseLevel {
                level: 120,
                mode: 2
            }),
            "Noise raw 120 (mode 2)"
        );
    }

    #[test]
    fn verdict_honors_explicit_control_confirmation() {
        let mut screen = RemoteScreen {
            phase: crate::remote::RemoteSessionPhase::Live,
            ..RemoteScreen::default()
        };
        screen.last_control_report = Some(RemoteControlReport {
            label: "menu".to_string(),
            strategy: RemoteControlStrategy::RawKey,
            bytes_hex: "0B 00".to_string(),
            success: true,
            evidence: RemoteEvidenceKind::ControlConfirmed,
            reaction: Some(RemoteCommandReaction {
                window_ms: 250,
                rx_first_ms: Some(34),
                surfaced_packets: 1,
                unknown_packets: 0,
                deltas: 1,
            }),
            detail: "step1 +0ms 0B wait 60ms".to_string(),
        });

        let verdict = remote_session_verdict(true, &screen);

        assert_eq!(verdict.badge, " CONTROL ");
        assert_eq!(verdict.headline, "control confirmed");
        assert_eq!(verdict.detail, "Command confirmation recorded.");
    }

    #[test]
    fn verdict_marks_radio_reporting_without_control_proof() {
        let screen = RemoteScreen {
            phase: crate::remote::RemoteSessionPhase::Live,
            last_signal_update: Some(Instant::now()),
            ..RemoteScreen::default()
        };

        let verdict = remote_session_verdict(true, &screen);

        assert_eq!(verdict.badge, " RADIO ");
        assert_eq!(verdict.headline, "radio updates seen");
        assert!(verdict.detail.contains("do not confirm control"));
    }

    #[test]
    fn verdict_surfaces_last_session_failure_when_remote_is_stopped() {
        let screen = RemoteScreen {
            phase: crate::remote::RemoteSessionPhase::Stopped,
            last_failure: Some(RemoteSessionFailure {
                kind: RemoteSessionFailureKind::RemoteOnAckFailed,
                summary: "Remote mode did not acknowledge the expected 4A start echo.".to_string(),
                detail: "nicFW start did not echo 4A as the immediate ack".to_string(),
            }),
            ..RemoteScreen::default()
        };

        let verdict = remote_session_verdict(false, &screen);

        assert_eq!(verdict.badge, " FAIL ");
        assert_eq!(verdict.headline, "radio did not respond");
        assert_eq!(
            verdict.detail,
            "The radio did not confirm remote control mode."
        );
    }

    #[test]
    fn layout_keeps_common_short_terminal_panes_visible() {
        for area in [Rect::new(0, 0, 80, 24), Rect::new(0, 0, 100, 30)] {
            let layout = remote_diagnostics_layout(area);

            assert!(layout.overview.height >= 4);
            assert!(layout.preview.height >= 12);
            assert!(layout.status.height >= 12);
            assert!(layout.activity.height >= 6);
            assert!(layout.controls.height >= 6);
            assert!(layout.preview.width > 0);
            assert!(layout.status.width > 0);
            assert!(layout.activity.width > 0);
            assert!(layout.controls.width > 0);
        }
    }

    #[test]
    fn control_transport_detail_strips_reaction_suffix() {
        let report = RemoteControlReport {
            label: "menu".to_string(),
            strategy: RemoteControlStrategy::Sequence,
            bytes_hex: "0B 00".to_string(),
            success: true,
            evidence: RemoteEvidenceKind::NoTelemetry,
            reaction: Some(RemoteCommandReaction {
                window_ms: 250,
                rx_first_ms: None,
                surfaced_packets: 0,
                unknown_packets: 0,
                deltas: 0,
            }),
            detail: "step1 +0ms 0B wait 80ms | reaction 250ms: rx-first=none surfaced=0 unknown=0 delta=0".to_string(),
        };

        assert_eq!(control_transport_detail(&report), "step1 +0ms 0B wait 80ms");
    }

    #[test]
    fn passive_changes_never_present_as_control_confirmation() {
        let mut app = App::new();
        app.remote_active = true;
        app.remote_screen.last_signal_update = Some(Instant::now());
        app.remote_screen.last_control_report = Some(RemoteControlReport {
            label: "Menu".to_string(),
            strategy: RemoteControlStrategy::RawKey,
            bytes_hex: "8A FF".to_string(),
            success: true,
            evidence: RemoteEvidenceKind::NoControlEvidence,
            reaction: Some(RemoteCommandReaction {
                window_ms: 250,
                rx_first_ms: Some(10),
                surfaced_packets: 3,
                unknown_packets: 0,
                deltas: 2,
            }),
            detail: String::new(),
        });
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            for (width, height, sidebar) in [(80, 24, 20), (100, 30, 22), (160, 45, 28)] {
                let area = Rect::new(sidebar, 3, width - sidebar, height - 5);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| render_remote_screen(frame, &app, area))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let text = buffer_text(buffer, area);
                assert_eq!(
                    text.contains("no control proof"),
                    diagnostics,
                    "{width}x{height}: {text}"
                );
                assert!(text.contains("not confirmed"), "{width}x{height}: {text}");
                assert!(!text.contains("control confirmed"));
                assert!(!text.contains("OK "));
                assert!(!text.contains("Light"));
                if diagnostics {
                    let control = remote_diagnostics_layout(area).controls;
                    assert!(buffer_text(buffer, control).contains("not confirmed"));
                } else {
                    assert!(text.contains("Last: Menu (not confirmed)"));
                }
            }
        }
    }

    #[test]
    fn every_corner_of_the_128_square_mirror_is_visible() {
        let mut app = App::new();
        for (x, y, color) in [
            (0, 0, 0x001f),
            (120, 0, 0x07e0),
            (0, 120, 0xf800),
            (120, 120, 0xffff),
        ] {
            receive_packet(
                &mut app,
                RemotePacket::DrawRectangle {
                    x,
                    y,
                    width: 8,
                    height: 8,
                    color,
                },
            );
        }
        for (width, height, sidebar) in [(80, 24, 20), (100, 30, 22), (160, 45, 28)] {
            let area = Rect::new(sidebar, 3, width - sidebar, height - 5);
            let inner = Block::bordered().inner(remote_screen_layout(area).display);
            let canvas = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
            let columns = canvas.width.min(canvas.height * 2);
            let rows = columns.div_ceil(2);
            let left = canvas.x + (canvas.width - columns) / 2;
            let top = canvas.y + (canvas.height - rows) / 2;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            for (x, y, color) in [
                (left, top, Color::Rgb(248, 0, 0)),
                (left + columns - 1, top, Color::Rgb(0, 252, 0)),
                (left, top + rows - 1, Color::Rgb(0, 0, 248)),
                (
                    left + columns - 1,
                    top + rows - 1,
                    Color::Rgb(248, 252, 248),
                ),
            ] {
                let cell = &terminal.backend().buffer()[(x, y)];
                assert!(
                    cell.fg == color || cell.bg == color,
                    "missing corner {x},{y} at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn split_frequency_and_bounded_redraw_preserve_the_ui_at_boundary_sizes() {
        let frequency = [
            RemotePacket::DisplayText {
                font_size: 129,
                x: 23,
                y: 41,
                fg_color: 0xffff,
                bg_color: 0,
                text: " 162".to_string(),
            },
            RemotePacket::DisplayText {
                font_size: 2,
                x: 87,
                y: 41,
                fg_color: 0xffff,
                bg_color: 0,
                text: "55000".to_string(),
            },
            RemotePacket::DrawRectangle {
                x: 85,
                y: 54,
                width: 2,
                height: 2,
                color: 0xffff,
            },
        ];
        for (width, height, expected_columns) in [
            (80, 24, 38),
            (100, 30, 50),
            (180, 70, 130),
            (61, 26, 32),
            (62, 26, 33),
            (63, 26, 34),
            (88, 40, 57),
            (89, 40, 58),
            (91, 40, 59),
            (97, 42, 63),
            (98, 42, 64),
            (100, 43, 65),
        ] {
            let mut app = App::new();
            assert!(app.port_candidates.is_empty());
            assert!(app.remote_tx.is_none());
            let area = Rect::new(0, 0, width, height);
            let inner = Block::bordered().inner(remote_screen_layout(area).display);
            let canvas = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
            let columns = canvas.width.min(canvas.height * 2);
            assert_eq!(columns, expected_columns);
            let rows = columns.div_ceil(2);
            let top = canvas.y + (canvas.height - rows) / 2;
            let frequency_top = top + (41 * u32::from(columns) / 256) as u16;
            let frequency_bottom = top + (57 * u32::from(columns)).div_ceil(256) as u16;
            receive_packet(
                &mut app,
                RemotePacket::DrawRectangle {
                    x: 4,
                    y: 104,
                    width: 8,
                    height: 8,
                    color: 0x001f,
                },
            );
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            let neighbor_only = terminal.backend().buffer().clone();

            for packet in &frequency[..2] {
                app.remote_screen.display.apply(packet);
            }
            let before_decimal = app.remote_screen.display.clone();
            app.remote_screen.display.apply(&frequency[2]);
            for y in 0..128 {
                for x in 0..128 {
                    let expected = if (85..87).contains(&x) && (54..56).contains(&y) {
                        Color::Rgb(248, 252, 248)
                    } else {
                        before_decimal.pixel(x, y)
                    };
                    assert_eq!(app.remote_screen.display.pixel(x, y), expected);
                }
            }
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            let painted = terminal.backend().buffer().clone();
            assert_ne!(
                painted, neighbor_only,
                "missing frequency at {width}x{height}"
            );
            for y in 0..height {
                for x in 0..width {
                    // Allow a sampling fringe, but not changes to LEDs, keypad,
                    // borders, or the distant native-pixel neighbor.
                    if !canvas.contains((x, y).into())
                        || y < frequency_top.saturating_sub(1)
                        || y > frequency_bottom
                    {
                        assert_eq!(
                            painted[(x, y)],
                            neighbor_only[(x, y)],
                            "frequency escaped at {x},{y} in {width}x{height}"
                        );
                    }
                }
            }
            for packet in &frequency {
                app.remote_screen.display.apply(packet);
            }
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            assert_eq!(
                terminal.backend().buffer(),
                &painted,
                "replay changed the frame"
            );

            // Check native update bounds here; visual occlusion needs a renderer
            // baseline, not an assertion about a label it might never emit.
            let before_occlusion = app.remote_screen.display.clone();
            app.remote_screen
                .display
                .apply(&RemotePacket::DrawRectangle {
                    x: 56,
                    y: 44,
                    width: 2,
                    height: 3,
                    color: 0,
                });
            for y in 0..128 {
                for x in 0..128 {
                    let expected = if (56..58).contains(&x) && (44..47).contains(&y) {
                        Color::Rgb(0, 0, 0)
                    } else {
                        before_occlusion.pixel(x, y)
                    };
                    assert_eq!(app.remote_screen.display.pixel(x, y), expected);
                }
            }
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            let mut fresh = Terminal::new(TestBackend::new(width, height)).unwrap();
            fresh
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            assert_eq!(terminal.backend().buffer(), fresh.backend().buffer());

            app.remote_screen
                .display
                .apply(&RemotePacket::DrawRectangle {
                    x: 23,
                    y: 41,
                    width: 104,
                    height: 16,
                    color: 0,
                });
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            assert_eq!(
                terminal.backend().buffer(),
                &neighbor_only,
                "clearing the frequency left a ghost at {width}x{height}"
            );
        }
    }

    #[test]
    fn freshness_exposes_age_without_claiming_a_live_connection() {
        let mut app = App::new();
        app.remote_active = true;
        app.remote_screen.last_received = Some(Instant::now() - Duration::from_secs(10));
        assert_eq!(freshness_summary(&app).0, "stale");
        app.remote_active = false;
        app.remote_screen.last_received = Some(Instant::now());
        assert_eq!(freshness_summary(&app).0, "stopped");
    }

    #[test]
    fn deduplicated_idle_receives_refresh_session_not_measurements_or_control() {
        use crate::app::AppEvent;

        let mut app = App::new();
        app.remote_active = true;
        let old = Instant::now() - Duration::from_secs(10);
        app.remote_screen.last_received = Some(old);
        app.remote_screen.last_signal_update = Some(old);
        app.remote_screen.last_noise_update = Some(old);
        app.remote_screen.last_battery_update = Some(old);
        app.remote_screen.last_text_update = Some(old);
        app.remote_screen.last_status_update = Some(old);
        app.remote_screen.last_led_update = Some(old);
        app.remote_screen.signal_strength = 42;
        app.remote_screen.noise_level = 7;
        app.remote_screen.battery_text = Some("7.4V".to_string());
        app.remote_screen.last_small_status = Some((0x70, 0, 0));
        app.remote_screen
            .elements
            .push_back(RemotePacket::SmallStatus {
                id: 0x70,
                value1: 0,
                value2: 0,
            });
        app.remote_screen.last_control_report = Some(RemoteControlReport {
            label: "Menu".to_string(),
            strategy: RemoteControlStrategy::RawKey,
            bytes_hex: "8A FF".to_string(),
            success: true,
            evidence: RemoteEvidenceKind::NoControlEvidence,
            reaction: None,
            detail: String::new(),
        });
        assert_eq!(freshness_summary(&app).0, "stale");

        // Deduplicated valid frames deliver receive events without new payloads.
        for _ in 0..2 {
            let received = Instant::now();
            app.event_tx
                .send(AppEvent::RemoteReceived(received))
                .unwrap();
            app.update();
            assert_eq!(app.remote_screen.last_received, Some(received));
            assert_eq!(freshness_summary(&app).0, "fresh");
            assert_eq!(app.remote_screen.elements.len(), 1);
            assert_eq!(app.remote_screen.last_signal_update, Some(old));
            assert_eq!(app.remote_screen.last_noise_update, Some(old));
            assert_eq!(app.remote_screen.last_battery_update, Some(old));
            assert_eq!(app.remote_screen.last_text_update, Some(old));
            assert_eq!(app.remote_screen.last_status_update, Some(old));
            assert_eq!(app.remote_screen.last_led_update, Some(old));
            assert_eq!(app.remote_screen.signal_strength, 42);
            assert_eq!(app.remote_screen.noise_level, 7);
            assert_eq!(battery_label(&app), "7.4V");
            assert_eq!(
                remote_session_verdict(true, &app.remote_screen).headline,
                "no control proof"
            );
        }
    }

    #[test]
    fn measurement_timestamps_do_not_replace_receive_freshness() {
        let mut app = App::new();
        app.remote_active = true;
        let now = Instant::now();
        app.remote_screen.last_signal_update = Some(now);
        app.remote_screen.last_battery_update = Some(now);
        assert_eq!(freshness_summary(&app).0, "waiting");
        app.remote_screen.last_received = Some(now - Duration::from_secs(10));
        assert_eq!(freshness_summary(&app).0, "stale");
    }

    #[test]
    fn truncate_line_never_exceeds_even_tiny_widths() {
        for width in 0..8 {
            assert!(truncate_line("passive updates", width).chars().count() <= width);
        }
    }

    #[test]
    fn absent_readings_are_unknown_not_zero_or_led_off() {
        let mut app = App::new();
        app.remote_active = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for diagnostics in [false, true] {
            app.remote_screen.show_diagnostics = diagnostics;
            let area = Rect::new(20, 3, 60, 19);
            terminal
                .draw(|frame| render_remote_screen(frame, &app, area))
                .unwrap();
            let text = buffer_text(terminal.backend().buffer(), area);
            assert!(!text.contains("0 raw"));
            assert!(!text.contains("LED: off"));
            assert!(text.contains("LED: unknown") || text.contains("L: unknown"));
            if diagnostics {
                assert!(text.contains("Signal: unknown"));
                assert!(text.contains("Noise: unknown"));
            }
        }
    }
}
