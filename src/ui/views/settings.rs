use crate::app::App;
use crate::protocol::SETTINGS_METADATA;
use crate::ui::render_shortcut;
use crate::ui::theme::COLOR_ACCENT;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_settings_table(f: &mut Frame, app: &mut App, area: Rect) {
    let settings = match &app.settings {
        Some(s) => s,
        None => {
            let hint = Paragraph::new("\n\nNO SETTINGS LOADED\n\nPress 'r' to read from radio")
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(hint, area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let rows = SETTINGS_METADATA.iter().enumerate().map(|(i, meta)| {
        let display_val = settings.get_display_value(i);
        let style = if Some(i) == app.settings_state.selected() {
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        Row::new(vec![
            Cell::from(format!("{:02}", i)),
            Cell::from(meta.name),
            Cell::from(display_val),
        ])
        .style(style)
    });

    let (title, border_style) = if app.settings_dirty {
        (
            " RADIO SETTINGS (UNSAVED) ",
            Style::default().fg(Color::Yellow),
        )
    } else {
        (" RADIO SETTINGS ", Style::default().fg(Color::Green))
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(25),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("#").style(Style::default().fg(Color::Yellow)),
            Cell::from("Setting").style(Style::default().fg(Color::Yellow)),
            Cell::from("Value").style(Style::default().fg(Color::Yellow)),
        ])
        .style(Style::default().bg(Color::DarkGray))
        .height(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(border_style),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(40, 80, 40))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[0], &mut app.settings_state);

    let help = Paragraph::new(format!(
        "{}: Nav | {}: Edit | {}: Write | {}: Read",
        render_shortcut("↑/↓"),
        render_shortcut("Enter"),
        render_shortcut("w"),
        render_shortcut("r")
    ))
    .alignment(ratatui::layout::Alignment::Center)
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, chunks[1]);
}
