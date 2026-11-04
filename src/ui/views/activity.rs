use crate::activity::format_frequency_mhz;
use crate::app::App;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG, COLOR_TEXT,
    COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

pub fn render_activity(f: &mut Frame, app: &mut App, area: Rect) {
    if area.is_empty() {
        return;
    }
    let active = app.activity_active();
    let state = app.activity.lock().unwrap().clone();
    let status = state.status.label();
    let title = format!(
        " Activity [A] | {status}{} ",
        if state.stopping { " / stopping" } else { "" }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(COLOR_BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    let setup_height = if inner.width < 76 { 3 } else { 2 };
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(setup_height),
        Constraint::Min(0),
        Constraint::Length(if inner.height >= 10 { 2 } else { 1 }),
    ])
    .split(inner);
    let raw = state
        .latest_signal_raw
        .map_or_else(|| "--".into(), |raw| raw.to_string());
    let summary = vec![
        Line::from(format!(
            "Hits {}  Heartbeats {}  Latest raw {}",
            state.hit_count, state.heartbeat_count, raw
        )),
        Line::from(if active {
            "Capture owns radio; stop before other radio actions.".into()
        } else if state.truncated_frames > 0 {
            format!("Incomplete frames: {}", state.truncated_frames)
        } else {
            "Raw signal units (not calibrated power).".into()
        }),
    ];
    f.render_widget(
        Paragraph::new(summary).style(Style::default().fg(COLOR_PRIMARY)),
        chunks[0],
    );
    f.render_widget(Paragraph::new("Start a VFO preset scan, then enable Activity Mode on the radio.\nCapture does not change radio settings.")
        .wrap(Wrap { trim: true }).style(Style::default().fg(COLOR_DIM)), chunks[1]);

    if state.log.rows().next().is_none() {
        f.render_widget(
            Paragraph::new(if active && state.heartbeat_count > 0 {
                "Radio scan is running. No signal hits received yet."
            } else if active {
                "Waiting for radio activity. Check manual setup if no hits arrive."
            } else {
                "No activity captured. Press s to start local capture."
            })
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(COLOR_TEXT)),
            chunks[2],
        );
    } else {
        let wide = inner.width >= 43;
        let (headers, widths) = if wide {
            (
                vec!["FrequencyMHz", "Hits", "MeanRaw", "LastSeen"],
                vec![
                    Constraint::Min(12),
                    Constraint::Length(7),
                    Constraint::Length(7),
                    Constraint::Length(8),
                ],
            )
        } else {
            (
                vec!["MHz", "Hits", "Raw"],
                vec![
                    Constraint::Min(10),
                    Constraint::Length(5),
                    Constraint::Length(5),
                ],
            )
        };
        let rows: Vec<Row> = state
            .log
            .rows()
            .map(|row| {
                let mut cells = vec![
                    Cell::from(format_frequency_mhz(row.frequency_10hz)),
                    Cell::from(row.count.to_string()),
                    Cell::from(format!("{:.1}", row.mean_signal_raw())),
                ];
                if wide {
                    cells.push(Cell::from(
                        row.last_seen
                            .with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string(),
                    ));
                }
                Row::new(cells)
            })
            .collect();
        let table = Table::new(rows, widths)
            .header(
                Row::new(headers).style(
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .row_highlight_style(
                Style::default()
                    .bg(COLOR_SELECTION_BG)
                    .fg(COLOR_SELECTION_FG),
            )
            .highlight_symbol("> ")
            .column_spacing(1);
        f.render_stateful_widget(table, chunks[2], &mut app.activity_state);
    }
    let controls = if state.stopping {
        "Stopping; waiting for cleanup | e CSV"
    } else if active {
        "s stop | e CSV | Up/Down select"
    } else {
        "s start | c clear | e CSV | Up/Down select"
    };
    let mut lines = vec![Line::from(controls)];
    if !state.notice.is_empty() {
        lines.push(Line::from(state.notice.clone()));
    }
    f.render_widget(
        Paragraph::new(lines).style(Style::default().fg(if state.notice.is_empty() {
            COLOR_DIM
        } else {
            COLOR_WARNING
        })),
        chunks[3],
    );
}
