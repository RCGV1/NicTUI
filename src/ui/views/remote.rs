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
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
};
use std::time::{Duration, Instant};

const LCD_COLS: usize = 30;
const LCD_ROWS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteSessionVerdict {
    badge: &'static str,
    headline: &'static str,
    detail: String,
    accent: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteScreenLayout {
    overview: Rect,
    preview: Rect,
    status: Rect,
    activity: Rect,
    controls: Rect,
}

pub fn render_remote_screen(f: &mut Frame, app: &App, area: Rect) {
    let layout = remote_screen_layout(area);

    render_remote_overview(f, app, layout.overview);
    render_lcd_preview(f, app, layout.preview);
    render_status_sidebar(f, app, layout.status);
    render_activity(f, app, layout.activity);
    render_controls(f, app, layout.controls);
}

fn remote_screen_layout(area: Rect) -> RemoteScreenLayout {
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
            Constraint::Min(12),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let body = if chunks[1].width < 76 {
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

    RemoteScreenLayout {
        overview: chunks[0],
        preview: body[0],
        status: body[1],
        activity: detail_split[0],
        controls: detail_split[1],
    }
}

fn render_lcd_preview(f: &mut Frame, app: &App, area: Rect) {
    let preview = build_lcd_preview(app);
    let paragraph = Paragraph::new(preview)
        .style(
            Style::default()
                .fg(Color::Rgb(198, 255, 214))
                .bg(Color::Rgb(10, 28, 20)),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Radio Display ")
                .border_style(if app.remote_active {
                    Style::default().fg(COLOR_SUCCESS)
                } else {
                    Style::default().fg(COLOR_BORDER)
                })
                .bg(Color::Rgb(8, 18, 14)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_status_sidebar(f: &mut Frame, app: &App, area: Rect) {
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

    let signal = app.remote_screen.signal_strength.min(100);
    let noise = app.remote_screen.noise_level.min(100);
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
        &format!("{}%", signal),
        &format!("{} {}", signal_bars(signal), signal_descriptor(signal)),
        signal_color(signal),
        "⌁",
    );
    render_metric_card(
        f,
        mid_row[1],
        "Noise Floor",
        &format!("{}%", noise),
        &format!("{} {}", signal_bars(noise), noise_descriptor(noise)),
        noise_color(noise),
        "≈",
    );
    render_metric_card(
        f,
        bottom_row[0],
        "Battery",
        &battery_label,
        &format!("{} live readings", battery_meter(battery_level)),
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

    render_signal_gauge(
        f,
        vertical[3],
        "Connection Quality",
        signal.saturating_sub(noise / 3),
        COLOR_PRIMARY,
        COLOR_SURFACE_1,
    );
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
        .label(format!("{}  {value}%", signal_bars(value)))
        .percent(value as u16);
    f.render_widget(gauge, area);
}

fn render_activity(f: &mut Frame, app: &App, area: Rect) {
    let lines = if app.remote_screen.elements.is_empty() {
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
                "A command is confirmed only when the radio visibly responds.",
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
                render_shortcut("a / b"),
                Span::raw(": PTT"),
            ]),
            Line::from(vec![
                render_shortcut("f"),
                Span::raw(": Light | "),
                render_shortcut("v"),
                Span::raw(": V-M | "),
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
            Line::from(vec![render_shortcut("1-9"), Span::raw(": Switch tabs")]),
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
    let signal = app.remote_screen.signal_strength.min(100);
    let noise = app.remote_screen.noise_level.min(100);
    let battery = battery_label(app);
    let status = status_label(app);
    let freshness = freshness_summary(app);

    let text = vec![
        Line::from(vec![
            Span::styled(
                "Remote Control",
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
            Span::raw("  "),
            Span::styled(
                session_summary(app.remote_active, app.remote_screen.phase),
                Style::default().fg(COLOR_DIM),
            ),
        ]),
        Line::from(vec![
            Span::styled("Result ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                truncate_line(&session_verdict.detail, 76),
                Style::default().fg(COLOR_TEXT),
            ),
            Span::raw("  "),
            Span::styled(freshness.1, Style::default().fg(COLOR_DIM)),
            Span::raw("  "),
            Span::styled(
                format!("{} recent update(s)", app.remote_screen.elements.len()),
                Style::default().fg(COLOR_DIM),
            ),
        ]),
        Line::from(vec![
            Span::styled("Signal ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(
                format!("{} {}%  ", signal_bars(signal), signal),
                Style::default().fg(signal_color(signal)),
            ),
            Span::styled("Noise ", Style::default().fg(COLOR_WARNING)),
            Span::styled(
                format!("{} {}%  ", signal_bars(noise), noise),
                Style::default().fg(noise_color(noise)),
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
            detail: "press o to start a session; commands confirm only after the radio responds"
                .to_string(),
            accent: COLOR_SURFACE_3,
        };
    }

    if let Some(report) = &screen.last_control_report {
        if !report.success {
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

        if let Some(reaction) = &report.reaction {
            if matches!(report.evidence, RemoteEvidenceKind::ControlConfirmed) {
                let first_rx = reaction
                    .rx_first_ms
                    .map(|millis| format!("{millis}ms"))
                    .unwrap_or_else(|| "the radio responded".to_string());
                return RemoteSessionVerdict {
                    badge: " CONTROL ",
                    headline: "control confirmed",
                    detail: format!(
                        "radio changed {} time{} after {}",
                        reaction.deltas,
                        if reaction.deltas == 1 { "" } else { "s" },
                        first_rx
                    ),
                    accent: COLOR_SUCCESS,
                };
            }

            if matches!(report.evidence, RemoteEvidenceKind::NoControlEvidence) {
                return RemoteSessionVerdict {
                    badge: " CHECK ",
                    headline: "no control proof",
                    detail: format!(
                        "{} radio update(s), but no visible response to the command yet",
                        reaction.surfaced_packets
                    ),
                    accent: COLOR_WARNING,
                };
            }

            if telemetry_seen {
                return RemoteSessionVerdict {
                    badge: " RADIO ",
                    headline: "radio is reporting",
                    detail: format!(
                        "the radio is connected, but the last command showed no change for {}ms",
                        reaction.window_ms
                    ),
                    accent: COLOR_WARNING,
                };
            }

            return RemoteSessionVerdict {
                badge: " QUIET ",
                headline: "session open",
                detail: format!(
                    "last command showed no visible response in {}ms",
                    reaction.window_ms
                ),
                accent: COLOR_DIM,
            };
        }
    }

    if telemetry_seen {
        return RemoteSessionVerdict {
            badge: " RADIO ",
            headline: "radio is reporting",
            detail: "signal and status are updating; send a command to confirm control".to_string(),
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
            "radio is ready; send a command to confirm control".to_string()
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
        "radio connected, awaiting command response".to_string()
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

fn build_lcd_preview(app: &App) -> Vec<Line<'static>> {
    let mut canvas = vec![vec![' '; LCD_COLS]; LCD_ROWS];

    for packet in app.remote_screen.elements.iter() {
        match packet {
            RemotePacket::DisplayText { x, y, text, .. } => {
                let row = ((*y as usize) * LCD_ROWS / 128).min(LCD_ROWS.saturating_sub(1));
                let col = ((*x as usize) * LCD_COLS / 160).min(LCD_COLS.saturating_sub(1));
                for (offset, ch) in text.chars().enumerate() {
                    let col = col + offset;
                    if col >= LCD_COLS {
                        break;
                    }
                    canvas[row][col] = sanitize_preview_char(ch);
                }
            }
            RemotePacket::DrawSymbol { x, y, .. } => {
                let row = ((*y as usize) * LCD_ROWS / 128).min(LCD_ROWS.saturating_sub(1));
                let col = ((*x as usize) * LCD_COLS / 160).min(LCD_COLS.saturating_sub(1));
                canvas[row][col] = '#';
            }
            _ => {}
        }
    }

    let mut lines = vec![Line::from(Span::styled(
        "Live sketch of the radio screen",
        Style::default().fg(Color::Rgb(129, 193, 146)),
    ))];

    for row in canvas {
        let text: String = row.into_iter().collect();
        lines.push(Line::from(vec![
            Span::styled("│", Style::default().fg(Color::Rgb(80, 140, 94))),
            Span::raw(text),
            Span::styled("│", Style::default().fg(Color::Rgb(80, 140, 94))),
        ]));
    }

    lines.push(Line::from(Span::styled(
        "Some icons may appear as simple markers.",
        Style::default().fg(Color::Rgb(129, 193, 146)),
    )));

    lines
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
        RemotePacket::SignalStrength {
            strength, battery, ..
        } => {
            format!("Signal {strength}% with battery {battery}%")
        }
        RemotePacket::NoiseLevel { level, .. } => format!("Noise level {level}%"),
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
        Some((0x70, 0x00, 0x00)) => "radio idle".to_string(),
        Some(_) => "radio state changed".to_string(),
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

fn signal_bars(value: u8) -> &'static str {
    match value {
        0..=10 => "▁▁▁▁",
        11..=30 => "▂▁▁▁",
        31..=50 => "▂▄▁▁",
        51..=70 => "▂▄▆▁",
        _ => "▂▄▆█",
    }
}

fn signal_descriptor(value: u8) -> &'static str {
    match value {
        0..=20 => "quiet",
        21..=45 => "present",
        46..=70 => "strong",
        _ => "peaking",
    }
}

fn noise_descriptor(value: u8) -> &'static str {
    match value {
        0..=15 => "clean floor",
        16..=35 => "light hash",
        36..=60 => "busy band",
        _ => "heavy noise",
    }
}

fn signal_color(value: u8) -> Color {
    match value {
        0..=20 => COLOR_DIM,
        21..=50 => COLOR_PRIMARY,
        _ => COLOR_SUCCESS,
    }
}

fn noise_color(value: u8) -> Color {
    match value {
        0..=15 => COLOR_SUCCESS,
        16..=40 => COLOR_WARNING,
        _ => COLOR_ERROR,
    }
}

fn freshness_summary(app: &App) -> (String, String, Color) {
    let last = [
        app.remote_screen.last_signal_update,
        app.remote_screen.last_noise_update,
        app.remote_screen.last_battery_update,
        app.remote_screen.last_text_update,
        app.remote_screen.last_status_update,
    ]
    .into_iter()
    .flatten()
    .max();

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
                format!("last update {}", age_label(instant)),
                freshness.1,
            )
        }
        None => (
            "waiting".to_string(),
            "no radio updates yet".to_string(),
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
        let (reaction_summary, reaction_metrics) = if let Some(reaction) = &report.reaction {
            let first = reaction
                .rx_first_ms
                .map(|millis| format!("{millis}ms"))
                .unwrap_or_else(|| "none".to_string());
            let (outcome, metrics) = match report.evidence {
                RemoteEvidenceKind::ControlConfirmed => (
                    format!("radio changed in {first}"),
                    format!(
                        "{} update(s) seen | {} visible change(s)",
                        reaction.surfaced_packets, reaction.deltas
                    ),
                ),
                RemoteEvidenceKind::NoControlEvidence => (
                    format!("radio updated in {first}, command not confirmed"),
                    format!(
                        "{} update(s) seen | {} visible change(s)",
                        reaction.surfaced_packets, reaction.deltas
                    ),
                ),
                RemoteEvidenceKind::NoTelemetry => (
                    format!("no reaction in {}ms", reaction.window_ms),
                    "0 update(s) seen | 0 visible change(s)".to_string(),
                ),
                RemoteEvidenceKind::CommandFailed => (
                    "command failed before the radio could respond".to_string(),
                    format!(
                        "{} update(s) seen | {} visible change(s)",
                        reaction.surfaced_packets, reaction.deltas
                    ),
                ),
            };
            (outcome, metrics)
        } else {
            (
                control_outcome_label(report.evidence),
                "no radio response recorded".to_string(),
            )
        };
        vec![
            Line::from(vec![
                Span::styled(
                    if report.success { "OK " } else { "ERR" },
                    Style::default()
                        .fg(if report.success {
                            COLOR_SUCCESS
                        } else {
                            COLOR_ERROR
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(&report.label, Style::default().fg(COLOR_TEXT)),
            ]),
            Line::from(vec![
                Span::styled("Method ", Style::default().fg(COLOR_DIM)),
                Span::styled(
                    control_method_label(report.strategy),
                    Style::default().fg(COLOR_PRIMARY),
                ),
            ]),
            Line::from(vec![
                Span::styled("Outcome ", Style::default().fg(COLOR_DIM)),
                Span::styled(reaction_summary, Style::default().fg(COLOR_TEXT)),
                Span::raw("  "),
                Span::styled("Result ", Style::default().fg(COLOR_DIM)),
                Span::styled(
                    control_outcome_label(report.evidence),
                    Style::default().fg(COLOR_WARNING),
                ),
            ]),
            Line::from(Span::styled(
                reaction_metrics,
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
                "Use keypad shortcuts once the radio is ready.",
                Style::default().fg(COLOR_DIM),
            )),
            Line::from(Span::styled(
                "Your last command and radio response will appear here.",
                Style::default().fg(COLOR_DIM),
            )),
        ]
    };

    let panel = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
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
        RemoteEvidenceKind::NoTelemetry => "no response".to_string(),
        RemoteEvidenceKind::CommandFailed => "failed".to_string(),
    }
}

fn control_user_detail(report: &crate::remote::RemoteControlReport) -> String {
    if report.success {
        "Waiting for the radio display or status to change.".to_string()
    } else {
        "The command did not reach the radio. Check the connection and try again.".to_string()
    }
}

fn truncate_line(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        text.to_string()
    } else {
        let mut shortened = text
            .chars()
            .take(max_len.saturating_sub(3))
            .collect::<String>();
        shortened.push_str("...");
        shortened
    }
}

fn sanitize_preview_char(ch: char) -> char {
    if ch.is_ascii_graphic() || ch == ' ' {
        ch
    } else {
        '?'
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{
        RemoteCommandReaction, RemoteControlReport, RemoteControlStrategy, RemoteEvidenceKind,
        RemoteSessionFailure, RemoteSessionFailureKind,
    };

    #[test]
    fn verdict_marks_control_confirmed_when_delta_seen() {
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
        assert!(verdict.detail.contains("radio changed 1 time"));
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
        assert_eq!(verdict.headline, "radio is reporting");
        assert!(verdict.detail.contains("send a command"));
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
            let layout = remote_screen_layout(area);

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
}
