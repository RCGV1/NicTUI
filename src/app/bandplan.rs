use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};
use crate::device::validate_band_plans_payload;
use crate::protocol::editor_value::{
    BAND_BANDWIDTHS, BAND_MODULATIONS, decimal_value, option_label, parse_decimal,
};

impl App {
    pub fn start_edit_bandplan(&mut self) {
        if let Some(plan) = self
            .bandplan_state
            .selected()
            .and_then(|i| self.band_plans.get(i))
            .cloned()
        {
            self.editing_band_plan = Some(plan);
            self.last_main_tab = MainTab::BandPlan;
            self.mode = AppMode::EditBandPlan(0);
            self.status_message = "Editing band plan".to_string();
            self.update_bandplan_edit_buffer();
        }
    }

    pub fn update_bandplan_edit_buffer(&mut self) {
        if let AppMode::EditBandPlan(field) = self.mode
            && let Some(plan) = &self.editing_band_plan
        {
            self.edit_buffer = match field {
                0 => plan.index.to_string(),
                1 => decimal_value(plan.start_freq, 5),
                2 => decimal_value(plan.end_freq, 5),
                3 => plan.max_power.to_string(),
                4 => {
                    self.selection_index = usize::from(plan.tx_allowed);
                    if plan.tx_allowed { "Yes" } else { "No" }.to_string()
                }
                5 => {
                    self.selection_index = usize::from(plan.wrap);
                    if plan.wrap { "Yes" } else { "No" }.to_string()
                }
                6 => {
                    self.selection_index = plan.modulation as usize;
                    option_label(BAND_MODULATIONS, plan.modulation)
                }
                7 => {
                    self.selection_index = plan.bandwidth as usize;
                    option_label(BAND_BANDWIDTHS, plan.bandwidth)
                }
                _ => String::new(),
            };
        }
    }

    pub fn save_current_bandplan_field(&mut self, field: usize) -> bool {
        let Some(plan) = self.editing_band_plan.as_mut() else {
            return false;
        };
        let result = (|| -> Result<(), String> {
            match field {
                0 => plan.index = parse_decimal(&self.edit_buffer, 0, 0, 19)? as u8,
                1 => plan.start_freq = parse_decimal(&self.edit_buffer, 5, 0, 130_000_000)?,
                2 => plan.end_freq = parse_decimal(&self.edit_buffer, 5, 0, 130_000_000)?,
                3 => plan.max_power = parse_decimal(&self.edit_buffer, 0, 0, 255)? as u8,
                4 | 5 if self.selection_index > 1 => return Err("Choose No or Yes".to_string()),
                4 => plan.tx_allowed = self.selection_index == 1,
                5 => plan.wrap = self.selection_index == 1,
                6 if self.selection_index < BAND_MODULATIONS.len() => {
                    plan.modulation = self.selection_index as u8
                }
                7 if self.selection_index < BAND_BANDWIDTHS.len() => {
                    plan.bandwidth = self.selection_index as u8
                }
                _ => return Err("Choose a supported option".to_string()),
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.status_message = format!("Invalid band: {error}");
            false
        } else {
            if self.status_message.starts_with("Invalid band:") {
                self.status_message = "Editing band plan".to_string();
            }
            true
        }
    }

    pub fn commit_bandplan_edit(&mut self) {
        let AppMode::EditBandPlan(field) = self.mode else {
            return;
        };
        if !self.save_current_bandplan_field(field) {
            return;
        }
        let Some(plan) = self.editing_band_plan.as_ref() else {
            return;
        };
        if let Err(error) = validate_band_plans_payload(std::slice::from_ref(plan)) {
            self.status_message = format!("Invalid band: {error}");
            return;
        }
        let Some(index) = self.bandplan_state.selected() else {
            return;
        };
        if self
            .band_plans
            .iter()
            .enumerate()
            .any(|(i, other)| i != index && other.index == plan.index)
        {
            self.status_message = "Invalid band: slot already exists".to_string();
            return;
        }
        if let Some(slot) = self.band_plans.get_mut(index) {
            let changed = crate::protocol::RadioProtocol::pack_bandplan(slot, self.endian)
                != crate::protocol::RadioProtocol::pack_bandplan(plan, self.endian)
                || slot.index != plan.index;
            *slot = plan.clone();
            if changed {
                self.band_plans_dirty = true;
            }
            self.status_message = if changed {
                "Band plan changed (Unsaved)"
            } else {
                "Band plan unchanged"
            }
            .to_string();
        }
        self.editing_band_plan = None;
        self.mode = AppMode::Main(MainTab::BandPlan);
    }

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
            KeyCode::Backspace if self.active_bandplan_is_numeric() => {
                self.edit_buffer.pop();
            }
            _ => {}
        }
    }

    fn select_next_bandplan_field(&mut self) {
        let AppMode::EditBandPlan(current_idx) = self.mode else {
            return;
        };
        if !self.save_current_bandplan_field(current_idx) {
            return;
        }
        self.mode = AppMode::EditBandPlan((current_idx + 1) % 8);
        self.update_bandplan_edit_buffer();
    }

    fn select_previous_bandplan_field(&mut self) {
        let AppMode::EditBandPlan(current_idx) = self.mode else {
            return;
        };
        if !self.save_current_bandplan_field(current_idx) {
            return;
        }
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
        if let AppMode::EditBandPlan(field) = self.mode {
            self.save_current_bandplan_field(field);
        }
        self.update_bandplan_edit_buffer();
    }

