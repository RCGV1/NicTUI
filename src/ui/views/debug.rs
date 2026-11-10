use crate::app::{App, logs::classify_log};
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_ERROR, COLOR_PRIMARY, COLOR_SECONDARY, COLOR_SURFACE_1,
    COLOR_TEXT, COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};

fn header_rows(area: Rect) -> u16 {
    if area.height >= 10 { 5 } else { 2 }
}

/// The same page size used by the renderer, for keyboard dispatch.
pub fn log_page_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(header_rows(area) + 2)).max(1)
}

pub fn render_debug_logs(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks =
        Layout::vertical([Constraint::Length(header_rows(area)), Constraint::Min(0)]).split(area);
    app.log_view.set_width(chunks[1].width.saturating_sub(2));
    let rows = app.log_view.rows(&app.logs);
    app.log_view.clamp(rows.len());
    let page_rows = usize::from(chunks[1].height.saturating_sub(2));
    let start = app.log_view.scroll;
    let end = start.saturating_add(page_rows).min(rows.len());
    let matched = app
        .logs
        .iter()
        .filter(|line| app.log_view.filter.matches(line))
        .count();
    let (mut rx, mut tx, mut remote, mut errors) = (0, 0, 0, 0);
    for line in &app.logs {
        let class = classify_log(line);
        rx += usize::from(class.rx);
        tx += usize::from(class.tx);
        remote += usize::from(class.remote);
        errors += usize::from(class.error);
    }
    let status = format!(
        "{} | Filter: {} | {matched}/{} entries | rows {}-{end}/{}",
        if app.log_view.follow {
            "LIVE"
        } else {
            "PAUSED"
        },
        app.log_view.filter.label(),
        app.logs.len(),
        if end > start { start + 1 } else { 0 },
        rows.len(),
    );
    let summary = Paragraph::new(vec![
        Line::styled(
            status,
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        ),
        Line::from(format!(
            "RX {rx} | TX {tx} | Remote {remote} | Errors {errors}"
        )),
        Line::from("Up/Down PgUp/PgDn browse | Home newest (paused)"),
        Line::from("End live | f filter | c clear local | F1/? help"),
        Line::from("Newest first. RX/TX are data, not RF."),
    ])
    .style(Style::default().fg(COLOR_DIM))
    .wrap(Wrap { trim: false });
    f.render_widget(summary, chunks[0]);

    let empty = rows.is_empty();
    let lines: Vec<Line> = if empty {
        vec![Line::styled(
            if app.logs.is_empty() {
                "No activity logged yet. Local history is empty.".to_string()
            } else {
                format!(
                    "No {} entries match. Press f to change filter.",
                    app.log_view.filter.label()
                )
            },
            Style::default().fg(COLOR_DIM),
        )]
    } else {
        rows.into_iter()
            .skip(start)
            .take(page_rows)
            .map(|(line, class)| {
                let color = if class.tx {
                    COLOR_SECONDARY
                } else if class.rx {
                    COLOR_PRIMARY
                } else if class.error {
                    COLOR_ERROR
                } else if class.remote {
                    COLOR_WARNING
                } else {
                    COLOR_TEXT
                };
                Line::styled(line, Style::default().fg(color))
            })
            .collect()
    };
    let mut list = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Recent Activity | Newest First ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .style(Style::default().bg(COLOR_SURFACE_1)),
    );
    if empty {
        list = list.wrap(Wrap { trim: false });
    }
    f.render_widget(list, chunks[1]);
}
