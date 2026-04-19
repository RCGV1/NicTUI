use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};
use crate::protocol::GROUP_LABEL_COUNT;

impl App {
    pub fn handle_channel_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.pending_channel_edit = None;
                self.mode = AppMode::Main(MainTab::Channels);
            }
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Up | KeyCode::BackTab => self.select_previous_channel_field(),
            KeyCode::Down | KeyCode::Tab => self.select_next_channel_field(),
            KeyCode::Left => self.select_previous_channel_option(),
            KeyCode::Right => self.select_next_channel_option(),
            KeyCode::Char(c) => {
                if self.active_channel_accepts_text_input() {
                    self.edit_buffer.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.active_channel_accepts_text_input() {
                    self.edit_buffer.pop();
                }
            }
            _ => {}
        }
    }

    fn select_next_channel_field(&mut self) {
        let AppMode::EditChannel(current_idx) = self.mode else {
            return;
        };
        self.save_current_field_to_pending(current_idx);
        self.mode = AppMode::EditChannel((current_idx + 1) % 11);
        self.update_edit_buffer();
    }

    fn select_previous_channel_field(&mut self) {
        let AppMode::EditChannel(current_idx) = self.mode else {
            return;
        };
        self.save_current_field_to_pending(current_idx);
        let new_idx = if current_idx == 0 {
            10
        } else {
            current_idx - 1
        };
        self.mode = AppMode::EditChannel(new_idx);
        self.update_edit_buffer();
    }

    fn active_channel_accepts_text_input(&self) -> bool {
        let AppMode::EditChannel(idx) = self.mode else {
            return false;
        };
        !matches!(idx, 6..=9)
    }

    fn select_next_channel_option(&mut self) {
        let Some(option_count) = self.active_channel_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + 1) % option_count;
    }

    fn select_previous_channel_option(&mut self) {
        let Some(option_count) = self.active_channel_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + option_count - 1) % option_count;
    }

    fn active_channel_option_count(&self) -> Option<usize> {
        let AppMode::EditChannel(idx) = self.mode else {
            return None;
        };

        match idx {
            6 | 7 => Some(2),
            8 => Some(5),
            9 => Some(GROUP_LABEL_COUNT + 1),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Channel;

    #[test]
    fn channel_modulation_cycles_all_modes() {
        let mut app = App::new();
        app.pending_channel_edit = Some(Channel::default());
        app.mode = AppMode::EditChannel(8);
        app.selection_index = 4;

        app.handle_channel_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }

    #[test]
    fn channel_boolean_fields_wrap_without_invalid_state() {
        let mut app = App::new();
        app.pending_channel_edit = Some(Channel::default());
        app.mode = AppMode::EditChannel(6);
        app.selection_index = 1;

        app.handle_channel_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }

    #[test]
    fn channel_option_fields_ignore_text_input() {
        let mut app = App::new();
        app.pending_channel_edit = Some(Channel::default());
        app.mode = AppMode::EditChannel(7);
        app.edit_buffer = "Wide".to_string();

        app.handle_channel_editor_key(KeyCode::Char('x'));

        assert_eq!(app.edit_buffer, "Wide");
    }

    #[test]
    fn channel_group_fields_wrap_through_none_and_all_groups() {
        let mut app = App::new();
        app.pending_channel_edit = Some(Channel::default());
        app.mode = AppMode::EditChannel(9);
        app.selection_index = GROUP_LABEL_COUNT;

        app.handle_channel_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }
}
