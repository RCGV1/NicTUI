use crate::app::App;
use crate::ui::render_shortcut;
use crate::ui::theme::COLOR_PRIMARY;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_bandplan(f: &mut Frame, app: &mut App, area: Rect) {
    if app.band_plans.is_empty() {
        let hint = Paragraph::new("\n\nNO BANDPLAN LOADED\n\nPress 'r' to read from radio")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(hint, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let header = Row::new(vec![
        Cell::from("#").style(Style::default().fg(Color::Yellow)),
        Cell::from("Start").style(Style::default().fg(Color::Yellow)),
        Cell::from("End").style(Style::default().fg(Color::Yellow)),
        Cell::from("Power").style(Style::default().fg(Color::Yellow)),
        Cell::from("TX").style(Style::default().fg(Color::Yellow)),
        Cell::from("Mod").style(Style::default().fg(Color::Yellow)),
        Cell::from("BW").style(Style::default().fg(Color::Yellow)),
    ])
    .style(Style::default().bg(Color::DarkGray))
    .height(1);

    let rows = app.band_plans.iter().enumerate().map(|(i, bp)| {
        let is_selected = Some(i) == app.bandplan_state.selected();

        let style = if is_selected {
            Style::default()
                .fg(Color::White)
                .bg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else if i % 2 == 0 {
            Style::default().bg(Color::Rgb(30, 30, 35))
        } else {
            Style::default()
        };
        Row::new(vec![
            bp.index.to_string(),
            format!("{:.5}", bp.start_freq as f64 / 100000.0),
            format!("{:.5}", bp.end_freq as f64 / 100000.0),
            bp.max_power.to_string(),
            if bp.tx_allowed {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
            match bp.modulation {
                1 => "AM".to_string(),
                2 => "USB".to_string(),
                _ => "FM".to_string(),
            },
            if bp.bandwidth == 1 {
                "Narrow".to_string()
            } else {
                "Wide".to_string()
            },
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" BAND PLAN ")
            .border_style(Style::default().fg(Color::Cyan)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(40, 40, 80))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[0], &mut app.bandplan_state);

    let help = Paragraph::new(Line::from(vec![
        render_shortcut("↑/↓"),
        Span::raw(": Nav | "),
        render_shortcut("Enter"),
        Span::raw(": Edit | "),
        render_shortcut("r"),
        Span::raw(": Read"),
    ]))
    .alignment(ratatui::layout::Alignment::Center)
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[1]);
}
