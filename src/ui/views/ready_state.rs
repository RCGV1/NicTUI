use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_SURFACE_1, COLOR_SURFACE_2, COLOR_SURFACE_3, COLOR_TEXT,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub struct ReadyStateAction<'a> {
    pub key: &'a str,
    pub label: &'a str,
}

pub struct ReadyStateContent<'a> {
    pub outer_title: &'a str,
    pub card_title: &'a str,
    pub heading: &'a str,
    pub description: String,
    pub note: Option<&'a str>,
}

pub fn render_ready_state(
    f: &mut Frame,
    area: Rect,
    content: ReadyStateContent<'_>,
    actions: &[ReadyStateAction<'_>],
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", content.outer_title))
        .border_style(Style::default().fg(COLOR_BORDER))
        .bg(COLOR_SURFACE_1);
    f.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let card_shell = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(9),
            Constraint::Min(0),
        ])
        .split(inner);

    let card_width = card_shell[1].width.min(72);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(card_width),
            Constraint::Fill(1),
        ])
        .split(card_shell[1]);
    let card_area = horizontal[1];

    let card = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", content.card_title))
        .border_style(Style::default().fg(COLOR_BORDER))
        .bg(COLOR_SURFACE_2);
    let card_inner = card.inner(card_area);
    f.render_widget(card, card_area);

    let mut lines = vec![
        Line::from(Span::styled(
            content.heading.to_string(),
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            content.description,
            Style::default().fg(COLOR_DIM),
        )),
        Line::from(""),
        action_line(actions),
    ];

    if let Some(note) = content.note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            note.to_string(),
            Style::default().fg(COLOR_DIM),
        )));
    }

    let empty_state = Paragraph::new(lines)
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().bg(COLOR_SURFACE_2))
        .wrap(Wrap { trim: true });
    f.render_widget(empty_state, card_inner);
}

fn action_line(actions: &[ReadyStateAction<'_>]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(
            format!(" {} ", action.key.to_uppercase()),
            Style::default()
                .fg(COLOR_TEXT)
                .bg(COLOR_SURFACE_3)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", action.label),
            Style::default().fg(COLOR_TEXT),
        ));
    }
    Line::from(spans)
}
