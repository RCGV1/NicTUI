use crate::app::App;
use crate::protocol::{SETTINGS_METADATA, SettingMetadata, SettingType, SettingsBlock};
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

pub fn render_settings_table(f: &mut Frame, app: &mut App, area: Rect) {
    let settings = match &app.settings {
        Some(s) => s,
        None => {
            render_ready_state(
                f,
                area,
                ReadyStateContent {
                    outer_title: "Settings",
                    card_title: "Ready To Load",
                    heading: "No Settings Loaded",
                    description: "Read the radio to load settings into the workspace.".to_string(),
                    note: None,
                },
                &[ReadyStateAction {
                    key: "r",
                    label: "read radio",
                }],
            );
            return;
        }
    };

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

    let enabled_toggles = SETTINGS_METADATA
        .iter()
        .enumerate()
        .filter(|(index, meta)| {
            matches!(meta.setting_type, SettingType::Boolean) && settings.get_value(*index) != 0
        })
        .count();
    let numeric_count = SETTINGS_METADATA
        .iter()
        .filter(|meta| matches!(meta.setting_type, SettingType::Numeric { .. }))
        .count();
    let enum_count = SETTINGS_METADATA
        .iter()
        .filter(|meta| matches!(meta.setting_type, SettingType::Enum(_)))
        .count();
    let selected_label = app
        .settings_state
        .selected()
        .map(|index| format!("M{}", SETTINGS_METADATA[index].menu_num))
        .unwrap_or_else(|| "--".to_string());

    let summary_text = if area.width < 90 {
        format!(
            "{} set | {} on | {} rng | {} opt | {}",
            SETTINGS_METADATA.len(),
            enabled_toggles,
            numeric_count,
            enum_count,
            selected_label
        )
    } else {
        format!(
            "{} fields | {} toggles on | {} numeric | {} choice | focus {}",
            SETTINGS_METADATA.len(),
            enabled_toggles,
            numeric_count,
            enum_count,
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

    let rows = SETTINGS_METADATA.iter().enumerate().map(|(i, meta)| {
        let display_val = settings.get_display_value(i);
        let style = if Some(i) == app.settings_state.selected() {
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
            Cell::from(meta.menu_num),
            Cell::from(meta.name),
            Cell::from(setting_kind(meta)),
            Cell::from(display_val),
        ])
        .style(style)
    });

    let (title, border_style) = if app.settings_dirty {
        (
            " RADIO SETTINGS (UNSAVED) ",
            Style::default().fg(COLOR_WARNING),
        )
    } else {
        (" RADIO SETTINGS ", Style::default().fg(COLOR_BORDER))
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(24),
            Constraint::Length(10),
            Constraint::Min(18),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("M#").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Setting").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Type").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Value").style(Style::default().fg(COLOR_ACCENT)),
        ])
        .style(Style::default().bg(COLOR_SURFACE_2))
        .height(1),
    )
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

    f.render_stateful_widget(table, chunks[1], &mut app.settings_state);

    let detail = Paragraph::new(selected_setting_lines(app, settings, area.width < 90))
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

fn setting_kind(meta: &SettingMetadata) -> &'static str {
    match meta.setting_type {
        SettingType::Boolean => "Toggle",
        SettingType::Enum(_) => "Choice",
        SettingType::Numeric { .. } => "Range",
    }
}

fn selected_setting_lines(
    app: &App,
    settings: &SettingsBlock,
    compact: bool,
) -> Vec<Line<'static>> {
    let Some(index) = app.settings_state.selected() else {
        return vec![
            Line::from("No setting selected"),
            Line::from(vec![
                render_shortcut("↑/↓"),
                Span::raw(": Move | "),
                render_shortcut("Enter"),
                Span::raw(": Edit | "),
                render_shortcut("w"),
                Span::raw(": Write | "),
                render_shortcut("r"),
                Span::raw(": Read"),
            ]),
        ];
    };

    let meta = &SETTINGS_METADATA[index];
    let display_value = settings.get_display_value(index);
    let raw_value = settings.get_value(index);
    let detail = match meta.setting_type {
        SettingType::Boolean => "Options Off / On".to_string(),
        SettingType::Enum(options) => format!("Options {}", options.join(" / ")),
        SettingType::Numeric { min, max, unit } => {
            if unit.is_empty() {
                format!("Range {}-{}", min, max)
            } else {
                format!("Range {}-{} {}", min, max, unit)
            }
        }
    };

    vec![
        Line::from(vec![
            Span::styled(
                format!(" M{} ", meta.menu_num),
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} ", meta.name)),
        ]),
        Line::from(vec![
            Span::styled(
                display_value,
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" | "),
            Span::styled(format!("raw {raw_value}"), Style::default().fg(COLOR_DIM)),
        ]),
        Line::from(if compact {
            vec![
                Span::styled(
                    setting_kind(meta),
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" | {}", compact_detail(&detail))),
            ]
        } else {
            vec![
                Span::styled(
                    setting_kind(meta),
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" | {}", detail)),
            ]
        }),
        Line::from(vec![
            render_shortcut("Enter"),
            Span::raw(": Edit | "),
            render_shortcut("↑/↓"),
            Span::raw(": Move | "),
            render_shortcut("r"),
            Span::raw(": Read | "),
            render_shortcut("w"),
            Span::raw(": Write"),
        ]),
    ]
}

fn compact_detail(detail: &str) -> String {
    detail
        .replace("Options ", "Opts ")
        .replace("Range ", "Rng ")
}
