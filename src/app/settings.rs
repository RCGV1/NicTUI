use crossterm::event::KeyCode;

use super::navigation::{next_item, prev_item};
use super::{App, AppMode, MainTab};
use crate::protocol::setting_value::{edit_value, parse_numeric_value, validate_raw_value};
use crate::protocol::{SETTINGS_METADATA, SettingType};

impl App {
    pub fn next_setting(&mut self) {
        if self.settings.is_some() {
            next_item(SETTINGS_METADATA.len(), &mut self.settings_state);
        }
    }

    pub fn prev_setting(&mut self) {
        if self.settings.is_some() {
            prev_item(SETTINGS_METADATA.len(), &mut self.settings_state);
        }
    }

    pub fn start_edit_setting(&mut self) {
        if let Some(i) = self.settings_state.selected() {
            self.mode = AppMode::EditSetting(i);
            self.status_message = format!("Editing {}", SETTINGS_METADATA[i].name);
            if let Some(s) = &self.settings {
                let meta = &SETTINGS_METADATA[i];
                match meta.setting_type {
                    SettingType::Enum(_) | SettingType::Boolean => {
                        self.selection_index = s.get_value(i) as usize;
                    }
                    _ => {}
                }
            }
            self.update_setting_edit_buffer();
        }
    }

    pub fn update_setting_edit_buffer(&mut self) {
        if let AppMode::EditSetting(idx) = self.mode
            && let Some(s) = &self.settings
        {
            self.edit_buffer = edit_value(idx, s.get_value(idx));
        }
    }

    pub fn commit_setting_edit(&mut self) {
        if let AppMode::EditSetting(idx) = self.mode {
            if let Some(s) = &mut self.settings {
                let meta = &SETTINGS_METADATA[idx];
                let current = s.get_value(idx);
                let candidate = match meta.setting_type {
                    SettingType::Enum(_) | SettingType::Boolean => {
                        u32::try_from(self.selection_index)
                            .map_err(|_| "Invalid choice".to_string())
                    }
                    SettingType::Numeric { .. } => {
                        // Preserve an untouched out-of-range value from another firmware.
                        if self.edit_buffer.trim() == edit_value(idx, current) {
                            Ok(current)
                        } else {
                            parse_numeric_value(idx, &self.edit_buffer)
                        }
                    }
                };
                let value = match candidate {
                    Ok(value) => value,
                    Err(error) => {
                        self.status_message = format!("Invalid setting: {error}");
                        return;
                    }
                };
                if value != current {
                    if let Err(error) = validate_raw_value(idx, value) {
                        self.status_message = format!("Invalid setting: {error}");
                        return;
                    }
                    s.set_value(idx, value);
                    self.settings_dirty = true;
                    self.status_message = "Setting changed (Unsaved)".to_string();
                } else {
                    self.status_message = "Setting unchanged".to_string();
                }
            }
            self.mode = AppMode::Main(MainTab::Settings);
        }
    }

