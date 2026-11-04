use super::common::{
    SCAN_PRESET_EDITOR_HEIGHT, SCAN_PRESET_EDITOR_WIDTH, begin_editor, render_option_popup,
};
use crate::app::{App, AppMode};
use crate::protocol::editor_value::{SCAN_MODULATIONS, decimal_value, option_label};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
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
    if inner_area.width < 2 || inner_area.height < 2 {
        return;
    }

    let current_field_idx = if let AppMode::EditScanPreset(idx) = app.mode {
        idx
    } else {
        0
    };

    let fields = [
        ("Label", sp.label.clone(), false),
        ("Start (MHz)", decimal_value(sp.start_freq, 5), false),
        ("Range (MHz)", decimal_value(sp.range as u32, 2), false),
        ("Step (kHz)", decimal_value(sp.step as u32, 2), false),
        ("Persist (s)", decimal_value(sp.persist as u32, 1), false),
        ("Resume (s)", sp.resume.to_string(), false),
        (
            "Modulation",
            option_label(SCAN_MODULATIONS, sp.modulation),
            true,
        ),
        ("Ultrascan (raw)", sp.ultrascan.to_string(), false),
    ];
    let current_value = &fields[current_field_idx].1;
    let (help_title, help_body) = scan_field_help(current_field_idx);

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
            Span::raw(decimal_value(sp.start_freq, 5)),
            Span::raw(" | "),
            Span::styled("Range ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{} MHz", decimal_value(sp.range as u32, 2))),
            Span::raw(" | "),
            Span::styled("Step ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{} kHz", decimal_value(sp.step as u32, 2))),
        ]),
        Line::from(vec![
            Span::styled("Resume ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{}s", sp.resume)),
            Span::raw(" | "),
            Span::styled("Persist ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{}s", decimal_value(sp.persist as u32, 1))),
            Span::raw(" | "),
            Span::styled("Mode ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(option_label(SCAN_MODULATIONS, sp.modulation)),
            Span::raw(" | "),
            Span::styled("Ultra raw ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(sp.ultrascan.to_string()),
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

    let table = Table::new(rows, [Constraint::Length(18), Constraint::Min(18)])
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
            6 => SCAN_MODULATIONS.to_vec(),
            _ => vec![],
        };
        render_option_popup(f, area, &options, app.selection_index);
    }

    f.render_widget(
        Paragraph::new(if app.status_message.starts_with("Invalid scan:") {
            app.status_message.as_str()
        } else {
            "↑/↓/Tab: Field | ←/→: Option | Enter: Save | Esc: Cancel"
        })
        .style(
            Style::default().fg(if app.status_message.starts_with("Invalid scan:") {
                COLOR_WARNING
            } else {
                COLOR_DIM
            }),
        )
        .alignment(ratatui::layout::Alignment::Center),
        vertical[2],
    );
}

fn scan_field_help(field_idx: usize) -> (&'static str, &'static str) {
    match field_idx {
        0 => ("Preset Label", "Up to 8 printable ASCII characters."),
        1 => (
            "Start Frequency",
            "18-1300 MHz in 10 Hz increments. 0 disables this preset.",
        ),
        2 => (
            "Range",
            "Scan span: 0.01-600.00 MHz, in 0.01 MHz increments.",
        ),
        3 => ("Step", "0.01-500.00 kHz, in 0.01 kHz (10 Hz) increments."),
        4 => ("Persist", "Hold after signal loss: 0.0-20.0 seconds."),
        5 => (
            "Resume",
            "Resume while signal remains: 0-250 seconds; 0 disables.",
        ),
        6 => (
            "Modulation",
            "Choose the modulation mode used for this preset.",
        ),
        7 => (
            "Ultrascan",
            "Raw level 0-20; 0=Off. Type a value or use left/right.",
        ),
        _ => ("Field", ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{BandPlan, DTMFPreset, ScanPreset};
    use ratatui::{Terminal, backend::TestBackend};

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| match app.mode {
                AppMode::EditScanPreset(_) => render_scan_preset_editor(frame, app),
                AppMode::EditBandPlan(_) => crate::ui::editors::render_bandplan_editor(frame, app),
                AppMode::EditDTMF(_) => crate::ui::editors::render_dtmf_editor(frame, app),
                _ => unreachable!(),
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn editors_render_physical_units_canonical_digits_and_all_modes() {
        let mut app = App::new();
        app.editing_scan_preset = Some(ScanPreset {
            start_freq: 11_800_000,
            range: 1900,
            step: 1250,
            persist: 200,
            ultrascan: 20,
            ..ScanPreset::default()
        });
        app.mode = AppMode::EditScanPreset(3);
        app.update_scan_preset_edit_buffer();
        let text = render(&app, 100, 40);
        for expected in ["19.00 MHz", "12.50 kHz", "20.0s", "Ultra raw 20"] {
            assert!(text.contains(expected), "{expected}");
        }
        app.editing_band_plan = Some(BandPlan {
            modulation: 7,
            bandwidth: 5,
            max_power: 255,
            ..BandPlan::default()
        });
        app.mode = AppMode::EditBandPlan(6);
        app.update_bandplan_edit_buffer();
        let text = render(&app, 100, 40);
        for expected in ["Enforce None", "FM Tuner", "Power (raw)", "255"] {
            assert!(text.contains(expected), "{expected}");
        }
        app.pending_dtmf_edit = Some(DTMFPreset {
            label: "Test".to_string(),
            digits: vec![1, 12, 14, 15],
            ..DTMFPreset::default()
        });
        app.dtmf_edit_preset_idx = Some(0);
        app.mode = AppMode::EditDTMF(1);
        app.update_dtmf_edit_buffer();
        assert!(render(&app, 100, 40).contains("1C*#"));
        for mode in [
            AppMode::EditScanPreset(7),
            AppMode::EditBandPlan(7),
            AppMode::EditDTMF(1),
        ] {
            app.mode = mode;
            for (width, height) in [(80, 24), (40, 18), (12, 8), (1, 1)] {
                render(&app, width, height);
            }
        }
    }
}
