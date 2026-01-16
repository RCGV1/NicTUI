use crate::app::App;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_HEADER, COLOR_PRIMARY, COLOR_SUCCESS,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    widgets::{Block, Borders, Paragraph},
};

pub fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(COLOR_BORDER))
        .bg(COLOR_HEADER);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(36),
            Constraint::Min(0),
            Constraint::Length(30),
        ])
        .split(inner);

    let title = Paragraph::new(" 📻💻 NicTUI ").style(
        Style::default()
            .fg(COLOR_PRIMARY)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(title, chunks[0]);

    let subtitle = Paragraph::new(" TDH3 Radio Programmer ")
        .style(Style::default().fg(COLOR_DIM).add_modifier(Modifier::BOLD));
    f.render_widget(subtitle, chunks[0]);

    let status = if app.remote_active {
        " REMOTE ACTIVE "
    } else {
        " IDLE "
    };
    let status_style = if app.remote_active {
        Style::default()
            .fg(COLOR_SUCCESS)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_DIM)
    };
    f.render_widget(
        Paragraph::new(status)
            .alignment(ratatui::layout::Alignment::Center)
            .style(status_style),
        chunks[1],
    );

    let port = format!(
        " 📟 {} ",
        app.protocol_port_name.as_deref().unwrap_or("No Port")
    );
    f.render_widget(
        Paragraph::new(port)
            .alignment(ratatui::layout::Alignment::Right)
            .style(Style::default().fg(COLOR_ACCENT)),
        chunks[2],
    );
}
