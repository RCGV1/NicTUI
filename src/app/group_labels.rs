use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};
use crate::protocol::{GROUP_LABEL_COUNT, normalize_group_labels};

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
        let Some(index) = self.editing_group_label_idx.take() else {
            self.mode = AppMode::Main(MainTab::MemoryGroups);
            return;
        };

        let mut labels = self.group_labels.clone();
        if labels.len() < GROUP_LABEL_COUNT {
            labels.resize(GROUP_LABEL_COUNT, String::new());
        }
        labels[index] = self.edit_buffer.trim().to_string();
        self.group_labels = normalize_group_labels(&labels);
        self.group_labels_dirty = true;
        self.status_message = format!(
            "Group {} name updated (unsaved)",
            (b'A' + index as u8) as char
        );
        self.mode = AppMode::Main(MainTab::MemoryGroups);
    }

    pub fn handle_group_label_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.editing_group_label_idx = None;
                self.mode = AppMode::Main(MainTab::MemoryGroups);
            }
            KeyCode::Enter => self.commit_group_label_edit(),
            KeyCode::Char(c) => {
                if self.edit_buffer.chars().count() < 6 {
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

        assert_eq!(app.group_labels[0], "Weathe");
        assert!(app.group_labels_dirty);
    }
}
