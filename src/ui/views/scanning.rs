use crate::app::App;
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG,
    COLOR_SURFACE_1, COLOR_SURFACE_2, COLOR_TEXT,
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

pub fn render_scanning_page(f: &mut Frame, app: &mut App, area: Rect) {
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
        format!("{} presets | Enter edit | 4 groups", app.scan_presets.len())
    } else {
        format!(
            "{} presets | Enter edits selected preset | 4 opens memory groups",
            app.scan_presets.len()
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

    render_preset_table(f, app, chunks[1], area.width < 90);

    let detail = Paragraph::new(selected_scan_preset_lines(app, area.width < 90))
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Focus ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(detail, chunks[2]);
}

fn render_preset_table(f: &mut Frame, app: &mut App, area: Rect, compact: bool) {
    let preset_border_style = Style::default().fg(COLOR_PRIMARY);

    if app.scan_presets.is_empty() {
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "Scan Presets",
                card_title: "Ready To Load",
                heading: "No Scan Presets Loaded",
                description: "Read the radio to load the current scan preset bank.".to_string(),
                note: None,
            },
            &[ReadyStateAction {
                key: "r",
                label: "read radio",
            }],
        );
        return;
    }

    let header = if compact {
        Row::new(vec![
            Cell::from("#").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Label").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Start").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Span").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Step").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Mod").style(Style::default().fg(COLOR_ACCENT)),
        ])
    } else {
        Row::new(vec![
            Cell::from("#").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Label").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Start").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Range").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Step").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Resume").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Mod").style(Style::default().fg(COLOR_ACCENT)),
        ])
    }
    .style(Style::default().bg(COLOR_SURFACE_2))
    .height(1);

    let rows = app.scan_presets.iter().enumerate().map(|(i, preset)| {
        let is_selected = Some(i) == app.preset_state.selected();
        let style = if is_selected {
            Style::default()
                .bg(COLOR_SELECTION_BG)
                .fg(COLOR_SELECTION_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_TEXT)
        };

        if compact {
            Row::new(vec![
                preset.index.to_string(),
                preset.label.clone(),
                format!("{:.5}", preset.start_freq as f64 / 100000.0),
                format!("{}M", preset.range),
                format!("{}Hz", preset.step),
                scan_modulation_label(preset.modulation).to_string(),
            ])
        } else {
            Row::new(vec![
                preset.index.to_string(),
                preset.label.clone(),
                format!("{:.5}", preset.start_freq as f64 / 100000.0),
                format!("{} MHz", preset.range),
                format!("{} Hz", preset.step),
                format!("{}s", preset.resume),
                scan_modulation_label(preset.modulation).to_string(),
            ])
        }
        .style(style)
    });

    let table = Table::new(
        rows,
        if compact {
            vec![
                Constraint::Length(3),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(4),
            ]
        } else {
            vec![
                Constraint::Length(4),
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(6),
            ]
        },
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" SCAN PRESETS ")
            .border_style(preset_border_style)
            .bg(COLOR_SURFACE_1),
    )
    .row_highlight_style(
        Style::default()
            .fg(COLOR_SELECTION_FG)
            .bg(COLOR_SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, area, &mut app.preset_state);
}

fn selected_scan_preset_lines(app: &App, compact: bool) -> Vec<Line<'static>> {
    let Some(index) = app.preset_state.selected() else {
        return vec![Line::from("No scan preset selected")];
    };
    let Some(preset) = app.scan_presets.get(index) else {
        return vec![Line::from("No scan preset selected")];
    };

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
                " {}",
                if preset.label.is_empty() {
                    "<unnamed>"
                } else {
                    preset.label.as_str()
                }
            )),
        ]),
        Line::from(if compact {
            format!(
                "{:.5} | {} MHz | {} Hz",
                preset.start_freq as f64 / 100000.0,
                preset.range,
                preset.step
            )
        } else {
            format!(
                "Start {:.5} | Range {} MHz | Step {} Hz",
                preset.start_freq as f64 / 100000.0,
                preset.range,
                preset.step
            )
        }),
        Line::from(if compact {
            format!(
                "Res {}s | Hold {}s | {} | U {}",
                preset.resume,
                preset.persist,
                scan_modulation_label(preset.modulation),
                preset.ultrascan
            )
        } else {
            format!(
                "Resume {}s | Persist {}s | {} | Ultrascan {}",
                preset.resume,
                preset.persist,
                scan_modulation_label(preset.modulation),
                preset.ultrascan
            )
        }),
        Line::from(vec![
            render_shortcut("Enter"),
            Span::raw(": Edit | "),
            render_shortcut("r"),
            Span::raw(": Read | "),
            render_shortcut("4"),
            Span::raw(": Memory groups"),
        ]),
    ]
}

fn scan_modulation_label(value: u8) -> &'static str {
    match value {
        1 => "AM",
        2 => "USB",
        _ => "FM",
    }
}
