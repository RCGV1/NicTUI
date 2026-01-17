use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_dtmf(f: &mut Frame, app: &mut App, area: Rect) {
    if app.dtmf_presets.is_empty() {
        let hint = Paragraph::new("\n\nNO DTMF LOADED\n\nPress 'r' to read from radio")
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
        Cell::from("Label").style(Style::default().fg(Color::Yellow)),
        Cell::from("Sequence").style(Style::default().fg(Color::Yellow)),
    ])
    .style(Style::default().bg(Color::DarkGray))
    .height(1);

    let rows = app.dtmf_presets.iter().enumerate().map(|(i, dp)| {
        let style = if Some(i) == app.dtmf_state.selected() {
            Style::default()
                .bg(Color::Rgb(40, 40, 80))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let digits: String = dp.digits.iter().map(|d| format!("{:X}", d)).collect();
        Row::new(vec![dp.index.to_string(), dp.label.clone(), digits]).style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(15),
            Constraint::Min(0),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" DTMF PRESETS ")
            .border_style(Style::default().fg(Color::Cyan)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(40, 40, 80))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[0], &mut app.dtmf_state);
}