    fn select_previous_bandplan_option(&mut self) {
        let Some(option_count) = self.active_bandplan_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + option_count - 1) % option_count;
        if let AppMode::EditBandPlan(field) = self.mode {
            self.save_current_bandplan_field(field);
        }
        self.update_bandplan_edit_buffer();
    }

    fn active_bandplan_option_count(&self) -> Option<usize> {
        let AppMode::EditBandPlan(idx) = self.mode else {
            return None;
        };

        match idx {
            4 | 5 => Some(2),
            6 => Some(BAND_MODULATIONS.len()),
            7 => Some(BAND_BANDWIDTHS.len()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::BandPlan;

    #[test]
    fn backspace_only_changes_numeric_bandplan_fields() {
        for field in 0..8 {
            let mut app = App::new();
            app.mode = AppMode::EditBandPlan(field);
            app.edit_buffer = "12".to_string();
            app.selection_index = 1;

            app.handle_bandplan_editor_key(KeyCode::Backspace);

            assert_eq!(app.edit_buffer, if field <= 3 { "1" } else { "12" });
            assert_eq!(app.selection_index, 1);
            assert_eq!(app.mode, AppMode::EditBandPlan(field));
        }
    }

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
    fn modulation_bandplan_option_cycles_all_eight_values() {
        let mut app = App::new();
        app.editing_band_plan = Some(BandPlan::default());
        app.mode = AppMode::EditBandPlan(6);
        app.selection_index = 7;

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

    fn editing(field: usize) -> App {
        let mut app = App::new();
        app.band_plans.push(BandPlan {
            start_freq: 14_400_000,
            end_freq: 14_800_000,
            max_power: 255,
            modulation: 6,
            bandwidth: 5,
            tx_allowed: true,
            wrap: true,
            raw_flags: 0xFF,
            ..BandPlan::default()
        });
        app.bandplan_state.select(Some(0));
        app.start_edit_bandplan();
        app.mode = AppMode::EditBandPlan(field);
        app.update_bandplan_edit_buffer();
        app
    }

    #[test]
    fn every_band_field_roundtrips_without_power_loss_or_enum_changes() {
        for field in 0..8 {
            let mut app = editing(field);
            let before =
                crate::protocol::RadioProtocol::pack_bandplan(&app.band_plans[0], app.endian);
            app.commit_bandplan_edit();
            assert!(
                matches!(app.mode, AppMode::Main(MainTab::BandPlan)),
                "field {field}"
            );
            assert_eq!(
                crate::protocol::RadioProtocol::pack_bandplan(&app.band_plans[0], app.endian),
                before
            );
            assert!(app.status_message.contains("unchanged"));
            assert!(!app.band_plans_dirty);
        }
    }

    #[test]
    fn all_band_mode_and_width_codes_commit_and_pack_exactly() {
        for (field, count) in [(6, 8), (7, 6)] {
            for code in 0..count {
                let mut app = editing(field);
                app.selection_index = code;
                app.commit_bandplan_edit();
                let raw =
                    crate::protocol::RadioProtocol::pack_bandplan(&app.band_plans[0], app.endian)
                        [9];
                assert_eq!(
                    if field == 6 { (raw >> 2) & 7 } else { raw >> 5 },
                    code as u8
                );
                assert_eq!(app.band_plans_dirty, code != if field == 6 { 6 } else { 5 });
            }
        }
        let mut app = editing(6);
        app.handle_bandplan_editor_key(KeyCode::Right);
        assert_eq!(app.selection_index, 7);
        app.commit_bandplan_edit();
        assert_eq!(app.band_plans[0].modulation, 7);
        assert!(app.band_plans_dirty);
    }

    #[test]
    fn band_noop_preserves_existing_dirty_state_and_cancel_stays_clean() {
        let mut app = editing(3);
        app.band_plans_dirty = true;
        app.commit_bandplan_edit();
        assert!(app.band_plans_dirty);
        assert!(!app.scan_presets_dirty);

        let mut app = editing(3);
        app.edit_buffer = "200".to_string();
        app.handle_bandplan_editor_key(KeyCode::Tab);
        app.handle_bandplan_editor_key(KeyCode::Esc);
        assert!(!app.band_plans_dirty);
        assert_eq!(app.band_plans[0].max_power, 255);
    }

    #[test]
    fn invalid_band_values_or_duplicate_slots_do_not_commit() {
        for (field, input) in [(0, "20"), (1, "inf"), (2, "140.00000"), (3, "256")] {
            let mut app = editing(field);
            app.edit_buffer = input.to_string();
            app.commit_bandplan_edit();
            assert!(matches!(app.mode, AppMode::EditBandPlan(i) if i == field));
            assert_eq!(app.band_plans[0].max_power, 255);
            assert!(app.status_message.starts_with("Invalid band:"));
            assert!(!app.band_plans_dirty);
        }
        let mut app = editing(0);
        app.band_plans.push(BandPlan {
            index: 1,
            ..BandPlan::default()
        });
        app.edit_buffer = "1".to_string();
        app.commit_bandplan_edit();
        assert_eq!(app.band_plans[0].index, 0);
        assert!(app.status_message.contains("already exists"));
        assert!(!app.band_plans_dirty);
    }
}
