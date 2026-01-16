use crate::app::App;
use crate::ui::render_shortcut;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_remote_screen(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" RADIO REMOTE ");
    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .constraints([Constraint::Length(3), Constraint::Min(10)])
        .split(inner_area);

    render_help(f, app, chunks[0]);
    render_keybinds(f, chunks[1]);
}

fn render_help(f: &mut Frame, app: &App, area: Rect) {
    let text = if app.remote_active {
        "Connected - press o to start, p to stop, q to quit"
    } else {
        "Disconnected - press o to connect"
    };
    let color = if app.remote_active {
        Color::Green
    } else {
        Color::DarkGray
    };

    let p = Paragraph::new(text)
        .style(Style::default().fg(color))
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(p, area);
}

fn render_keybinds(f: &mut Frame, area: Rect) {
    let title = Block::default().title(" KEYBINDS ").borders(Borders::ALL);
    let inner = title.inner(area);
    f.render_widget(title, area);

    let rows = vec![
        Row::new(vec![
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("o"), "Connect"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("p"), "Disconnect"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("q"), "Quit"),
                Style::default().fg(Color::White),
            )),
        ])
        .height(2),
        Row::new(vec![
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("0-9"), "Radio Keys"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("*"), "Star"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("#"), "Pound"),
                Style::default().fg(Color::White),
            )),
        ])
        .height(2),
        Row::new(vec![
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("u"), "Up"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("d"), "Down"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("v"), "V/M"),
                Style::default().fg(Color::White),
            )),
        ])
        .height(2),
        Row::new(vec![
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("m"), "Menu"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("e"), "Exit"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("f"), "Flashlight"),
                Style::default().fg(Color::White),
            )),
        ])
        .height(2),
        Row::new(vec![
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("a"), "PTT A"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled(
                format!("{} = {}", render_shortcut("b"), "PTT B"),
                Style::default().fg(Color::White),
            )),
            Cell::from(Span::styled("".to_string(), Style::default())),
        ])
        .height(2),
    ];

    let widths = vec![
        Constraint::Length(18),
        Constraint::Length(18),
        Constraint::Length(18),
    ];

    let table = Table::new(rows, widths).column_spacing(1);

    f.render_widget(table, inner);
}
