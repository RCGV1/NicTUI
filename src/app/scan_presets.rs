use crossterm::event::KeyCode;

use super::{App, AppMode, MainTab};
use crate::device::validate_scan_presets;
use crate::protocol::editor_value::{
    SCAN_MODULATIONS, decimal_value, option_label, parse_decimal, validate_label,
};

impl App {
    pub fn start_edit_scan_preset(&mut self) {
        if let Some(preset) = self
            .preset_state
            .selected()
            .and_then(|i| self.scan_presets.get(i))
            .cloned()
        {
            self.editing_scan_preset = Some(preset);
            self.last_main_tab = MainTab::Scanning;
            self.mode = AppMode::EditScanPreset(0);
            self.status_message = "Editing scan preset".to_string();
            self.update_scan_preset_edit_buffer();
        }
    }

    pub fn update_scan_preset_edit_buffer(&mut self) {
        if let AppMode::EditScanPreset(field) = self.mode
            && let Some(preset) = &self.editing_scan_preset
        {
            self.edit_buffer = match field {
                0 => preset.label.clone(),
                1 => decimal_value(preset.start_freq, 5),
                2 => decimal_value(preset.range as u32, 2),
                3 => decimal_value(preset.step as u32, 2),
                4 => decimal_value(preset.persist as u32, 1),
                5 => preset.resume.to_string(),
                6 => {
                    self.selection_index = preset.modulation as usize;
                    option_label(SCAN_MODULATIONS, preset.modulation)
                }
                7 => {
                    self.selection_index = preset.ultrascan as usize;
                    preset.ultrascan.to_string()
                }
                _ => String::new(),
            };
        }
    }

    pub fn save_current_scan_preset_field(&mut self, field: usize) -> bool {
        let Some(preset) = self.editing_scan_preset.as_mut() else {
            return false;
        };
        let result = (|| -> Result<(), String> {
            match field {
                0 => {
                    validate_label(&self.edit_buffer, 8)?;
                    preset.label = self.edit_buffer.clone();
                }
                1 => {
                    let raw = parse_decimal(&self.edit_buffer, 5, 0, 130_000_000)?;
                    if raw != 0 && raw < 1_800_000 {
                        return Err("Start must be 0 (disabled) or 18-1300 MHz".to_string());
                    }
                    preset.start_freq = raw;
                }
                2 => {
                    preset.range = parse_decimal(
                        &self.edit_buffer,
                        2,
                        u32::from(preset.start_freq != 0),
                        60_000,
                    )? as u16
                }
                3 => {
                    preset.step = parse_decimal(
                        &self.edit_buffer,
                        2,
                        u32::from(preset.start_freq != 0),
                        50_000,
                    )? as u16
                }
                4 => preset.persist = parse_decimal(&self.edit_buffer, 1, 0, 200)? as u8,
                5 => preset.resume = parse_decimal(&self.edit_buffer, 0, 0, 250)? as u8,
                6 if self.selection_index < SCAN_MODULATIONS.len() => {
                    preset.modulation = self.selection_index as u8
                }
                7 => preset.ultrascan = parse_decimal(&self.edit_buffer, 0, 0, 20)? as u8,
                _ => return Err("Choose a supported option".to_string()),
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.status_message = format!("Invalid scan: {error}");
            false
        } else {
            if self.status_message.starts_with("Invalid scan:") {
                self.status_message = "Editing scan preset".to_string();
            }
            true
        }
    }

    pub fn commit_scan_preset_edit(&mut self) {
        let AppMode::EditScanPreset(field) = self.mode else {
            return;
        };
        if !self.save_current_scan_preset_field(field) {
            return;
        }
        let Some(preset) = self.editing_scan_preset.as_ref() else {
            return;
        };
        if let Err(error) = validate_scan_presets(std::slice::from_ref(preset)) {
            self.status_message = format!("Invalid scan: {error}");
            return;
        }
        if let Some(slot) = self
            .preset_state
            .selected()
            .and_then(|i| self.scan_presets.get_mut(i))
        {
            let changed = crate::protocol::RadioProtocol::pack_scan_preset(slot, self.endian)
                != crate::protocol::RadioProtocol::pack_scan_preset(preset, self.endian);
            *slot = preset.clone();
            if changed {
                self.scan_presets_dirty = true;
            }
            self.status_message = if changed {
                "Scan preset changed (Unsaved)"
            } else {
                "Scan preset unchanged"
            }
            .to_string();
        }
        self.editing_scan_preset = None;
        self.mode = AppMode::Main(MainTab::Scanning);
    }

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
        if !self.save_current_scan_preset_field(current_idx) {
            return;
        }
        self.mode = AppMode::EditScanPreset((current_idx + 1) % 8);
        self.update_scan_preset_edit_buffer();
    }

    fn select_previous_scan_preset_field(&mut self) {
        let AppMode::EditScanPreset(current_idx) = self.mode else {
            return;
        };
        if !self.save_current_scan_preset_field(current_idx) {
            return;
        }
        let new_idx = if current_idx == 0 { 7 } else { current_idx - 1 };
        self.mode = AppMode::EditScanPreset(new_idx);
        self.update_scan_preset_edit_buffer();
    }

    fn active_scan_preset_is_numeric_or_text(&self) -> bool {
        let AppMode::EditScanPreset(idx) = self.mode else {
            return false;
        };
        idx <= 5 || idx == 7
    }

    fn select_next_scan_preset_option(&mut self) {
        let Some(option_count) = self.active_scan_preset_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + 1) % option_count;
        if matches!(self.mode, AppMode::EditScanPreset(7)) {
            self.edit_buffer = self.selection_index.to_string();
        }
        if let AppMode::EditScanPreset(field) = self.mode {
            self.save_current_scan_preset_field(field);
        }
        self.update_scan_preset_edit_buffer();
    }

