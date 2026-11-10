use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};
use crate::device::validate_dtmf_presets_payload;
use crate::protocol::editor_value::{dtmf_text, parse_dtmf, validate_label};

impl App {
    pub fn update_dtmf_edit_buffer(&mut self) {
        if self.pending_dtmf_edit.is_none() {
            self.pending_dtmf_edit = self
                .dtmf_edit_preset_idx
                .and_then(|i| self.dtmf_presets.get(i))
                .cloned();
            self.status_message = "Editing DTMF preset".to_string();
        }
        if let AppMode::EditDTMF(field) = self.mode
            && let Some(preset) = &self.pending_dtmf_edit
        {
            self.edit_buffer = if field == 0 {
                preset.label.clone()
            } else {
                dtmf_text(&preset.digits)
            };
        }
    }

    pub fn save_current_dtmf_field_to_pending(&mut self, field: usize) -> bool {
        let Some(preset) = self.pending_dtmf_edit.as_mut() else {
            return false;
        };
        let result = (|| -> Result<(), String> {
            match field {
                0 => {
                    validate_label(&self.edit_buffer, 7)?;
                    preset.label = self.edit_buffer.clone();
                }
                1 => preset.digits = parse_dtmf(&self.edit_buffer)?,
                _ => return Err("Unknown DTMF field".to_string()),
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.status_message = format!("Invalid DTMF: {error}");
            false
        } else {
            if self.status_message.starts_with("Invalid DTMF:") {
                self.status_message = "Editing DTMF preset".to_string();
            }
            true
        }
    }

    pub fn commit_dtmf_edit(&mut self) {
        let AppMode::EditDTMF(field) = self.mode else {
            return;
        };
        if !self.save_current_dtmf_field_to_pending(field) {
            return;
        }
        let Some(preset) = self.pending_dtmf_edit.as_ref() else {
            return;
        };
        if let Err(error) = validate_dtmf_presets_payload(std::slice::from_ref(preset)) {
            self.status_message = format!("Invalid DTMF: {error}");
            return;
        }
        if let Some(slot) = self
            .dtmf_edit_preset_idx
            .and_then(|i| self.dtmf_presets.get_mut(i))
        {
            if slot.label != preset.label || slot.digits != preset.digits {
                *slot = preset.clone();
                self.dtmf_dirty = true;
                self.status_message = "DTMF preset changed (Unsaved)".to_string();
            } else {
                self.status_message = "DTMF preset unchanged".to_string();
            }
        }
        self.pending_dtmf_edit = None;
        self.dtmf_edit_preset_idx = None;
        self.mode = AppMode::Main(MainTab::DTMF);
    }

    pub fn handle_dtmf_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.dtmf_edit_preset_idx = None;
                self.pending_dtmf_edit = None;
                self.mode = AppMode::Main(MainTab::DTMF);
                self.status_message = "DTMF edit cancelled".into();
            }
            KeyCode::Enter => self.commit_dtmf_edit(),
            KeyCode::Up | KeyCode::BackTab => self.select_previous_dtmf_field(),
            KeyCode::Down | KeyCode::Tab => self.select_next_dtmf_field(),
            KeyCode::Char(c) => self.edit_buffer.push(c),
            KeyCode::Backspace => {
                self.edit_buffer.pop();
            }
            _ => {}
        }
    }

    fn select_next_dtmf_field(&mut self) {
        let AppMode::EditDTMF(current_idx) = self.mode else {
            return;
        };
        if !self.save_current_dtmf_field_to_pending(current_idx) {
            return;
        }
        self.mode = AppMode::EditDTMF((current_idx + 1) % 2);
        self.update_dtmf_edit_buffer();
    }

    fn select_previous_dtmf_field(&mut self) {
        let AppMode::EditDTMF(current_idx) = self.mode else {
            return;
        };
        if !self.save_current_dtmf_field_to_pending(current_idx) {
            return;
        }
        let new_idx = if current_idx == 0 { 1 } else { current_idx - 1 };
        self.mode = AppMode::EditDTMF(new_idx);
        self.update_dtmf_edit_buffer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtmf_editor_wraps_between_fields() {
        let mut app = App::new();
        app.pending_dtmf_edit = Some(crate::protocol::DTMFPreset::default());
        app.mode = AppMode::EditDTMF(0);

        app.handle_dtmf_editor_key(KeyCode::Up);

        assert!(matches!(app.mode, AppMode::EditDTMF(1)));
    }

    #[test]
    fn dtmf_editor_accepts_digit_input() {
        let mut app = App::new();
        app.mode = AppMode::EditDTMF(1);

        app.handle_dtmf_editor_key(KeyCode::Char('1'));
        app.handle_dtmf_editor_key(KeyCode::Char('A'));

        assert_eq!(app.edit_buffer, "1A");
    }

    fn editing(field: usize) -> App {
        let mut app = App::new();
        app.dtmf_presets.push(crate::protocol::DTMFPreset {
            index: 0,
            label: "Test".to_string(),
            digits: vec![1, 12, 14, 15],
        });
        app.dtmf_edit_preset_idx = Some(0);
        app.dtmf_state.select(Some(0));
        app.mode = AppMode::EditDTMF(field);
        app.update_dtmf_edit_buffer();
        app
    }

    #[test]
    fn dtmf_cancel_discards_all_fields_visited_and_noop_is_clean() {
        let mut app = editing(0);
        app.edit_buffer = "Changed".to_string();
        app.handle_dtmf_editor_key(KeyCode::Tab);
        assert_eq!(app.dtmf_presets[0].label, "Test");
        assert_eq!(app.edit_buffer, "1C*#");
        app.edit_buffer = "9*#".to_string();
        app.handle_dtmf_editor_key(KeyCode::BackTab);
        app.handle_dtmf_editor_key(KeyCode::Esc);
        assert_eq!(app.dtmf_presets[0].label, "Test");
        assert_eq!(app.dtmf_presets[0].digits, vec![1, 12, 14, 15]);
        assert!(!app.dtmf_dirty);
        assert!(app.pending_dtmf_edit.is_none());
        let mut app = editing(1);
        app.commit_dtmf_edit();
        assert!(!app.dtmf_dirty);
    }

    #[test]
    fn dtmf_commit_encodes_symbols_and_rejects_lossy_input() {
        let mut app = editing(1);
        app.edit_buffer = "aD*#".to_string();
        app.commit_dtmf_edit();
        assert_eq!(app.dtmf_presets[0].digits, vec![10, 13, 14, 15]);
        assert!(app.dtmf_dirty);
        for (field, input) in [(0, "12345678"), (0, "é"), (1, "E"), (1, "1234567890")] {
            let mut app = editing(field);
            app.edit_buffer = input.to_string();
            app.handle_dtmf_editor_key(KeyCode::Tab);
            assert!(matches!(app.mode, AppMode::EditDTMF(i) if i == field));
            app.commit_dtmf_edit();
            assert!(app.status_message.starts_with("Invalid DTMF:"));
            assert!(!app.dtmf_dirty);
            assert_eq!(app.dtmf_presets[0].digits, vec![1, 12, 14, 15]);
        }
    }
}
