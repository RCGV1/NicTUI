use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};

impl App {
    pub fn handle_dtmf_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.dtmf_edit_preset_idx = None;
                self.mode = AppMode::Main(MainTab::DTMF);
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
        self.save_current_dtmf_field_to_pending(current_idx);
        self.mode = AppMode::EditDTMF((current_idx + 1) % 2);
        self.update_dtmf_edit_buffer();
    }

    fn select_previous_dtmf_field(&mut self) {
        let AppMode::EditDTMF(current_idx) = self.mode else {
            return;
        };
        self.save_current_dtmf_field_to_pending(current_idx);
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
}
