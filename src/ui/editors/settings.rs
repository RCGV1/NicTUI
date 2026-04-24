use super::common::{SETTINGS_EDITOR_HEIGHT, SETTINGS_EDITOR_WIDTH, begin_editor};
use crate::app::{App, AppMode};
use crate::protocol::{SETTINGS_METADATA, SettingType};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Cell, Paragraph, Row, Table},
};

const BLUETOOTH_SETTING_INDEX: usize = 30;

pub(crate) fn render_settings_editor(f: &mut Frame, app: &App) {
    if let AppMode::EditSetting(idx) = app.mode {
        let (_, inner_area) = begin_editor(
            f,
            format!(
                " Setting M{}: {} ",
                SETTINGS_METADATA[idx].menu_num, SETTINGS_METADATA[idx].name
            ),
            SETTINGS_EDITOR_WIDTH,
            SETTINGS_EDITOR_HEIGHT,
        );

        let meta = &SETTINGS_METADATA[idx];
        let current_value = app
            .settings
            .as_ref()
            .map(|settings| settings.get_display_value(idx))
            .unwrap_or_else(|| "--".to_string());

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(13), Constraint::Length(1)])
            .split(inner_area);

        match meta.setting_type {
            SettingType::Numeric { min, max, unit } => {
                let numeric_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(4),
                        Constraint::Min(1),
                    ])
                    .split(chunks[0]);
                let help_text = if unit.is_empty() {
                    format!("Range {}-{}", min, max)
                } else {
                    format!("Range {}-{} {}", min, max, unit)
                };
                f.render_widget(
                    Paragraph::new(format!("Current: {}\n{}", current_value, help_text))
                        .style(Style::default().fg(COLOR_TEXT))
                        .block(
                            Block::default()
                                .borders(ratatui::widgets::Borders::BOTTOM)
                                .title(" Details "),
                        ),
                    numeric_chunks[0],
                );

                f.render_widget(
                    Paragraph::new(app.edit_buffer.as_str())
                        .block(
                            Block::default()
                                .borders(ratatui::widgets::Borders::ALL)
                                .title(" New Value "),
                        )
                        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_2)),
                    numeric_chunks[1],
                );

                f.render_widget(
                    Paragraph::new("Type a value, then press Enter to save it.")
                        .style(Style::default().fg(COLOR_DIM)),
                    numeric_chunks[2],
                );
            }
            SettingType::Boolean | SettingType::Enum(_) => {
                let options = match meta.setting_type {
                    SettingType::Boolean => vec!["Off", "On"],
                    SettingType::Enum(opts) => opts.to_vec(),
                    _ => unreachable!(),
                };
                let detail_text = if idx == BLUETOOTH_SETTING_INDEX {
                    format!(
                        "Current: {}\nUse ↑ for previous and ↓ for next. Turn this on to connect over BLE.",
                        current_value
                    )
                } else {
                    format!(
                        "Current: {}\nUse ↑ for previous and ↓ for next.",
                        current_value
                    )
                };
                let option_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(1)])
                    .split(chunks[0]);

                f.render_widget(
                    Paragraph::new(detail_text)
                        .style(Style::default().fg(COLOR_DIM))
                        .block(
                            Block::default()
                                .borders(ratatui::widgets::Borders::BOTTOM)
                                .title(" Details "),
                        ),
                    option_chunks[0],
                );

                let rows = options.iter().enumerate().map(|(i, opt)| {
                    let style = if i == app.selection_index {
                        Style::default()
                            .fg(COLOR_SELECTION_FG)
                            .bg(COLOR_SELECTION_BG)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(COLOR_TEXT)
                    };
                    let display_value = format!(
                        "{} {}",
                        if i == app.selection_index { ">" } else { " " },
                        opt
                    );
                    Row::new(vec![Cell::from(display_value).style(style)])
                });

                let table = Table::new(rows, [Constraint::Min(1)])
                    .block(
                        Block::default()
                            .borders(ratatui::widgets::Borders::ALL)
                            .title(" Options "),
                    )
                    .row_highlight_style(
                        Style::default()
                            .fg(COLOR_SELECTION_FG)
                            .bg(COLOR_SELECTION_BG)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol(" ");
                f.render_widget(table, option_chunks[1]);
            }
        }

        f.render_widget(
            Paragraph::new("↑/↓ change | Enter save | Esc cancel")
                .style(Style::default().fg(COLOR_DIM))
                .alignment(ratatui::layout::Alignment::Center),
            chunks[1],
        );
    }
}
