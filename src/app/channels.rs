use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};
use crate::protocol::GROUP_LABEL_COUNT;

impl App {
    pub fn handle_channel_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.pending_channel_edit = None;
                self.mode = AppMode::Main(MainTab::Channels);
                self.status_message = "Channel edit cancelled".into();
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
            KeyCode::Backspace if self.active_channel_accepts_text_input() => {
                self.edit_buffer.pop();
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
            8 => Some(4),
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
    fn backspace_only_changes_text_input_channel_fields() {
        for field in 0..11 {
            let mut app = App::new();
            app.mode = AppMode::EditChannel(field);
            app.edit_buffer = "12".to_string();
            app.selection_index = 1;

            app.handle_channel_editor_key(KeyCode::Backspace);

            assert_eq!(
                app.edit_buffer,
                if matches!(field, 6..=9) { "12" } else { "1" }
            );
            assert_eq!(app.selection_index, 1);
            assert_eq!(app.mode, AppMode::EditChannel(field));
        }
    }

    #[test]
    fn channel_modulation_cycles_all_modes() {
        let mut app = App::new();
        app.pending_channel_edit = Some(Channel::default());
        app.mode = AppMode::EditChannel(8);
        app.selection_index = 3;

        app.handle_channel_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }

    #[test]
    fn channel_reverse_field_wraps_and_preserves_position_on_navigation() {
        for position in [0, 1] {
            let mut app = App::new();
            app.pending_channel_edit = Some(Channel {
                reverse: true,
                position,
                ..Channel::default()
            });
            app.mode = AppMode::EditChannel(6);
            app.update_edit_buffer();

            app.handle_channel_editor_key(KeyCode::Right);
            assert_eq!(app.selection_index, 0);
            app.handle_channel_editor_key(KeyCode::Tab);
            assert_eq!(app.mode, AppMode::EditChannel(7));
            let pending = app.pending_channel_edit.as_ref().unwrap();
            assert!(!pending.reverse);
            assert_eq!(pending.position, position);

            app.handle_channel_editor_key(KeyCode::BackTab);
            app.handle_channel_editor_key(KeyCode::Left);
            assert_eq!(app.selection_index, 1);
            app.handle_channel_editor_key(KeyCode::Tab);
            let pending = app.pending_channel_edit.as_ref().unwrap();
            assert!(pending.reverse);
            assert_eq!(pending.position, position);
        }
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
