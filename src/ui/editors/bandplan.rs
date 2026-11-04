use super::common::{
    BANDPLAN_EDITOR_HEIGHT, BANDPLAN_EDITOR_WIDTH, begin_editor, render_option_popup,
};
use crate::app::{App, AppMode};
use crate::protocol::editor_value::{
    BAND_BANDWIDTHS, BAND_MODULATIONS, decimal_value, option_label,
};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
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
    if inner_area.width < 2 || inner_area.height < 2 {
        return;
    }

    let current_field_idx = if let AppMode::EditBandPlan(idx) = app.mode {
        idx
    } else {
        0
    };

    let mod_str = option_label(BAND_MODULATIONS, bp.modulation);
    let bw_str = option_label(BAND_BANDWIDTHS, bp.bandwidth);

    let fields = [
        ("Index", bp.index.to_string(), false),
        ("Start (MHz)", decimal_value(bp.start_freq, 5), false),
        ("End (MHz)", decimal_value(bp.end_freq, 5), false),
        ("Power (raw)", bp.max_power.to_string(), false),
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
            Constraint::Length(5),
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
                " {}-{} MHz | {}",
                decimal_value(bp.start_freq, 5),
                decimal_value(bp.end_freq, 5),
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
            Span::raw(option_label(BAND_MODULATIONS, bp.modulation)),
            Span::raw(" | "),
            Span::styled("BW ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(option_label(BAND_BANDWIDTHS, bp.bandwidth)),
        ]),
        Line::from(vec![
            Span::styled("Power raw ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(if bp.max_power == 0 {
                "0 (Ignore)".to_string()
            } else {
                bp.max_power.to_string()
            }),
        ]),
    ])
    .style(Style::default().fg(COLOR_TEXT))
    .wrap(ratatui::widgets::Wrap { trim: true })
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
    ])
    .style(Style::default().fg(COLOR_TEXT))
    .wrap(ratatui::widgets::Wrap { trim: true })
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
    let mut state = TableState::default();
    state.select(Some(current_field_idx));
    f.render_stateful_widget(table, body[1], &mut state);

    if fields[current_field_idx].2 {
        let options = match current_field_idx {
            4 | 5 => vec!["No", "Yes"],
            6 => BAND_MODULATIONS.to_vec(),
            7 => BAND_BANDWIDTHS.to_vec(),
            _ => vec![],
        };
        render_option_popup(f, area, &options, app.selection_index);
    }

    f.render_widget(
        Paragraph::new(if app.status_message.starts_with("Invalid band:") {
            app.status_message.as_str()
        } else {
            "↑/↓/Tab: Field | ←/→: Option | Enter: Save | Esc: Cancel"
        })
        .style(
            Style::default().fg(if app.status_message.starts_with("Invalid band:") {
                COLOR_WARNING
            } else {
                COLOR_DIM
            }),
        )
        .alignment(ratatui::layout::Alignment::Center),
        vertical[2],
    );
}

fn bandplan_field_help(field_idx: usize) -> (&'static str, &'static str) {
    match field_idx {
        0 => ("Index", "Unique band plan slot 0-19."),
        1 => (
            "Start Frequency",
            "Lower bound: 0-1300 MHz, 10 Hz increments.",
        ),
        2 => (
            "End Frequency",
            "Greater than start; both 0 marks an unused plan.",
        ),
        3 => (
            "Max Power",
            "Raw power limit 0-255; 0=Ignore. This value is not watts.",
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
            "Ignore, FM/AM/USB defaults, or enforced modes.",
        ),
        7 => (
            "Bandwidth",
            "Ignore, Wide/Narrow, enforced widths, or FM Tuner.",
        ),
        _ => ("Field", ""),
    }
}
