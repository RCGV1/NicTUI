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

const BLUETOOTH_SETTING_INDEX: usize = 30;

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
    let bluetooth_summary = if settings.get_value(BLUETOOTH_SETTING_INDEX) == 0 {
        "BLE disabled"
    } else {
        "BLE enabled"
    };

    let summary_text = if area.width < 90 {
        format!(
            "{} fields | {} toggles on | {} numeric | {} choices | {} | {}",
            SETTINGS_METADATA.len(),
            enabled_toggles,
            numeric_count,
            enum_count,
            selected_label,
            bluetooth_summary
        )
    } else {
        format!(
            "{} fields | {} toggles on | {} numeric | {} choices | focus {} | {}",
            SETTINGS_METADATA.len(),
            enabled_toggles,
            numeric_count,
            enum_count,
            selected_label,
            bluetooth_summary
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
        let display_val = truncate_line(&settings.get_display_value(i), 24);
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
    let detail = match &meta.setting_type {
        SettingType::Boolean => "Allowed: Off or On".to_string(),
        SettingType::Enum(options) => format!("Allowed: {}", options.join(" / ")),
        SettingType::Numeric { min, max, unit } => {
            if unit.is_empty() {
                format!("Allowed: {}-{}", min, max)
            } else {
                format!("Allowed: {}-{} {}", min, max, unit)
            }
        }
    };
    let bluetooth_hint = if index == BLUETOOTH_SETTING_INDEX {
        Some(if raw_value == 0 {
            "BLE is disabled. Turn this on before using a phone app or NicTUI over BLE."
        } else {
            "BLE is enabled. You can scan from the connection screen."
        })
    } else {
        None
    };

    let mut lines = vec![
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
            Span::styled("Current: ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                display_value.clone(),
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(setting_storage_hint(meta, raw_value, &display_value)),
        ]),
        Line::from(if compact {
            vec![
                Span::styled(
                    setting_kind(meta),
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    " | {}",
                    truncate_line(&compact_detail(&detail), 56)
                )),
            ]
        } else {
            vec![
                Span::styled(
                    setting_kind(meta),
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" | {}", truncate_line(&detail, 72))),
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
    ];

    if let Some(bluetooth_hint) = bluetooth_hint {
        lines.push(Line::from(vec![
            Span::styled(
                " BLE ",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(bluetooth_hint),
        ]));
    }

    lines
}

fn compact_detail(detail: &str) -> String {
    detail.replace("Allowed: ", "Allowed ").replace(" or ", "/")
}

fn setting_storage_hint(meta: &SettingMetadata, raw_value: u32, display_value: &str) -> String {
    match &meta.setting_type {
        SettingType::Enum(options) if raw_value as usize >= options.len() => {
            format!(" | Stored code {raw_value}")
        }
        SettingType::Boolean if raw_value > 1 => format!(" | Stored code {raw_value}"),
        SettingType::Numeric { unit, .. }
            if !unit.is_empty() && display_value != raw_value.to_string() =>
        {
            format!(" | Stored value {raw_value}")
        }
        _ => String::new(),
    }
}

fn truncate_line(text: &str, max_len: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_len {
        text.to_string()
    } else if max_len <= 3 {
        text.chars().take(max_len).collect()
    } else {
        let mut shortened = text
            .chars()
            .take(max_len.saturating_sub(3))
            .collect::<String>();
        shortened.push_str("...");
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_storage_hint_hides_redundant_raw_values() {
        let meta = SettingMetadata {
            menu_num: "00",
            name: "Squelch",
            setting_type: SettingType::Numeric {
                min: 0,
                max: 9,
                unit: "",
            },
        };

        assert_eq!(setting_storage_hint(&meta, 4, "4"), "");
    }

    #[test]
    fn setting_storage_hint_explains_unknown_choice_codes() {
        let meta = SettingMetadata {
            menu_num: "03",
            name: "Active VFO",
            setting_type: SettingType::Enum(&["VFO A", "VFO B"]),
        };

        assert_eq!(
            setting_storage_hint(&meta, 4, "Unknown (4)"),
            " | Stored code 4"
        );
    }

    #[test]
    fn truncate_line_adds_ascii_ellipsis() {
        assert_eq!(truncate_line("Off / RX / TX / Both", 12), "Off / RX ...");
    }
}
