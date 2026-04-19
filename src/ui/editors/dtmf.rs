use super::common::{DTMF_EDITOR_HEIGHT, DTMF_EDITOR_WIDTH, begin_editor};
use crate::app::{App, AppMode};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub(crate) fn render_dtmf_editor(f: &mut Frame, app: &App) {
    let (_, inner_area) = begin_editor(
        f,
        " Edit DTMF Preset ".to_string(),
        DTMF_EDITOR_WIDTH,
        DTMF_EDITOR_HEIGHT,
    );

    if let AppMode::EditDTMF(field_idx) = app.mode
        && let Some(idx) = app.dtmf_state.selected()
        && let Some(dtmf) = app.dtmf_presets.get(idx)
    {
        let digits_str: String = dtmf.digits.iter().map(|d| format!("{:X}", d)).collect();
        let fields = [
            ("Label", dtmf.label.clone()),
            ("Digits", digits_str.clone()),
        ];
        let current_value = &fields[field_idx].1;
        let (help_title, help_body) = dtmf_field_help(field_idx);

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(8),
                Constraint::Length(1),
            ])
            .split(inner_area);

        let summary = Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!(" D{:02} ", idx),
                    Style::default()
                        .fg(COLOR_SELECTION_FG)
                        .bg(COLOR_SELECTION_BG)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    " {} | {}",
                    if dtmf.label.is_empty() {
                        "<empty>"
                    } else {
                        dtmf.label.as_str()
                    },
                    fields[field_idx].0
                )),
            ]),
            Line::from(vec![
                Span::styled("Digits ", Style::default().fg(COLOR_PRIMARY)),
                Span::raw(if digits_str.is_empty() {
                    "<empty>"
                } else {
                    digits_str.as_str()
                }),
            ]),
        ])
        .style(Style::default().fg(COLOR_TEXT))
        .block(Block::default().borders(Borders::BOTTOM).title(" Summary "));
        f.render_widget(summary, vertical[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(29), Constraint::Min(30)])
            .split(vertical[1]);

        let details = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Editing ", Style::default().fg(COLOR_PRIMARY)),
                Span::styled(fields[field_idx].0, Style::default().fg(COLOR_TEXT)),
            ]),
            Line::from(vec![
                Span::styled("Current ", Style::default().fg(COLOR_PRIMARY)),
                Span::raw(current_value.as_str()),
            ]),
            Line::default(),
            Line::from(Span::styled(
                help_title,
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(help_body),
            Line::default(),
            Line::from("Digits accept 0-9, A-F, *, and #."),
        ])
        .style(Style::default().fg(COLOR_TEXT))
        .block(Block::default().borders(Borders::ALL).title(" Details "));
        f.render_widget(details, body[0]);

        let rows = fields.iter().enumerate().map(|(i, (label, value))| {
            let style = if i == field_idx {
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            let display_value = if i == field_idx {
                format!("> {} <", app.edit_buffer)
            } else {
                value.clone()
            };
            Row::new(vec![
                Cell::from(*label).style(style),
                Cell::from(display_value).style(style),
            ])
        });

        let table = Table::new(rows, [Constraint::Length(12), Constraint::Min(16)])
            .block(Block::default().borders(Borders::ALL).title(" Fields "))
            .row_highlight_style(
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(" ");
        f.render_widget(table, body[1]);

        f.render_widget(
            Paragraph::new("↑/↓/Tab: Field | Enter: Save | Esc: Cancel")
                .style(Style::default().fg(COLOR_DIM))
                .alignment(ratatui::layout::Alignment::Center),
            vertical[2],
        );
    }
}

fn dtmf_field_help(field_idx: usize) -> (&'static str, &'static str) {
    match field_idx {
        0 => ("Preset Label", "Short label shown in the DTMF preset list."),
        1 => (
            "DTMF Digits",
            "Enter the transmit sequence exactly as it should be sent.",
        ),
        _ => ("Field", ""),
    }
}
