use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};

impl App {
    pub fn handle_scan_preset_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.editing_scan_preset = None;
                self.mode = AppMode::Main(MainTab::Scanning);
            }
            KeyCode::Enter => self.commit_scan_preset_edit(),
            KeyCode::Up | KeyCode::BackTab => self.select_previous_scan_preset_field(),
            KeyCode::Down | KeyCode::Tab => self.select_next_scan_preset_field(),
            KeyCode::Left => self.select_previous_scan_preset_option(),
            KeyCode::Right => self.select_next_scan_preset_option(),
            KeyCode::Char(c) => {
                if self.active_scan_preset_is_numeric_or_text() {
                    self.edit_buffer.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.active_scan_preset_is_numeric_or_text() {
                    self.edit_buffer.pop();
                }
            }
            _ => {}
        }
    }

    fn select_next_scan_preset_field(&mut self) {
        let AppMode::EditScanPreset(current_idx) = self.mode else {
            return;
        };
        self.save_current_scan_preset_field(current_idx);
        self.mode = AppMode::EditScanPreset((current_idx + 1) % 8);
        self.update_scan_preset_edit_buffer();
    }

    fn select_previous_scan_preset_field(&mut self) {
        let AppMode::EditScanPreset(current_idx) = self.mode else {
            return;
        };
        self.save_current_scan_preset_field(current_idx);
        let new_idx = if current_idx == 0 { 7 } else { current_idx - 1 };
        self.mode = AppMode::EditScanPreset(new_idx);
        self.update_scan_preset_edit_buffer();
    }

    fn active_scan_preset_is_numeric_or_text(&self) -> bool {
        let AppMode::EditScanPreset(idx) = self.mode else {
            return false;
        };
        idx <= 5
    }

    fn select_next_scan_preset_option(&mut self) {
        let Some(option_count) = self.active_scan_preset_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + 1) % option_count;
        self.update_scan_preset_edit_buffer();
    }

    fn select_previous_scan_preset_option(&mut self) {
        let Some(option_count) = self.active_scan_preset_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + option_count - 1) % option_count;
        self.update_scan_preset_edit_buffer();
    }

    fn active_scan_preset_option_count(&self) -> Option<usize> {
        let AppMode::EditScanPreset(idx) = self.mode else {
            return None;
        };

        match idx {
            6 | 7 => Some(3),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ScanPreset;

    #[test]
    fn scan_preset_modulation_cycles_three_values() {
        let mut app = App::new();
        app.editing_scan_preset = Some(ScanPreset::default());
        app.mode = AppMode::EditScanPreset(6);
        app.selection_index = 2;

        app.handle_scan_preset_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }

    #[test]
    fn scan_preset_ultrascan_cycles_three_values() {
        let mut app = App::new();
        app.editing_scan_preset = Some(ScanPreset::default());
        app.mode = AppMode::EditScanPreset(7);
        app.selection_index = 2;

        app.handle_scan_preset_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }

    #[test]
    fn scan_preset_option_fields_ignore_text_input() {
        let mut app = App::new();
        app.editing_scan_preset = Some(ScanPreset::default());
        app.mode = AppMode::EditScanPreset(6);
        app.edit_buffer = "FM".to_string();

        app.handle_scan_preset_editor_key(KeyCode::Char('x'));

        assert_eq!(app.edit_buffer, "FM");
    }
}
