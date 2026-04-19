use crate::app::App;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SECONDARY, COLOR_SURFACE_1, COLOR_TEXT,
    COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

pub fn render_debug_logs(f: &mut Frame, app: &App, area: Rect) {
    let summary_height = if area.width < 90 { 3 } else { 2 };
    let compact = area.width < 90;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Length(4),
            Constraint::Min(10),
        ])
        .split(area);

    let remote_count = app
        .logs
        .iter()
        .filter(|line| line.contains("REMOTE:"))
        .count();
    let rx_count = app.logs.iter().filter(|line| line.contains("RX:")).count();
    let tx_count = app.logs.iter().filter(|line| line.contains("TX:")).count();

    let summary = Paragraph::new(format!(
        "{} entries | {} RX | {} TX | {} remote",
        app.logs.len(),
        rx_count,
        tx_count,
        remote_count
    ))
    .style(Style::default().fg(COLOR_DIM))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(summary, chunks[0]);

    let hero = Paragraph::new(vec![
        Line::from(Span::styled(
            "Activity Debug",
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            if compact {
                "Recent traffic with faster visual scanning."
            } else {
                "A cleaner readout of recent traffic so protocol activity is easier to scan quickly."
            },
            Style::default().fg(COLOR_DIM),
        )),
        Line::from(vec![
            metric_chip("RX", rx_count.to_string(), COLOR_PRIMARY),
            Span::raw(" "),
            metric_chip("TX", tx_count.to_string(), COLOR_SECONDARY),
            Span::raw(" "),
            metric_chip("REMOTE", remote_count.to_string(), COLOR_WARNING),
        ]),
    ])
    .style(Style::default().bg(COLOR_SURFACE_1))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(hero, chunks[1]);

    let body = if chunks[2].width < 110 {
        None
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
            .split(chunks[2])
            .into()
    };

    let logs: Vec<ListItem> = if app.logs.is_empty() {
        vec![ListItem::new("No activity logged yet").style(Style::default().fg(COLOR_DIM))]
    } else {
        app.logs
            .iter()
            .rev()
            .map(|line| {
                let style = if line.contains("TX:") {
                    Style::default().fg(COLOR_SECONDARY)
                } else if line.contains("RX:") {
                    Style::default().fg(COLOR_PRIMARY)
                } else if line.contains("REMOTE:") {
                    Style::default().fg(COLOR_WARNING)
                } else {
                    Style::default().fg(COLOR_TEXT)
                };
                ListItem::new(line.as_str()).style(style)
            })
            .collect()
    };

    let list = List::new(logs).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Recent Activity ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );

    if let Some(body) = body {
        let insight = Paragraph::new(vec![
            detail_row("Latest", latest_log(app).unwrap_or("No activity yet")),
            detail_row(
                "Bus",
                if rx_count + tx_count > 0 {
                    "Traffic present"
                } else {
                    "Idle"
                },
            ),
            detail_row("Remote", if remote_count > 0 { "Seen" } else { "None" }),
            Line::from(""),
            Line::from(Span::styled(
                "Newest entries stay at the top so bursts are easier to follow.",
                Style::default().fg(COLOR_DIM),
            )),
        ])
        .style(Style::default().bg(COLOR_SURFACE_1))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Insight ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
        f.render_widget(insight, body[0]);
        f.render_widget(list, body[1]);
    } else {
        f.render_widget(list, chunks[2]);
    }
}

fn metric_chip(
    label: impl Into<String>,
    value: impl Into<String>,
    color: ratatui::style::Color,
) -> Span<'static> {
    let label = label.into();
    let value = value.into();
    Span::styled(
        format!(" {label} {value} "),
        Style::default()
            .fg(crate::ui::theme::COLOR_SURFACE_0)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn detail_row(label: impl Into<String>, value: impl Into<String>) -> Line<'static> {
    let label = label.into();
    let value = value.into();
    Line::from(vec![
        Span::styled(
            format!("{label:<8}"),
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value, Style::default().fg(COLOR_TEXT)),
    ])
}

fn latest_log(app: &App) -> Option<&str> {
    app.logs.back().map(|line| line.as_str())
}