    fn select_previous_scan_preset_option(&mut self) {
        let Some(option_count) = self.active_scan_preset_option_count() else {
            return;
        };
        self.selection_index = (self.selection_index + option_count - 1) % option_count;
        if matches!(self.mode, AppMode::EditScanPreset(7)) {
            self.edit_buffer = self.selection_index.to_string();
        }
        if let AppMode::EditScanPreset(field) = self.mode {
            self.save_current_scan_preset_field(field);
        }
        self.update_scan_preset_edit_buffer();
    }

    fn active_scan_preset_option_count(&self) -> Option<usize> {
        let AppMode::EditScanPreset(idx) = self.mode else {
            return None;
        };

        match idx {
            6 => Some(3),
            7 => Some(21),
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
    fn scan_preset_ultrascan_cycles_all_twenty_one_values() {
        let mut app = App::new();
        app.editing_scan_preset = Some(ScanPreset::default());
        app.mode = AppMode::EditScanPreset(7);
        app.selection_index = 20;

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

    fn editing(field: usize) -> App {
        let mut app = App::new();
        app.scan_presets.push(ScanPreset {
            start_freq: 11_800_000,
            range: 1900,
            step: 1250,
            persist: 200,
            resume: 2,
            label: "Air".to_string(),
            ultrascan: 20,
            modulation: 1,
            raw_tail: 0xA5,
            ..ScanPreset::default()
        });
        app.preset_state.select(Some(0));
        app.start_edit_scan_preset();
        app.mode = AppMode::EditScanPreset(field);
        app.update_scan_preset_edit_buffer();
        app
    }

    #[test]
    fn every_scan_field_roundtrips_without_changing_wire_data() {
        for field in 0..8 {
            let mut app = editing(field);
            let before =
                crate::protocol::RadioProtocol::pack_scan_preset(&app.scan_presets[0], app.endian);
            app.commit_scan_preset_edit();
            assert!(
                matches!(app.mode, AppMode::Main(MainTab::Scanning)),
                "field {field}"
            );
            assert_eq!(
                crate::protocol::RadioProtocol::pack_scan_preset(&app.scan_presets[0], app.endian),
                before
            );
            assert!(app.status_message.contains("unchanged"));
            assert!(!app.scan_presets_dirty);
        }
    }

    #[test]
    fn scan_fields_use_physical_units_and_arrows_keep_new_values() {
        for (field, input, raw) in [(2, "20.25", 2025), (3, "6.25", 625), (4, "2.5", 25)] {
            let mut app = editing(field);
            app.edit_buffer = input.to_string();
            app.commit_scan_preset_edit();
            assert!(app.scan_presets_dirty);
            assert_eq!(
                match field {
                    2 => app.scan_presets[0].range as u32,
                    3 => app.scan_presets[0].step as u32,
                    _ => app.scan_presets[0].persist as u32,
                },
                raw
            );
        }
        let mut app = editing(6);
        app.handle_scan_preset_editor_key(KeyCode::Right);
        assert_eq!(app.selection_index, 2);
        app.commit_scan_preset_edit();
        assert_eq!(app.scan_presets[0].modulation, 2);
        assert!(app.scan_presets_dirty);
        let mut app = editing(7);
        app.handle_scan_preset_editor_key(KeyCode::Left);
        app.commit_scan_preset_edit();
        assert_eq!(app.scan_presets[0].ultrascan, 19);
        assert!(app.scan_presets_dirty);
    }

    #[test]
    fn scan_noop_preserves_existing_dirty_state_and_cancel_stays_clean() {
        let mut app = editing(0);
        app.scan_presets_dirty = true;
        app.commit_scan_preset_edit();
        assert!(app.scan_presets_dirty);
        assert!(!app.band_plans_dirty);

        let mut app = editing(0);
        app.edit_buffer = "Changed".to_string();
        app.handle_scan_preset_editor_key(KeyCode::Tab);
        app.handle_scan_preset_editor_key(KeyCode::Esc);
        assert!(!app.scan_presets_dirty);
        assert_eq!(app.scan_presets[0].label, "Air");
    }

    #[test]
    fn invalid_scan_input_blocks_navigation_and_save_and_cancel_discards_draft() {
        for (field, input) in [
            (0, "123456789"),
            (1, "NaN"),
            (2, "0"),
            (3, "0.001"),
            (4, "20.1"),
            (5, "251"),
            (7, "21"),
        ] {
            let mut app = editing(field);
            let original = app.scan_presets[0].clone();
            app.edit_buffer = input.to_string();
            app.handle_scan_preset_editor_key(KeyCode::Tab);
            assert!(matches!(app.mode, AppMode::EditScanPreset(i) if i == field));
            app.commit_scan_preset_edit();
            assert!(app.status_message.starts_with("Invalid scan:"));
            assert!(!app.scan_presets_dirty);
            assert_eq!(app.scan_presets[0].label, original.label);
            app.handle_scan_preset_editor_key(KeyCode::Esc);
            assert!(app.editing_scan_preset.is_none());
        }
    }
}
