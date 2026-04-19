use crate::app::App;
use crate::device::{PortCandidate, PortKind};
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

pub fn render_port_selection(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    let welcome = Paragraph::new(
        "NicTUI\n\nConnect to the radio, choose the programming port, and start reading or editing live data.",
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
                .title(" Connect to Radio ")
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(welcome, chunks[0]);

    let selected_candidate = app.selected_port_candidate();
    let selected_port = selected_candidate
        .map(|candidate| candidate.port_name.as_str())
        .unwrap_or("None selected");
    let selected_kind = selected_candidate
        .map(|candidate| candidate.badge())
        .unwrap_or("none");
    let status = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Detected: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                format!("{}", app.port_candidates.len()),
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ports", Style::default().fg(COLOR_DIM)),
        ]),
        Line::from(vec![
            Span::styled("Radio Port: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                selected_port,
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                selected_kind,
                Style::default().fg(COLOR_DIM).add_modifier(Modifier::BOLD),
            ),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Session ")
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(status, chunks[1]);

    let ports: Vec<ListItem> = if app.port_candidates.is_empty() {
        vec![ListItem::new(" No serial ports detected ").style(Style::default().fg(COLOR_DIM))]
    } else {
        app.port_candidates
            .iter()
            .enumerate()
            .map(|(i, candidate)| {
                let style = if i == app.selected_port_index {
                    Style::default()
                        .fg(COLOR_SELECTION_FG)
                        .bg(COLOR_SELECTION_BG)
                        .add_modifier(Modifier::BOLD)
                } else if matches!(candidate.kind, PortKind::Radio | PortKind::Candidate) {
                    Style::default().fg(COLOR_ACCENT)
                } else {
                    Style::default().fg(COLOR_DIM)
                };
                let mut line = vec![Span::styled(candidate.port_name.as_str(), style)];
                line.push(Span::raw("  "));
                line.push(Span::styled(
                    candidate.badge(),
                    Style::default().fg(COLOR_DIM).add_modifier(Modifier::BOLD),
                ));
                if let Some(detail) = port_detail(candidate) {
                    line.push(Span::raw("  "));
                    line.push(Span::styled(detail, Style::default().fg(COLOR_DIM)));
                }
                ListItem::new(Line::from(line)).style(style)
            })
            .collect()
    };

    let list = List::new(ports)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Serial Ports ")
                .border_style(Style::default().fg(COLOR_BORDER)),
        )
        .highlight_symbol(" ▶ ");
    f.render_widget(list, chunks[2]);

    let help = Paragraph::new(Line::from(vec![
        render_shortcut("↑/↓"),
        Span::raw(" Navigate | "),
        render_shortcut("Enter"),
        Span::raw(" Select | "),
        render_shortcut("r"),
        Span::raw(" Refresh | "),
        render_shortcut("nictui ports"),
        Span::raw(" CLI | "),
        render_shortcut("q"),
        Span::raw(" Quit "),
    ]))
    .alignment(ratatui::layout::Alignment::Center)
    .style(Style::default().fg(COLOR_DIM))
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(help, chunks[3]);
}

fn port_detail(candidate: &PortCandidate) -> Option<String> {
    if let (Some(vid), Some(pid)) = (candidate.usb_vid, candidate.usb_pid) {
        return Some(format!("VID:PID {:04X}:{:04X}", vid, pid));
    }

    candidate
        .product
        .as_deref()
        .or(candidate.manufacturer.as_deref())
        .map(str::to_string)
}
