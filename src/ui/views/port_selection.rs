use crate::app::App;
use crate::device::{PortCandidate, PortKind};
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG,
    COLOR_TEXT,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

pub fn render_port_selection(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    let welcome = Paragraph::new(vec![
        Line::from(Span::styled(
            "NicTUI",
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("USB: ", Style::default().fg(COLOR_TEXT)),
            Span::styled(
                "best for first setup and dependable reads/writes.",
                Style::default().fg(COLOR_DIM),
            ),
        ]),
        Line::from(vec![
            Span::styled("BLE: ", Style::default().fg(COLOR_TEXT)),
            Span::styled(
                "NicTUI scans automatically; keep the radio nearby with Bluetooth on.",
                Style::default().fg(COLOR_DIM),
            ),
        ]),
        Line::from(vec![
            Span::styled("Need help: ", Style::default().fg(COLOR_TEXT)),
            Span::styled(
                "press b to scan again, or use USB if wireless setup is not ready.",
                Style::default().fg(COLOR_DIM),
            ),
        ]),
    ])
    .alignment(ratatui::layout::Alignment::Center)
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" First Run ")
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(welcome, chunks[0]);

    let selected_candidate = app.selected_port_candidate();
    let selected_port = selected_candidate
        .map(|candidate| App::display_port_name(&candidate.port_name))
        .unwrap_or_else(|| "None selected".to_string());
    let selected_kind = selected_candidate
        .map(|candidate| candidate.badge())
        .unwrap_or("none");
    let selected_detail = selected_candidate
        .and_then(port_detail)
        .unwrap_or_else(|| "Choose a target from the list".to_string());
    let selected_guidance = selected_candidate
        .map(connection_guidance)
        .unwrap_or("Connect USB, or wait for NicTUI to find nearby radios automatically.");
    let serial_count = app
        .port_candidates
        .iter()
        .filter(|candidate| !candidate.is_ble())
        .count();
    let ble_count = app.ble_target_count();
    let (ble_readiness_summary, ble_readiness_hint) = port_picker_ble_overview(app);
    let scan_status = port_picker_scan_status(app, serial_count, ble_count);
    let status = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Found: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                format!("{}", app.port_candidates.len()),
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" targets", Style::default().fg(COLOR_DIM)),
            Span::raw("  "),
            Span::styled(
                format!("{serial_count} USB"),
                Style::default().fg(COLOR_DIM),
            ),
            Span::raw("  "),
            Span::styled(format!("{ble_count} BLE"), Style::default().fg(COLOR_DIM)),
        ]),
        Line::from(vec![
            Span::styled("Selected: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                selected_port,
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                selected_kind,
                Style::default().fg(COLOR_DIM).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(selected_detail, Style::default().fg(COLOR_DIM)),
        ]),
        Line::from(vec![
            Span::styled("Flow: ", Style::default().fg(COLOR_DIM)),
            Span::styled(selected_guidance, Style::default().fg(COLOR_DIM)),
        ]),
        Line::from(vec![
            Span::styled("Scan: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                if app.ble_scan_in_progress {
                    "running"
                } else {
                    "idle"
                },
                Style::default()
                    .fg(if app.ble_scan_in_progress {
                        COLOR_ACCENT
                    } else {
                        COLOR_DIM
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(scan_status, Style::default().fg(COLOR_DIM)),
        ]),
        Line::from(vec![
            Span::styled("BLE: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                ble_readiness_summary,
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Next: ", Style::default().fg(COLOR_DIM)),
            Span::styled(ble_readiness_hint, Style::default().fg(COLOR_DIM)),
        ]),
    ])
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Connection Readiness ")
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(status, chunks[1]);

    let ports: Vec<ListItem> = if app.port_candidates.is_empty() {
        vec![
            ListItem::new(" No targets yet. Connect USB and press r, or press b to scan BLE. ")
                .style(Style::default().fg(COLOR_DIM)),
        ]
    } else {
        app.port_candidates
            .iter()
            .enumerate()
            .map(|(i, candidate)| {
                let style = if i == app.selected_port_index {
                    Style::default()
                        .fg(COLOR_SELECTION_FG)
                        .bg(COLOR_SELECTION_BG)
                        .add_modifier(Modifier::BOLD)
                } else if matches!(
                    candidate.kind,
                    PortKind::Radio | PortKind::Ble | PortKind::Candidate
                ) {
                    Style::default().fg(COLOR_ACCENT)
                } else {
                    Style::default().fg(COLOR_DIM)
                };
                let mut line = vec![Span::styled(
                    App::display_port_name(&candidate.port_name),
                    style,
                )];
                line.push(Span::raw("  "));
                line.push(Span::styled(
                    candidate.badge(),
                    Style::default().fg(COLOR_DIM).add_modifier(Modifier::BOLD),
                ));
                if let Some(detail) = port_detail(candidate) {
                    line.push(Span::raw("  "));
                    line.push(Span::styled(detail, Style::default().fg(COLOR_DIM)));
                }
                ListItem::new(Line::from(line)).style(style)
            })
            .collect()
    };

    let list = List::new(ports)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Radio Targets ")
                .border_style(Style::default().fg(COLOR_BORDER)),
        )
        .highlight_symbol(" ▶ ");
    let mut list_state = ListState::default();
    if !app.port_candidates.is_empty() {
        list_state.select(Some(app.selected_port_index));
    }
    f.render_stateful_widget(list, chunks[2], &mut list_state);

    let mut help_spans = vec![
        render_shortcut("↑/↓"),
        Span::raw(" Choose | "),
        render_shortcut("Enter"),
        Span::raw(" Continue | "),
        render_shortcut("r"),
        Span::raw(" Refresh All | "),
        render_shortcut("b"),
        Span::raw(" Scan BLE | "),
    ];
    if area.width >= 104 {
        help_spans.push(Span::styled(
            "wireless help: keep Bluetooth on and radio nearby | ",
            Style::default().fg(COLOR_DIM),
        ));
    }
    help_spans.extend([render_shortcut("q"), Span::raw(" Quit ")]);

    let help = Paragraph::new(Line::from(help_spans))
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(COLOR_DIM))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(help, chunks[3]);
}

