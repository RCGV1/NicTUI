use super::common::{
    BANDPLAN_EDITOR_HEIGHT, BANDPLAN_EDITOR_WIDTH, begin_editor, render_option_popup,
};
use crate::app::{App, AppMode};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub(crate) fn render_bandplan_editor(f: &mut Frame, app: &App) {
    let Some(bp) = app.editing_band_plan.as_ref() else {
        return;
    };

    let (area, inner_area) = begin_editor(
        f,
        format!(" Band Plan {} ", bp.index),
        BANDPLAN_EDITOR_WIDTH,
        BANDPLAN_EDITOR_HEIGHT,
    );

    let current_field_idx = if let AppMode::EditBandPlan(idx) = app.mode {
        idx
    } else {
        0
    };

    let mod_str = match bp.modulation {
        1 => "AM".to_string(),
        2 => "USB".to_string(),
        _ => "FM".to_string(),
    };

    let bw_str = match bp.bandwidth {
        1 => "Narrow".to_string(),
        _ => "Wide".to_string(),
    };

    let fields = [
        ("Index", bp.index.to_string(), false),
        (
            "Start Freq",
            format!("{:.5}", bp.start_freq as f64 / 100000.0),
            false,
        ),
        (
            "End Freq",
            format!("{:.5}", bp.end_freq as f64 / 100000.0),
            false,
        ),
        ("Max Power", bp.max_power.to_string(), false),
        (
            "TX Allowed",
            if bp.tx_allowed { "Yes" } else { "No" }.to_string(),
            true,
        ),
        ("Wrap", if bp.wrap { "Yes" } else { "No" }.to_string(), true),
        ("Modulation", mod_str, true),
        ("Bandwidth", bw_str, true),
    ];
    let current_value = &fields[current_field_idx].1;
    let (help_title, help_body) = bandplan_field_help(current_field_idx);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(inner_area);

    let summary = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                format!(" B{:02} ", bp.index),
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {}-{} | {}",
                bp.start_freq as f64 / 100000.0,
                bp.end_freq as f64 / 100000.0,
                fields[current_field_idx].0
            )),
        ]),
        Line::from(vec![
            Span::styled("TX ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(if bp.tx_allowed { "Yes" } else { "No" }),
            Span::raw(" | "),
            Span::styled("Wrap ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(if bp.wrap { "Yes" } else { "No" }),
            Span::raw(" | "),
            Span::styled("Mode ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(match bp.modulation {
                1 => "AM",
                2 => "USB",
                _ => "FM",
            }),
            Span::raw(" | "),
            Span::styled("BW ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(if bp.bandwidth == 1 { "Narrow" } else { "Wide" }),
        ]),
        Line::from(vec![
            Span::styled("Max Power ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(bp.max_power.to_string()),
        ]),
    ])
    .style(Style::default().fg(COLOR_TEXT))
    .block(Block::default().borders(Borders::BOTTOM).title(" Summary "));
    f.render_widget(summary, vertical[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(29), Constraint::Min(34)])
        .split(vertical[1]);

    let details = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Editing ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(fields[current_field_idx].0, Style::default().fg(COLOR_TEXT)),
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
        Line::from(if fields[current_field_idx].2 {
            "Use ←/→ to change the selected option."
        } else {
            "Type to edit the numeric value directly."
        }),
    ])
    .style(Style::default().fg(COLOR_TEXT))
    .block(Block::default().borders(Borders::ALL).title(" Details "));
    f.render_widget(details, body[0]);

    let rows = fields.iter().enumerate().map(|(i, (label, value, _))| {
        let style = if i == current_field_idx {
            Style::default()
                .fg(COLOR_SELECTION_FG)
                .bg(COLOR_SELECTION_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_DIM)
        };
        let display_value = if i == current_field_idx {
            format!("> {} <", app.edit_buffer)
        } else {
            value.clone()
        };
        Row::new(vec![
            Cell::from(*label).style(style),
            Cell::from(display_value).style(style),
        ])
    });

    let table = Table::new(rows, [Constraint::Length(16), Constraint::Min(18)])
        .block(Block::default().borders(Borders::ALL).title(" Fields "))
        .row_highlight_style(
            Style::default()
                .fg(COLOR_SELECTION_FG)
                .bg(COLOR_SELECTION_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" ");
    f.render_widget(table, body[1]);

    if fields[current_field_idx].2 {
        let options = match current_field_idx {
            4 | 5 => vec!["No", "Yes"],
            6 => vec!["FM", "AM", "USB"],
            7 => vec!["Wide", "Narrow"],
            _ => vec![],
        };
        render_option_popup(f, area, &options, app.selection_index);
    }

    f.render_widget(
        Paragraph::new("↑/↓/Tab: Field | ←/→: Change Option | Enter: Save | Esc: Cancel")
            .style(Style::default().fg(COLOR_DIM))
            .alignment(ratatui::layout::Alignment::Center),
        vertical[2],
    );
}

fn bandplan_field_help(field_idx: usize) -> (&'static str, &'static str) {
    match field_idx {
        0 => ("Index", "Band plan slot number stored on the radio."),
        1 => ("Start Frequency", "Lower bound of the band plan in MHz."),
        2 => ("End Frequency", "Upper bound of the band plan in MHz."),
        3 => (
            "Max Power",
            "Maximum transmit power allowed for this band plan.",
        ),
        4 => (
            "TX Allowed",
            "Enable or block transmit inside this frequency range.",
        ),
        5 => (
            "Wrap",
            "Controls whether tuning wraps at the edge of the plan.",
        ),
        6 => (
            "Modulation",
            "Choose the default modulation for this range.",
        ),
        7 => ("Bandwidth", "Choose wide or narrow bandwidth for the plan."),
        _ => ("Field", ""),
    }
}
