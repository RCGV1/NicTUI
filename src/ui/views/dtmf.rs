use crate::app::App;
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG,
    COLOR_SURFACE_1, COLOR_SURFACE_2, COLOR_TEXT, COLOR_WARNING,
};
use crate::ui::views::ready_state::{ReadyStateAction, ReadyStateContent, render_ready_state};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_dtmf(f: &mut Frame, app: &mut App, area: Rect) {
    if app.dtmf_presets.is_empty() {
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "DTMF Presets",
                card_title: "Ready To Load",
                heading: "No DTMF Loaded",
                description: "Read the radio to load the current DTMF preset bank.".to_string(),
                note: None,
            },
            &[ReadyStateAction {
                key: "r",
                label: "read radio",
            }],
        );
        return;
    }

    let labeled = app
        .dtmf_presets
        .iter()
        .filter(|preset| !preset.label.trim().is_empty())
        .count();
    let total_digits: usize = app
        .dtmf_presets
        .iter()
        .map(|preset| preset.digits.len())
        .sum();
    let selected_label = app
        .dtmf_state
        .selected()
        .and_then(|index| app.dtmf_presets.get(index))
        .map(|preset| format!("P{:02}", preset.index))
        .unwrap_or_else(|| "--".to_string());

    let summary_height = if area.width < 90 { 3 } else { 2 };
    let detail_height = if area.width < 90 { 6 } else { 5 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Min(1),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let summary_text = if area.width < 90 {
        format!(
            "{} presets | {} named | {}",
            app.dtmf_presets.len(),
            labeled,
            selected_label
        )
    } else {
        format!(
            "{} presets | {} labeled | {} digits total | focus {}",
            app.dtmf_presets.len(),
            labeled,
            total_digits,
            selected_label
        )
    };
    let summary = Paragraph::new(summary_text)
        .style(Style::default().fg(COLOR_DIM))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(summary, chunks[0]);

    let header = Row::new(vec![
        Cell::from("#").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("Label").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("Sequence").style(Style::default().fg(COLOR_ACCENT)),
    ])
    .style(Style::default().bg(COLOR_SURFACE_2))
    .height(1);

    let rows = app.dtmf_presets.iter().enumerate().map(|(i, dp)| {
        let style = if Some(i) == app.dtmf_state.selected() {
            Style::default()
                .bg(COLOR_SELECTION_BG)
                .fg(COLOR_SELECTION_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_TEXT)
        };
        let digits: String = dp.digits.iter().map(|d| format!("{:X}", d)).collect();
        Row::new(vec![dp.index.to_string(), dp.label.clone(), digits]).style(style)
    });

    let title = if app.dtmf_dirty {
        " DTMF PRESETS (UNSAVED) "
    } else {
        " DTMF PRESETS "
    };
    let border_style = if app.dtmf_dirty {
        Style::default().fg(COLOR_WARNING)
    } else {
        Style::default().fg(COLOR_BORDER)
    };

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
            .title(title)
            .border_style(border_style)
            .bg(COLOR_SURFACE_1),
    )
    .row_highlight_style(
        Style::default()
            .fg(COLOR_SELECTION_FG)
            .bg(COLOR_SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[1], &mut app.dtmf_state);

    let compact = area.width < 90;
    let detail_lines = app
        .dtmf_state
        .selected()
        .and_then(|index| app.dtmf_presets.get(index))
        .map(|preset| {
            let digits: String = preset
                .digits
                .iter()
                .map(|digit| format!("{:X}", digit))
                .collect();
            vec![
                Line::from(vec![
                    Span::styled(
                        format!(" P{:02} ", preset.index),
                        Style::default()
                            .fg(COLOR_SELECTION_FG)
                            .bg(COLOR_SELECTION_BG)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        " {} | {} {}",
                        if preset.label.is_empty() {
                            "<unnamed>"
                        } else {
                            preset.label.as_str()
                        },
                        preset.digits.len(),
                        if compact { "dig" } else { "digits" },
                    )),
                ]),
                Line::from(vec![
                    Span::styled("Sequence ", Style::default().fg(COLOR_PRIMARY)),
                    Span::raw(if digits.is_empty() {
                        "<empty>".to_string()
                    } else {
                        digits.clone()
                    }),
                ]),
                Line::from(vec![
                    render_shortcut("Enter"),
                    Span::raw(": Edit | "),
                    render_shortcut("r"),
                    Span::raw(": Read | "),
                    render_shortcut("w"),
                    Span::raw(": Write"),
                ]),
            ]
        })
        .unwrap_or_else(|| vec![Line::from("No DTMF preset selected")]);

    let detail_bar = Paragraph::new(detail_lines)
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Focus ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(detail_bar, chunks[2]);
}
