use crate::app::App;
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_SELECTION_BG, COLOR_SELECTION_FG, COLOR_SURFACE_1,
    COLOR_SURFACE_2, COLOR_TEXT,
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

pub fn render_bandplan(f: &mut Frame, app: &mut App, area: Rect) {
    if app.band_plans.is_empty() {
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "Band Plan",
                card_title: "Ready To Load",
                heading: "No Band Plan Loaded",
                description: "Read the radio to load the current band plan ranges.".to_string(),
                note: None,
            },
            &[ReadyStateAction {
                key: "r",
                label: "read radio",
            }],
        );
        return;
    }

    let tx_enabled = app.band_plans.iter().filter(|plan| plan.tx_allowed).count();
    let focused_label = app
        .bandplan_state
        .selected()
        .and_then(|index| app.band_plans.get(index))
        .map(|plan| format!("#{}", plan.index))
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
            "{} plans | {} TX | {}",
            app.band_plans.len(),
            tx_enabled,
            focused_label
        )
    } else {
        format!(
            "{} plans | {} TX enabled | focus {}",
            app.band_plans.len(),
            tx_enabled,
            focused_label
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
        Cell::from("Start").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("End").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("Power").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("TX").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("Mod").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("BW").style(Style::default().fg(COLOR_ACCENT)),
    ])
    .style(Style::default().bg(COLOR_SURFACE_2))
    .height(1);

    let rows = app.band_plans.iter().enumerate().map(|(i, bp)| {
        let is_selected = Some(i) == app.bandplan_state.selected();

        let style = if is_selected {
            Style::default()
                .fg(COLOR_SELECTION_FG)
                .bg(COLOR_SELECTION_BG)
                .add_modifier(Modifier::BOLD)
        } else if i % 2 == 0 {
            Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1)
        } else {
            Style::default().fg(COLOR_TEXT)
        };
        Row::new(vec![
            bp.index.to_string(),
            format!("{:.5}", bp.start_freq as f64 / 100000.0),
            format!("{:.5}", bp.end_freq as f64 / 100000.0),
            bp.max_power.to_string(),
            if bp.tx_allowed {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
            match bp.modulation {
                1 => "AM".to_string(),
                2 => "USB".to_string(),
                _ => "FM".to_string(),
            },
            if bp.bandwidth == 1 {
                "Narrow".to_string()
            } else {
                "Wide".to_string()
            },
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" BAND PLAN ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    )
    .row_highlight_style(
        Style::default()
            .fg(COLOR_SELECTION_FG)
            .bg(COLOR_SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[1], &mut app.bandplan_state);

    let compact = area.width < 90;
    let detail_lines = app
        .bandplan_state
        .selected()
        .and_then(|index| app.band_plans.get(index))
        .map(|plan| {
            vec![
                Line::from(vec![
                    Span::styled(
                        format!(" #{} ", plan.index),
                        Style::default()
                            .fg(COLOR_SELECTION_FG)
                            .bg(COLOR_SELECTION_BG)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        " {:.5} - {:.5}",
                        plan.start_freq as f64 / 100000.0,
                        plan.end_freq as f64 / 100000.0,
                    )),
                ]),
                Line::from(vec![Span::raw(format!(
                    "{} {} | {} {} {}",
                    if compact { "Pwr" } else { "Power" },
                    plan.max_power,
                    if plan.tx_allowed { "TX on" } else { "TX off" },
                    match plan.modulation {
                        1 => "AM",
                        2 => "USB",
                        _ => "FM",
                    },
                    if plan.bandwidth == 1 {
                        "Narrow"
                    } else {
                        "Wide"
                    }
                ))]),
                Line::from(vec![
                    render_shortcut("↑/↓"),
                    Span::raw(": Nav | "),
                    render_shortcut("Enter"),
                    Span::raw(": Edit | "),
                    render_shortcut("r"),
                    Span::raw(": Read | "),
                    render_shortcut("w"),
                    Span::raw(": Write"),
                ]),
            ]
        })
        .unwrap_or_else(|| vec![Line::from("No band plan selected")]);

    let help = Paragraph::new(detail_lines)
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Focus ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(help, chunks[2]);
}
