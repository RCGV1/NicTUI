use crate::app::App;
use crate::ui::render_shortcut;
use crate::ui::theme::{COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

pub fn render_port_selection(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10),
            Constraint::Min(0),
            Constraint::Length(4),
        ])
        .split(area);

    let welcome = Paragraph::new(
        "\n 📻💻 NicTUI \n\n Professional TDH3 Radio Programmer\n\nSelect a serial port to begin.",
    )
    .alignment(ratatui::layout::Alignment::Center)
    .style(
        Style::default()
            .fg(COLOR_PRIMARY)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(welcome, chunks[0]);

    let ports: Vec<ListItem> = app
        .ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == app.selected_port_index {
                Style::default()
                    .fg(Color::Black)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            ListItem::new(format!(" 📟 {} ", p)).style(style)
        })
        .collect();

    let list = List::new(ports)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Available Ports ")
                .border_style(Style::default().fg(COLOR_BORDER)),
        )
        .highlight_symbol(" ▶ ");
    f.render_widget(list, chunks[1]);

    let help = Paragraph::new(Line::from(vec![
        render_shortcut("↑/↓"),
        Span::raw(" Navigate | "),
        render_shortcut("Enter"),
        Span::raw(" Select | "),
        render_shortcut("r"),
        Span::raw(" Refresh | "),
        render_shortcut("q"),
        Span::raw(" Quit "),
    ]))
    .alignment(ratatui::layout::Alignment::Center)
    .style(Style::default().fg(COLOR_DIM));
    f.render_widget(help, chunks[2]);
}
