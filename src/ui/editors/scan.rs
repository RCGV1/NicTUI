use super::common::{
    SCAN_PRESET_EDITOR_HEIGHT, SCAN_PRESET_EDITOR_WIDTH, begin_editor, render_option_popup,
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

pub(crate) fn render_scan_preset_editor(f: &mut Frame, app: &App) {
    let Some(sp) = app.editing_scan_preset.as_ref() else {
        return;
    };

    let (area, inner_area) = begin_editor(
        f,
        format!(" Scan Preset {} ", sp.index),
        SCAN_PRESET_EDITOR_WIDTH,
        SCAN_PRESET_EDITOR_HEIGHT,
    );

    let current_field_idx = if let AppMode::EditScanPreset(idx) = app.mode {
        idx
    } else {
        0
    };

    let fields = [
        ("Label", sp.label.clone(), false),
        (
            "Start Freq",
            format!("{:.5}", sp.start_freq as f64 / 100000.0),
            false,
        ),
        ("Range (MHz)", sp.range.to_string(), false),
        ("Step (Hz)", sp.step.to_string(), false),
        ("Persist (s)", sp.persist.to_string(), false),
        ("Resume (s)", sp.resume.to_string(), false),
        (
            "Modulation",
            modulation_label(sp.modulation).to_string(),
            true,
        ),
        ("Ultrascan", ultrascan_label(sp.ultrascan).to_string(), true),
    ];
    let current_value = &fields[current_field_idx].1;
    let (help_title, help_body) = scan_field_help(current_field_idx);

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
                format!(" P{:02} ", sp.index),
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {} | {}",
                if sp.label.is_empty() {
                    "<unnamed>"
                } else {
                    sp.label.as_str()
                },
                fields[current_field_idx].0
            )),
        ]),
        Line::from(vec![
            Span::styled("Start ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{:.5}", sp.start_freq as f64 / 100000.0)),
            Span::raw(" | "),
            Span::styled("Range ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{} MHz", sp.range)),
            Span::raw(" | "),
            Span::styled("Step ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{} Hz", sp.step)),
        ]),
        Line::from(vec![
            Span::styled("Resume ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{}s", sp.resume)),
            Span::raw(" | "),
            Span::styled("Persist ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{}s", sp.persist)),
            Span::raw(" | "),
            Span::styled("Mode ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(modulation_label(sp.modulation)),
            Span::raw(" | "),
            Span::styled("Ultra ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(ultrascan_label(sp.ultrascan)),
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
            "Type to edit the value directly."
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

    let table = Table::new(rows, [Constraint::Length(18), Constraint::Min(18)])
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
            6 => vec!["FM", "AM", "USB"],
            7 => vec!["0", "1", "2"],
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

fn modulation_label(modulation: u8) -> &'static str {
    match modulation {
        1 => "AM",
        2 => "USB",
        _ => "FM",
    }
}

fn ultrascan_label(ultrascan: u8) -> &'static str {
    match ultrascan {
        1 => "1",
        2 => "2",
        _ => "0",
    }
}

fn scan_field_help(field_idx: usize) -> (&'static str, &'static str) {
    match field_idx {
        0 => (
            "Preset Label",
            "Short name shown in the scan preset list and summaries.",
        ),
        1 => (
            "Start Frequency",
            "Enter the scan start frequency in MHz, for example 118.00000.",
        ),
        2 => (
            "Range",
            "Scan span in MHz starting from the selected start frequency.",
        ),
        3 => (
            "Step",
            "Tuning step in Hz used while scanning through the range.",
        ),
        4 => (
            "Persist",
            "How long to stay on an active frequency before moving on.",
        ),
        5 => (
            "Resume",
            "Delay before the scan resumes after activity stops.",
        ),
        6 => (
            "Modulation",
            "Choose the modulation mode used for this preset.",
        ),
        7 => (
            "Ultrascan",
            "Select the radio's ultrascan tail value. The current firmware exposes 0, 1, and 2.",
        ),
        _ => ("Field", ""),
    }
}