    pub fn handle_settings_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => self.mode = AppMode::Main(MainTab::Settings),
            KeyCode::Enter => self.commit_setting_edit(),
            KeyCode::Down => self.select_next_setting_option(),
            KeyCode::Up => self.select_previous_setting_option(),
            KeyCode::Char(c) => {
                if self.active_setting_is_numeric() {
                    self.edit_buffer.push(c);
                }
            }
            KeyCode::Backspace if self.active_setting_is_numeric() => {
                self.edit_buffer.pop();
            }
            _ => {}
        }
    }

    fn active_setting_is_numeric(&self) -> bool {
        let AppMode::EditSetting(idx) = self.mode else {
            return false;
        };

        matches!(
            SETTINGS_METADATA[idx].setting_type,
            SettingType::Numeric { .. }
        )
    }

    fn select_next_setting_option(&mut self) {
        let Some(option_count) = self.active_setting_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + 1) % option_count;
    }

    fn select_previous_setting_option(&mut self) {
        let Some(option_count) = self.active_setting_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + option_count - 1) % option_count;
    }

    fn active_setting_option_count(&self) -> Option<usize> {
        let AppMode::EditSetting(idx) = self.mode else {
            return None;
        };

        match SETTINGS_METADATA[idx].setting_type {
            SettingType::Boolean => Some(2),
            SettingType::Enum(options) => Some(options.len()),
            SettingType::Numeric { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SettingsBlock;

    #[test]
    fn backspace_only_changes_numeric_settings() {
        for (index, metadata) in SETTINGS_METADATA.iter().enumerate() {
            let mut app = App::new();
            app.mode = AppMode::EditSetting(index);
            app.edit_buffer = "12".to_string();
            app.selection_index = 1;

            app.handle_settings_editor_key(KeyCode::Backspace);

            let expected = if matches!(metadata.setting_type, SettingType::Numeric { .. }) {
                "1"
            } else {
                "12"
            };
            assert_eq!(app.edit_buffer, expected);
            assert_eq!(app.selection_index, 1);
            assert_eq!(app.mode, AppMode::EditSetting(index));
        }
    }

    #[test]
    fn settings_editor_down_moves_forward_through_enum_options() {
        let mut app = App::new();
        app.settings = Some(SettingsBlock::default());
        app.mode = AppMode::EditSetting(3);
        app.selection_index = 0;

        app.handle_settings_editor_key(KeyCode::Down);

        assert_eq!(app.selection_index, 1);
    }

    #[test]
    fn settings_editor_up_wraps_backward_through_enum_options() {
        let mut app = App::new();
        app.settings = Some(SettingsBlock::default());
        app.mode = AppMode::EditSetting(3);
        app.selection_index = 0;

        app.handle_settings_editor_key(KeyCode::Up);

        assert_eq!(app.selection_index, 1);
    }

    fn editing(index: usize, raw: u32) -> App {
        let mut app = App::new();
        let mut settings = SettingsBlock::default();
        settings.set_value(index, raw);
        app.settings = Some(settings);
        app.settings_state.select(Some(index));
        app.start_edit_setting();
        app
    }

    #[test]
    fn settings_editor_converts_physical_units_before_commit() {
        let mut app = editing(4, 500);
        assert_eq!(app.edit_buffer, "5.00");
        app.edit_buffer = "12.5".to_string();

        app.commit_setting_edit();

        assert_eq!(app.settings.as_ref().unwrap().step, 1250);
        assert!(app.settings_dirty);
        assert!(matches!(app.mode, AppMode::Main(MainTab::Settings)));
    }

    #[test]
    fn invalid_setting_stays_open_without_mutating_or_marking_dirty() {
        for (index, raw, input) in [
            (0, 4, "256"),
            (4, 500, "0.001"),
            (13, 20, "bad"),
            (19, 5, "31"),
        ] {
            let mut app = editing(index, raw);
            app.edit_buffer = input.to_string();

            app.commit_setting_edit();

            assert_eq!(app.settings.as_ref().unwrap().get_value(index), raw);
            assert!(!app.settings_dirty);
            assert!(matches!(app.mode, AppMode::EditSetting(i) if i == index));
            assert!(app.status_message.starts_with("Invalid setting:"));
        }
    }

    #[test]
    fn unchanged_setting_does_not_create_or_clear_dirty_state() {
        let mut app = editing(13, 200);
        app.commit_setting_edit();
        assert!(!app.settings_dirty);

        let mut app = editing(19, 5);
        app.settings_dirty = true;
        app.commit_setting_edit();
        assert!(app.settings_dirty);
        assert_eq!(app.settings.as_ref().unwrap().breathe, 5);
    }

    #[test]
    fn untouched_unknown_enum_is_preserved_but_new_invalid_code_is_rejected() {
        let mut app = editing(3, 9);
        app.commit_setting_edit();
        assert_eq!(app.settings.as_ref().unwrap().active_vfo, 9);
        assert!(!app.settings_dirty);

        let mut app = editing(3, 0);
        app.selection_index = 9;
        app.commit_setting_edit();
        assert_eq!(app.settings.as_ref().unwrap().active_vfo, 0);
        assert!(!app.settings_dirty);
        assert!(matches!(app.mode, AppMode::EditSetting(3)));
    }

    #[test]
    fn cancel_does_not_apply_the_edit() {
        let mut app = editing(19, 5);
        app.edit_buffer = "10".to_string();
        app.handle_settings_editor_key(KeyCode::Esc);
        assert_eq!(app.settings.as_ref().unwrap().breathe, 5);
        assert!(!app.settings_dirty);
    }
}
