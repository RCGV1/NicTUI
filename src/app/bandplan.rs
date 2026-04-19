use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};

impl App {
    pub fn handle_bandplan_editor_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.editing_band_plan = None;
                self.mode = AppMode::Main(MainTab::BandPlan);
            }
            KeyCode::Enter => self.commit_bandplan_edit(),
            KeyCode::Up | KeyCode::BackTab => self.select_previous_bandplan_field(),
            KeyCode::Down | KeyCode::Tab => self.select_next_bandplan_field(),
            KeyCode::Left => self.select_previous_bandplan_option(),
            KeyCode::Right => self.select_next_bandplan_option(),
            KeyCode::Char(c) => {
                if self.active_bandplan_is_numeric() {
                    self.edit_buffer.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.active_bandplan_is_numeric() {
                    self.edit_buffer.pop();
                }
            }
            _ => {}
        }
    }

    fn select_next_bandplan_field(&mut self) {
        let AppMode::EditBandPlan(current_idx) = self.mode else {
            return;
        };
        self.save_current_bandplan_field(current_idx);
        self.mode = AppMode::EditBandPlan((current_idx + 1) % 8);
        self.update_bandplan_edit_buffer();
    }

    fn select_previous_bandplan_field(&mut self) {
        let AppMode::EditBandPlan(current_idx) = self.mode else {
            return;
        };
        self.save_current_bandplan_field(current_idx);
        let new_idx = if current_idx == 0 { 7 } else { current_idx - 1 };
        self.mode = AppMode::EditBandPlan(new_idx);
        self.update_bandplan_edit_buffer();
    }

    fn active_bandplan_is_numeric(&self) -> bool {
        let AppMode::EditBandPlan(idx) = self.mode else {
            return false;
        };
        idx <= 3
    }

    fn select_next_bandplan_option(&mut self) {
        let Some(option_count) = self.active_bandplan_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + 1) % option_count;
        self.update_bandplan_edit_buffer();
    }

    fn select_previous_bandplan_option(&mut self) {
        let Some(option_count) = self.active_bandplan_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + option_count - 1) % option_count;
        self.update_bandplan_edit_buffer();
    }

    fn active_bandplan_option_count(&self) -> Option<usize> {
        let AppMode::EditBandPlan(idx) = self.mode else {
            return None;
        };

        match idx {
            4 | 5 | 7 => Some(2),
            6 => Some(3),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::BandPlan;

    #[test]
    fn binary_bandplan_option_wraps_without_invalid_third_state() {
        let mut app = App::new();
        app.editing_band_plan = Some(BandPlan::default());
        app.mode = AppMode::EditBandPlan(4);
        app.selection_index = 1;

        app.handle_bandplan_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }

    #[test]
    fn modulation_bandplan_option_cycles_three_values() {
        let mut app = App::new();
        app.editing_band_plan = Some(BandPlan::default());
        app.mode = AppMode::EditBandPlan(6);
        app.selection_index = 2;

        app.handle_bandplan_editor_key(KeyCode::Right);

        assert_eq!(app.selection_index, 0);
    }

    #[test]
    fn option_fields_ignore_text_input() {
        let mut app = App::new();
        app.editing_band_plan = Some(BandPlan::default());
        app.mode = AppMode::EditBandPlan(7);
        app.edit_buffer = "Wide".to_string();

        app.handle_bandplan_editor_key(KeyCode::Char('x'));

        assert_eq!(app.edit_buffer, "Wide");
    }
}
