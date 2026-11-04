use super::common::{SETTINGS_EDITOR_HEIGHT, SETTINGS_EDITOR_WIDTH, begin_editor};
use crate::app::{App, AppMode};
use crate::protocol::setting_value::allowed_values;
use crate::protocol::{SETTINGS_METADATA, SettingType};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Cell, Paragraph, Row, Table, TableState},
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
            SettingType::Numeric { .. } => {
                let numeric_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(4),
                        Constraint::Length(4),
                        Constraint::Min(1),
                    ])
                    .split(chunks[0]);
                let help_text = format!("Range {}", allowed_values(idx));
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
                    Paragraph::new("Enter a value in the units shown above.\nEnter saves; invalid input stays open.")
                        .wrap(ratatui::widgets::Wrap { trim: true })
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
                let mut state = TableState::default();
                if app.selection_index < options.len() {
                    state.select(Some(app.selection_index));
                }
                f.render_stateful_widget(table, option_chunks[1], &mut state);
            }
        }

        let error = app.status_message.starts_with("Invalid setting:");
        f.render_widget(
            Paragraph::new(if error {
                app.status_message.as_str()
            } else {
                "↑/↓ change | Enter save | Esc cancel"
            })
            .style(Style::default().fg(if error { COLOR_WARNING } else { COLOR_DIM }))
            .alignment(ratatui::layout::Alignment::Center),
            chunks[1],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SettingsBlock;
    use ratatui::{Terminal, backend::TestBackend};

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 35)).unwrap();
        terminal
            .draw(|frame| render_settings_editor(frame, app))
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
    fn editor_displays_physical_values_and_ranges() {
        let mut app = App::new();
        app.settings = Some(SettingsBlock {
            scan_persist: 200,
            ..SettingsBlock::default()
        });
        app.mode = AppMode::EditSetting(13);
        app.edit_buffer = "20.0".to_string();
        let output = render(&app);
        assert!(output.contains("Current: 20.0 s"));
        assert!(output.contains("Range 0.0-20.0 s"));
        assert!(!output.contains("2000.1s"));
    }

    #[test]
    fn ninth_filter_option_scrolls_into_view() {
        let mut app = App::new();
        app.settings = Some(SettingsBlock::default());
        app.mode = AppMode::EditSetting(50);
        app.selection_index = 8;
        assert!(render(&app).contains("> FSK Mode"));
    }

    #[test]
    fn invalid_input_error_is_visible_inside_editor() {
        let mut app = App::new();
        app.settings = Some(SettingsBlock::default());
        app.mode = AppMode::EditSetting(0);
        app.status_message = "Invalid setting: Squelch accepts 0-9".to_string();
        assert!(render(&app).contains("Invalid setting: Squelch accepts 0-9"));
    }
}
