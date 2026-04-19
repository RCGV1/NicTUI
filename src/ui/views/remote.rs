use crate::app::App;
use crate::protocol::RemotePacket;
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SUCCESS, COLOR_TEXT, COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
};

const LCD_COLS: usize = 30;
const LCD_ROWS: usize = 8;

pub fn render_remote_screen(f: &mut Frame, app: &App, area: Rect) {
    let summary_height = if area.width < 100 { 3 } else { 2 };
    let detail_height = if area.width < 110 { 10 } else { 8 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Min(12),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let battery = app
        .remote_screen
        .battery_level
        .map(|level| format!("{level}%"))
        .unwrap_or_else(|| "unknown".to_string());
    let summary = Paragraph::new(format!(
        "session {} | signal {}% | noise {}% | battery {} | packets {}",
        if app.remote_active { "live" } else { "idle" },
        app.remote_screen.signal_strength.min(100),
        app.remote_screen.noise_level.min(100),
        battery,
        app.remote_screen.elements.len()
    ))
    .style(Style::default().fg(COLOR_DIM))
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(summary, chunks[0]);

    let body = if chunks[1].width < 110 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Length(18)])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(chunks[1])
    };

    render_lcd_preview(f, app, body[0]);
    render_status_sidebar(f, app, body[1]);

    let detail_split = if chunks[2].width < 100 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(5)])
            .split(chunks[2])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(chunks[2])
    };

    render_activity(f, app, detail_split[0]);
    render_controls(f, app, detail_split[1]);
}

fn render_lcd_preview(f: &mut Frame, app: &App, area: Rect) {
    let preview = build_lcd_preview(app);
    let paragraph = Paragraph::new(preview)
        .style(
            Style::default()
                .fg(Color::Rgb(198, 255, 214))
                .bg(Color::Rgb(14, 26, 18)),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" LCD Preview ")
                .border_style(if app.remote_active {
                    Style::default().fg(COLOR_SUCCESS)
                } else {
                    Style::default().fg(COLOR_BORDER)
                }),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_status_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Min(6),
        ])
        .split(area);

    let session_lines = if app.remote_active {
        vec![
            Line::from(Span::styled(
                "Live remote session",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("Digits stay on the radio keypad."),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "Session idle",
                Style::default().fg(COLOR_DIM).add_modifier(Modifier::BOLD),
            )),
            Line::from("Press o to open a remote session."),
        ]
    };
    let session = Paragraph::new(session_lines)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Session ")
                .border_style(if app.remote_active {
                    Style::default().fg(COLOR_SUCCESS)
                } else {
                    Style::default().fg(COLOR_BORDER)
                }),
        );
    f.render_widget(session, chunks[0]);

    render_signal_gauge(
        f,
        chunks[1],
        "Signal",
        app.remote_screen.signal_strength.min(100),
        COLOR_PRIMARY,
        Color::Rgb(25, 30, 35),
    );
    render_signal_gauge(
        f,
        chunks[2],
        "Noise",
        app.remote_screen.noise_level.min(100),
        COLOR_WARNING,
        Color::Rgb(35, 25, 20),
    );

    let battery_label = app
        .remote_screen
        .battery_level
        .map(|level| format!("{level}%"))
        .unwrap_or_else(|| "unknown".to_string());
    let link = Paragraph::new(vec![
        Line::from(format!(
            "State {}",
            if app.remote_active { "live" } else { "idle" }
        )),
        Line::from(format!("Battery {}", battery_label)),
        Line::from(format!("Decoded {}", app.remote_screen.elements.len())),
        Line::from("LCD view updates from decoded packets."),
    ])
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Link ")
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(link, chunks[3]);
}

fn render_signal_gauge(f: &mut Frame, area: Rect, title: &str, value: u8, fg: Color, bg: Color) {
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} ")),
        )
        .gauge_style(Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD))
        .label(format!("{value}%"))
        .percent(value as u16);
    f.render_widget(gauge, area);
}

fn render_activity(f: &mut Frame, app: &App, area: Rect) {
    let lines = if app.remote_screen.elements.is_empty() {
        "No decoded remote packets yet.\n\nOpen a session and press a few keys to build the LCD preview."
            .to_string()
    } else {
        app.remote_screen
            .elements
            .iter()
            .rev()
            .take(8)
            .map(RemotePacket::summary)
            .collect::<Vec<_>>()
            .join("\n")
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Activity ")
                .border_style(Style::default().fg(COLOR_BORDER)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_controls(f: &mut Frame, app: &App, area: Rect) {
    let lines = if app.remote_active {
        vec![
            Line::from(vec![
                render_shortcut("0-9"),
                Span::raw(": Digits | "),
                render_shortcut("m"),
                Span::raw(": Menu | "),
                render_shortcut("e / Esc"),
                Span::raw(": Exit"),
            ]),
            Line::from(vec![
                render_shortcut("u / d"),
                Span::raw(": Nav | "),
                render_shortcut("* / #"),
                Span::raw(": Keys | "),
                render_shortcut("a / b"),
                Span::raw(": PTT"),
            ]),
            Line::from(vec![
                render_shortcut("f"),
                Span::raw(": Light | "),
                render_shortcut("v"),
                Span::raw(": V-M | "),
                render_shortcut("Tab"),
                Span::raw(": Leave remote"),
            ]),
        ]
    } else {
        vec![
            Line::from(vec![
                render_shortcut("o"),
                Span::raw(": Start session | "),
                render_shortcut("Esc"),
                Span::raw(": Back"),
            ]),
            Line::from(vec![
                render_shortcut("Tab"),
                Span::raw(": Next tab | "),
                render_shortcut("Shift-Tab"),
                Span::raw(": Previous tab"),
            ]),
            Line::from(vec![render_shortcut("1-9"), Span::raw(": Switch tabs")]),
        ]
    };

    let controls = Paragraph::new(lines)
        .style(Style::default().fg(COLOR_TEXT))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Controls ")
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(controls, area);
}

fn build_lcd_preview(app: &App) -> Vec<Line<'static>> {
    let mut canvas = vec![vec![' '; LCD_COLS]; LCD_ROWS];

    for packet in app.remote_screen.elements.iter() {
        match packet {
            RemotePacket::DisplayText { x, y, text, .. } => {
                let row = ((*y as usize) * LCD_ROWS / 128).min(LCD_ROWS.saturating_sub(1));
                let col = ((*x as usize) * LCD_COLS / 160).min(LCD_COLS.saturating_sub(1));
                for (offset, ch) in text.chars().enumerate() {
                    let col = col + offset;
                    if col >= LCD_COLS {
                        break;
                    }
                    canvas[row][col] = sanitize_preview_char(ch);
                }
            }
            RemotePacket::DrawSymbol { x, y, .. } => {
                let row = ((*y as usize) * LCD_ROWS / 128).min(LCD_ROWS.saturating_sub(1));
                let col = ((*x as usize) * LCD_COLS / 160).min(LCD_COLS.saturating_sub(1));
                canvas[row][col] = '#';
            }
            _ => {}
        }
    }

    let mut lines = vec![
        Line::from(Span::styled(
            "Approximate display built from decoded text packets",
            Style::default().fg(COLOR_DIM),
        )),
        Line::from(""),
    ];

    for row in canvas {
        let text: String = row.into_iter().collect();
        lines.push(Line::from(text));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Unparsed radio draw commands will not appear here yet.",
        Style::default().fg(COLOR_DIM),
    )));

    lines
}

fn sanitize_preview_char(ch: char) -> char {
    if ch.is_ascii_graphic() || ch == ' ' {
        ch
    } else {
        '?'
    }
}
