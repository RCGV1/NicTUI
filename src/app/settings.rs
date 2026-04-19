use crossterm::event::KeyCode;

use super::navigation::{next_item, prev_item};
use super::{App, AppMode, MainTab};
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
            self.edit_buffer = s.get_value(idx).to_string();
        }
    }

    pub fn commit_setting_edit(&mut self) {
        if let AppMode::EditSetting(idx) = self.mode {
            if let Some(s) = &mut self.settings {
                let meta = &SETTINGS_METADATA[idx];
                match meta.setting_type {
                    SettingType::Enum(_) | SettingType::Boolean => {
                        s.set_value(idx, self.selection_index as u32);
                    }
                    _ => {
                        if let Ok(val) = self.edit_buffer.parse::<u32>() {
                            s.set_value(idx, val);
                        }
                    }
                }
                self.settings_dirty = true;
                self.status_message = "Setting changed (Unsaved)".to_string();
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
            KeyCode::Backspace => {
                if self.active_setting_is_numeric() {
                    self.edit_buffer.pop();
                }
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
}