fn connection_guidance(candidate: &PortCandidate) -> &'static str {
    if candidate.is_ble() {
        "Press Enter to connect to this BLE radio."
    } else if candidate.is_radio() {
        "Press Enter to continue; USB is the most dependable setup path."
    } else {
        "Press Enter to try this target; if it is not the radio, refresh and choose another."
    }
}

fn port_picker_scan_status(app: &App, serial_count: usize, ble_count: usize) -> String {
    if app.ble_scan_in_progress {
        "Looking for nearby TD-H3 radios...".to_string()
    } else if ble_count > 0 {
        "Wireless radio found. Choose it and press Enter.".to_string()
    } else if serial_count > 0 {
        "USB radio found. Choose it and press Enter.".to_string()
    } else {
        "Connect USB, or keep the radio nearby with Bluetooth on.".to_string()
    }
}

fn port_picker_ble_overview(app: &App) -> (String, String) {
    if app.ble_scan_in_progress {
        return (
            "Checking nearby TD-H3 radios".to_string(),
            "Keep the radio nearby and awake while NicTUI scans.".to_string(),
        );
    }

    if let Some(candidate) = app
        .selected_port_candidate()
        .filter(|candidate| candidate.is_ble())
    {
        let hint = match candidate.ble_rssi {
            Some(rssi) => format!("Signal {rssi} dBm. Press Enter to continue."),
            None => "Press Enter to continue.".to_string(),
        };
        return ("Wireless radio selected".to_string(), hint);
    }

    let ble_count = app.ble_target_count();
    if ble_count > 0 {
        return (
            format!("{ble_count} wireless radio(s) visible"),
            "Use ↑/↓ to choose one, then press Enter.".to_string(),
        );
    }

    (
        "No wireless radios visible yet".to_string(),
        "Press b to scan again, or use USB for setup.".to_string(),
    )
}

fn port_detail(candidate: &PortCandidate) -> Option<String> {
    if let (Some(vid), Some(pid)) = (candidate.usb_vid, candidate.usb_pid) {
        return Some(format!("VID:PID {:04X}:{:04X}", vid, pid));
    }

    if candidate.is_ble() {
        return Some(match candidate.ble_rssi {
            Some(rssi) => format!("Wireless radio {rssi} dBm"),
            None => "Wireless radio".to_string(),
        });
    }

    candidate
        .product
        .as_deref()
        .or(candidate.manufacturer.as_deref())
        .map(str::to_string)
}
