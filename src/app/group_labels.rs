use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};
use crate::protocol::{GROUP_LABEL_COUNT, GROUP_LABEL_TEXT_SIZE, normalize_group_labels};

impl App {
    pub fn start_edit_group_label(&mut self) {
        let index = self
            .scanning_group_state
            .selected()
            .unwrap_or(0)
            .min(GROUP_LABEL_COUNT - 1);
        self.last_main_tab = MainTab::MemoryGroups;
        self.editing_group_label_idx = Some(index);
        self.mode = AppMode::EditGroupLabel(index);
        self.edit_buffer = self.group_labels.get(index).cloned().unwrap_or_default();
    }

    pub fn commit_group_label_edit(&mut self) {
        let Some(index) = self.editing_group_label_idx else {
            self.mode = AppMode::Main(MainTab::MemoryGroups);
            return;
        };
        if index >= GROUP_LABEL_COUNT {
            self.editing_group_label_idx = None;
            self.mode = AppMode::Main(MainTab::MemoryGroups);
            return;
        }
        if !self
            .edit_buffer
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        {
            self.status_message = "Group names accept printable ASCII characters only".to_string();
            return;
        }

        let mut labels = self.group_labels.clone();
        if labels.len() < GROUP_LABEL_COUNT {
            labels.resize(GROUP_LABEL_COUNT, String::new());
        }
        labels[index] = self.edit_buffer.trim().to_string();
        let labels = normalize_group_labels(&labels);
        if labels != self.group_labels {
            self.group_labels = labels;
            self.group_labels_dirty = true;
            self.status_message = format!(
                "Group {} name updated (unsaved)",
                (b'A' + index as u8) as char
            );
        } else {
            self.status_message = "Group name unchanged".to_string();
        }
        self.editing_group_label_idx = None;
        self.mode = AppMode::Main(MainTab::MemoryGroups);
    }

    pub fn handle_group_label_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.editing_group_label_idx = None;
                self.mode = AppMode::Main(MainTab::MemoryGroups);
                self.status_message = "Group name edit cancelled".into();
            }
            KeyCode::Enter => self.commit_group_label_edit(),
            KeyCode::Char(c) => {
                if (c.is_ascii_graphic() || c == ' ')
                    && self.edit_buffer.len() < GROUP_LABEL_TEXT_SIZE
                {
                    self.edit_buffer.push(c);
                }
            }
            KeyCode::Backspace => {
                self.edit_buffer.pop();
            }
            _ => {}
        }
    }

    pub fn start_write_dirty_group_labels(&mut self) {
        if !self.group_labels_dirty {
            self.status_message = "No group name changes to save".to_string();
            return;
        }
        self.start_write_group_labels();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_label_edit_marks_dirty_and_truncates() {
        let mut app = App::new();
        app.scanning_group_state.select(Some(0));
        app.ensure_group_selection();
        app.start_edit_group_label();
        app.edit_buffer = "WeatherNet".to_string();

        app.commit_group_label_edit();

        assert_eq!(app.group_labels[0], "Weath");
        assert!(app.group_labels_dirty);
    }

    #[test]
    fn unchanged_group_name_does_not_dirty_workspace() {
        let mut app = App::new();
        app.group_labels = normalize_group_labels(&["Weather".to_string()]);
        app.start_edit_group_label();
        app.commit_group_label_edit();
        assert!(!app.group_labels_dirty);
        assert_eq!(app.group_labels[0], "Weath");
    }

    #[test]
    fn unsupported_group_characters_do_not_reach_the_wire() {
        let mut app = App::new();
        app.start_edit_group_label();
        app.handle_group_label_editor_key(KeyCode::Char('\u{00e9}'));
        assert!(app.edit_buffer.is_empty());
        app.edit_buffer = "bad\n".to_string();
        app.commit_group_label_edit();
        assert!(!app.group_labels_dirty);
        assert!(app.editing_group_label_idx.is_some());
    }
}
