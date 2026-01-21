use super::navigation::{next_item, prev_item, update_selection_after_add};
use super::state::{App, AppEvent, AppMode, MainTab};
use crate::protocol::*;

impl App {
    pub fn update(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::Progress(p) => self.progress = p,
                AppEvent::Status(s) => self.status_message = s,
                AppEvent::Log(l) => self.log(&l),
                AppEvent::ReadChannelsComplete(channels, endian) => {
                    self.channels = channels;
                    self.endian = endian;
                    self.channel_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::Channels);
                    self.status_message = "Channels read complete".to_string();
                }
                AppEvent::ReadPresetsComplete(presets) => {
                    self.scan_presets = presets;
                    self.preset_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::Scanning);
                    self.status_message = "Presets read complete".to_string();
                }
                AppEvent::ReadBandPlanComplete(plans) => {
                    self.band_plans = plans;
                    self.bandplan_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::BandPlan);
                    self.status_message = "BandPlan read complete".to_string();
                }
                AppEvent::ReadDTMFComplete(presets) => {
                    self.dtmf_presets = presets;
                    self.dtmf_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::DTMF);
                    self.status_message = "DTMF read complete".to_string();
                }
                AppEvent::ReadSettingsComplete(settings, endian) => {
                    self.settings = Some(settings);
                    self.endian = endian;
                    self.settings_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::Settings);
                    self.status_message = "Settings read complete".to_string();
                }
                AppEvent::RemotePacket(pkt) => {
                    let now = std::time::Instant::now();
                    match pkt {
                        RemotePacket::SignalStrength {
                            strength, battery, ..
                        } => {
                            self.remote_screen.signal_strength = strength;
                            self.remote_screen.battery_level = Some(battery);
                            self.remote_screen.last_signal_update = Some(now);
                            self.remote_screen.last_battery_update = Some(now);
                        }
                        RemotePacket::NoiseLevel { level, .. } => {
                            self.remote_screen.noise_level = level;
                            self.remote_screen.last_noise_update = Some(now);
                        }
                        RemotePacket::LedStatus { status } => {
                            self.remote_screen.leds = status;
                            self.remote_screen.last_led_update = Some(now);
                        }
                        _ => {
                            self.remote_screen.elements.push(pkt);
                            if self.remote_screen.elements.len() > 50 {
                                self.remote_screen.elements.remove(0);
                            }
                        }
                    }
                }
                AppEvent::WriteComplete => {
                    self.mode = AppMode::Main(self.last_main_tab);
                    self.channels_dirty = false;
                    self.deleted_channels.clear();
                    self.dtmf_dirty = false;
                }
                AppEvent::LoadCSV(path) => self.start_import_and_write(path),
                AppEvent::WriteCSV(path) => self.start_write_csv_channels(path),
                AppEvent::ExportCSV(path) => {
                    if let Err(e) = self.export_csv(path) {
                        self.mode = AppMode::Error(format!("Failed to export CSV: {}", e));
                    }
                }
                AppEvent::LoadCodeplug(path) => self.start_import_codeplug(path),
                AppEvent::ExportCodeplug(path) => self.start_export_codeplug(path),
                AppEvent::ShowImportDialog => self.show_import_dialog(),
                AppEvent::ShowExportDialog => self.show_export_dialog(),
                AppEvent::ShowWriteDialog => self.show_write_dialog(),
                AppEvent::ShowCodeplugImportDialog => self.show_codeplug_import_dialog(),
                AppEvent::ShowCodeplugExportDialog => self.show_codeplug_export_dialog(),
                AppEvent::CodeplugLoaded(path, data) => {
                    self.codeplug_data = Some(data);
                    self.codeplug_path = Some(path);
                    self.status_message = "Codeplug loaded successfully".to_string();
                    self.mode = AppMode::Main(MainTab::Codeplug);
                }
                AppEvent::ShowBinFirmwareDialog => self.show_bin_firmware_dialog(),
                AppEvent::LoadBinFirmware(path) => self.start_load_bin_firmware(path),
                AppEvent::BinFirmwareLoaded(path, data) => {
                    self.bin_firmware_data = Some(data);
                    self.bin_file_path = Some(path);
                    self.status_message = "Firmware file loaded".to_string();
                    self.progress = 0.0;
                    self.mode = AppMode::Main(MainTab::BinFlash);
                }
                AppEvent::BinFlashComplete => {
                    self.status_message = "BIN flashing completed successfully!".to_string();
                    self.progress = 0.0;
                    self.mode = AppMode::Main(MainTab::BinFlash);
                }
                AppEvent::BinFlashFailed(msg) => {
                    self.mode = AppMode::Error(msg);
                }
                AppEvent::Error(e) => {
                    self.mode = AppMode::Error(e);
                }
                AppEvent::SuspendUI => self.suspend_ui(),
                AppEvent::ResumeUI => self.resume_ui(),
            }
        }
    }

    pub fn next_channel(&mut self) {
        if !self.channels.is_empty() {
            next_item(self.channels.len(), &mut self.channel_state);
        }
    }

    pub fn prev_channel(&mut self) {
        if !self.channels.is_empty() {
            prev_item(self.channels.len(), &mut self.channel_state);
        }
    }

    pub fn next_dtmf(&mut self) {
        next_item(self.dtmf_presets.len(), &mut self.dtmf_state);
    }

    pub fn prev_dtmf(&mut self) {
        prev_item(self.dtmf_presets.len(), &mut self.dtmf_state);
    }

    pub fn next_bandplan(&mut self) {
        next_item(self.band_plans.len(), &mut self.bandplan_state);
    }

    pub fn prev_bandplan(&mut self) {
        prev_item(self.band_plans.len(), &mut self.bandplan_state);
    }

    pub fn next_setting(&mut self) {
        if self.settings.is_some() {
            next_item(SETTINGS_METADATA.len(), &mut self.settings_state);
        }
    }

    pub fn prev_setting(&mut self) {
        if self.settings.is_some() {
            prev_item(SETTINGS_METADATA.len(), &mut self.settings_state);
        }
    }

    pub fn next_scanning_item(&mut self) {
        if self.scanning_focus == 0 {
            if !self.scan_presets.is_empty() {
                next_item(self.scan_presets.len(), &mut self.preset_state);
            }
        } else if !self.channels.is_empty() {
            next_item(self.channels.len(), &mut self.scanning_group_state);
        }
    }

    pub fn prev_scanning_item(&mut self) {
        if self.scanning_focus == 0 {
            if !self.scan_presets.is_empty() {
                prev_item(self.scan_presets.len(), &mut self.preset_state);
            }
        } else if !self.channels.is_empty() {
            prev_item(self.channels.len(), &mut self.scanning_group_state);
        }
    }

    pub fn toggle_scanning_focus(&mut self) {
        self.scanning_focus = 1 - self.scanning_focus;
        if self.scanning_focus == 1
            && self.scanning_group_state.selected().is_none()
            && !self.channels.is_empty()
        {
            self.scanning_group_state.select(Some(0));
        }
    }

    pub fn start_edit_scan_preset(&mut self) {
        if let Some(i) = self.preset_state.selected() {
            if i < self.scan_presets.len() {
                if let Some(sp) = self.scan_presets.get(i).cloned() {
                    self.last_main_tab = MainTab::Scanning;
                    self.editing_scan_preset = Some(sp);
                    self.mode = AppMode::EditScanPreset(0);
                    self.edit_buffer.clear();
                    self.selection_index = 0;
                    self.update_scan_preset_edit_buffer();
                }
            }
        }
    }

    pub fn update_scan_preset_edit_buffer(&mut self) {
        if let AppMode::EditScanPreset(field_idx) = self.mode {
            if let Some(sp) = self.editing_scan_preset.as_ref() {
                self.edit_buffer = match field_idx {
                    0 => sp.label.clone(),
                    1 => format!("{:.5}", sp.start_freq as f64 / 100000.0),
                    2 => sp.range.to_string(),
                    3 => sp.step.to_string(),
                    4 => sp.persist.to_string(),
                    5 => sp.resume.to_string(),
                    6 => {
                        self.selection_index = match sp.modulation {
                            1 => 1,
                            2 => 2,
                            _ => 0,
                        };
                        match sp.modulation {
                            1 => "AM".to_string(),
                            2 => "USB".to_string(),
                            _ => "FM".to_string(),
                        }
                    }
                    7 => {
                        self.selection_index = sp.ultrascan as usize;
                        sp.ultrascan.to_string()
                    }
                    _ => String::new(),
                };
            }
        }
    }

    pub fn save_current_scan_preset_field(&mut self, field_idx: usize) {
        if let Some(sp) = self.editing_scan_preset.as_mut() {
            match field_idx {
                0 => sp.label = self.edit_buffer.clone(),
                1 => {
                    if let Ok(freq) = self.edit_buffer.parse::<u32>() {
                        sp.start_freq = freq * 100000;
                    }
                }
                2 => {
                    if let Ok(range) = self.edit_buffer.parse::<u16>() {
                        sp.range = range;
                    }
                }
                3 => {
                    if let Ok(step) = self.edit_buffer.parse::<u16>() {
                        sp.step = step;
                    }
                }
                4 => {
                    if let Ok(persist) = self.edit_buffer.parse::<u8>() {
                        sp.persist = persist;
                    }
                }
                5 => {
                    if let Ok(resume) = self.edit_buffer.parse::<u8>() {
                        sp.resume = resume;
                    }
                }
                6 => {
                    sp.modulation = match self.selection_index {
                        1 => 1,
                        2 => 2,
                        _ => 0,
                    };
                }
                7 => {
                    sp.ultrascan = self.selection_index as u8;
                }
                _ => {}
            }
        }
    }

    pub fn commit_scan_preset_edit(&mut self) {
        if let AppMode::EditScanPreset(field_idx) = self.mode {
            self.save_current_scan_preset_field(field_idx);
        }

        if let Some(edited_sp) = self.editing_scan_preset.take() {
            if let Some(i) = self.preset_state.selected() {
                if i < self.scan_presets.len() {
                    self.scan_presets[i] = edited_sp;
                    self.status_message = format!("Scan preset {} saved", i + 1);
                }
            }
        }
        self.mode = AppMode::Main(MainTab::Scanning);
    }

    pub fn next_port(&mut self) {
        if !self.ports.is_empty() {
            self.selected_port_index = (self.selected_port_index + 1) % self.ports.len();
        }
    }

    pub fn previous_port(&mut self) {
        if !self.ports.is_empty() {
            self.selected_port_index = self.selected_port_index.saturating_sub(1);
        }
    }

    pub fn next_tab(&mut self) {
        if let AppMode::Main(tab) = self.mode {
            if tab == MainTab::Remote {
                self.remote_off();
            }
            let next = match tab {
                MainTab::Channels => MainTab::Settings,
                MainTab::Settings => MainTab::Scanning,
                MainTab::Scanning => MainTab::BandPlan,
                MainTab::BandPlan => MainTab::DTMF,
                MainTab::DTMF => MainTab::Remote,
                MainTab::Remote => MainTab::Codeplug,
                MainTab::Codeplug => MainTab::BinFlash,
                MainTab::BinFlash => MainTab::Debug,
                MainTab::Debug => MainTab::Channels,
            };
            self.mode = AppMode::Main(next);
            self.last_main_tab = next;
        }
    }

    pub fn prev_tab(&mut self) {
        if let AppMode::Main(tab) = self.mode {
            if tab == MainTab::Remote {
                self.remote_off();
            }
            let prev = match tab {
                MainTab::Channels => MainTab::Debug,
                MainTab::Settings => MainTab::Channels,
                MainTab::Scanning => MainTab::Settings,
                MainTab::BandPlan => MainTab::Scanning,
                MainTab::DTMF => MainTab::BandPlan,
                MainTab::Remote => MainTab::DTMF,
                MainTab::Codeplug => MainTab::Remote,
                MainTab::BinFlash => MainTab::Codeplug,
                MainTab::Debug => MainTab::BinFlash,
            };
            self.mode = AppMode::Main(prev);
            self.last_main_tab = prev;
        }
    }

    pub fn start_edit_channel(&mut self) {
        if let Some(i) = self.channel_state.selected() {
            if let Some(ch) = self.channels.get(i).cloned() {
                self.last_main_tab = MainTab::Channels;
                self.pending_channel_edit = Some(ch);
                self.mode = AppMode::EditChannel(0);
                self.edit_buffer.clear();
                self.selection_index = 0;
                self.update_edit_buffer();
            }
        }
    }

    pub fn update_edit_buffer(&mut self) {
        if let AppMode::EditChannel(field_idx) = self.mode {
            if let Some(ch) = self.pending_channel_edit.as_ref() {
                self.edit_buffer = match field_idx {
                    0 => ch.name.clone(),
                    1 => ch.rx_freq.clone(),
                    2 => ch.tx_freq.clone(),
                    3 => ch.rx_tone.clone(),
                    4 => ch.tx_tone.clone(),
                    5 => {
                        if ch.power == 0 {
                            "Off".to_string()
                        } else {
                            ch.power.to_string()
                        }
                    }
                    6 => {
                        self.selection_index = if ch.position == 1 { 1 } else { 0 };
                        if ch.position == 1 {
                            "On".to_string()
                        } else {
                            "Off".to_string()
                        }
                    }
                    7 => {
                        self.selection_index = if ch.bandwidth == "Narrow" { 1 } else { 0 };
                        ch.bandwidth.clone()
                    }
                    8 => {
                        self.selection_index = match ch.modulation.as_str() {
                            "AM" => 1,
                            "USB" => 2,
                            "LSB" => 3,
                            "CW" => 4,
                            _ => 0,
                        };
                        ch.modulation.clone()
                    }
                    9 => {
                        let mut s = String::new();
                        for &g in ch.groups.iter() {
                            if g != 0 && g != 0xFF && g >= 1 && g <= 26 {
                                s.push((b'A' + g - 1) as char);
                            }
                        }
                        s
                    }
                    10 => ch.channel_num.to_string(),
                    _ => String::new(),
                };
            }
        }
    }

    pub fn save_current_field_to_pending(&mut self, field_idx: usize) {
        if let Some(ch) = self.pending_channel_edit.as_mut() {
            match field_idx {
                0 => ch.name = self.edit_buffer.clone(),
                1 => ch.rx_freq = self.edit_buffer.clone(),
                2 => ch.tx_freq = self.edit_buffer.clone(),
                3 => ch.rx_tone = self.edit_buffer.clone(),
                4 => ch.tx_tone = self.edit_buffer.clone(),
                5 => {
                    let val_str = self.edit_buffer.trim();
                    ch.power = if val_str.eq_ignore_ascii_case("Off") {
                        0
                    } else {
                        val_str.parse::<u8>().unwrap_or(0)
                    };
                }
                6 => ch.position = if self.selection_index == 1 { 1 } else { 0 },
                7 => {
                    ch.bandwidth = if self.selection_index == 1 {
                        "Narrow".to_string()
                    } else {
                        "Wide".to_string()
                    }
                }
                8 => {
                    ch.modulation = match self.selection_index {
                        1 => "AM".to_string(),
                        2 => "USB".to_string(),
                        3 => "LSB".to_string(),
                        4 => "CW".to_string(),
                        _ => "FM".to_string(),
                    }
                }
                9 => {
                    let mut groups = [0; 4];
                    let mut current_slot = 0;
                    for c in self.edit_buffer.to_uppercase().chars().take(4) {
                        if c >= 'A' && c <= 'O' {
                            groups[current_slot] = c as u8 - b'A' + 1;
                            current_slot += 1;
                        }
                    }
                    ch.groups = groups;
                }
                10 => {
                    if let Ok(num) = self.edit_buffer.parse::<u16>() {
                        ch.channel_num = num;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn commit_edit(&mut self) {
        let mut index_changed = false;
        if let AppMode::EditChannel(field_idx) = self.mode {
            self.save_current_field_to_pending(field_idx);
            if field_idx == 10 {
                index_changed = true;
            }
        }

        if let Some(pending_ch) = self.pending_channel_edit.take() {
            let new_channel_num = pending_ch.channel_num;
            let old_channel_num = (self.channel_state.selected().unwrap_or(0) + 1) as u16;

            if let Some(i) = self.channel_state.selected() {
                if i < self.channels.len() {
                    self.channels[i] = pending_ch;

                    if new_channel_num != old_channel_num {
                        self.renumber_channels();
                        index_changed = true;
                        if let Some(new_pos) = self
                            .channels
                            .iter()
                            .position(|c| c.channel_num == new_channel_num)
                        {
                            self.channel_state.select(Some(new_pos));
                        }
                    }

                    if index_changed {
                        self.channels_dirty = true;
                        self.status_message =
                            format!("Channel {} saved (Unsaved)", new_channel_num);
                    } else {
                        self.status_message = format!("Channel {} saved", new_channel_num);
                    }
                }
            }
        }
        self.mode = AppMode::Main(MainTab::Channels);
    }

    pub fn renumber_channels(&mut self) {
        for (j, c) in self.channels.iter_mut().enumerate() {
            c.channel_num = (j + 1) as u16;
        }
    }

    pub fn start_edit_setting(&mut self) {
        if let Some(i) = self.settings_state.selected() {
            self.mode = AppMode::EditSetting(i);
            if let Some(s) = &self.settings {
                let meta = &crate::protocol::SETTINGS_METADATA[i];
                match meta.setting_type {
                    crate::protocol::SettingType::Enum(_)
                    | crate::protocol::SettingType::Boolean => {
                        self.selection_index = s.get_value(i) as usize;
                    }
                    _ => {}
                }
            }
            self.update_setting_edit_buffer();
        }
    }

    pub fn update_setting_edit_buffer(&mut self) {
        if let AppMode::EditSetting(idx) = self.mode {
            if let Some(s) = &self.settings {
                self.edit_buffer = s.get_value(idx).to_string();
            }
        }
    }

    pub fn commit_setting_edit(&mut self) {
        if let AppMode::EditSetting(idx) = self.mode {
            if let Some(s) = &mut self.settings {
                let meta = &crate::protocol::SETTINGS_METADATA[idx];
                match meta.setting_type {
                    crate::protocol::SettingType::Enum(_)
                    | crate::protocol::SettingType::Boolean => {
                        s.set_value(idx, self.selection_index as u32);
                    }
                    _ => {
                        if let Ok(val) = self.edit_buffer.parse::<u32>() {
                            s.set_value(idx, val);
                        }
                    }
                }
                self.settings_dirty = true;
                self.status_message = "Setting changed (Unsaved)".to_string();
            }
            self.mode = AppMode::Main(MainTab::Settings);
        }
    }

    pub fn save_current_dtmf_field_to_pending(&mut self, field_idx: usize) {
        if let Some(preset_idx) = self.dtmf_edit_preset_idx {
            if let Some(dtmf) = self.dtmf_presets.get_mut(preset_idx) {
                match field_idx {
                    0 => dtmf.label = self.edit_buffer.clone(),
                    1 => {
                        let mut digits = Vec::new();
                        for c in self.edit_buffer.to_uppercase().chars() {
                            match c {
                                '0'..='9' => digits.push(c as u8 - b'0'),
                                'A' => digits.push(10),
                                'B' => digits.push(11),
                                'C' => digits.push(12),
                                'D' => digits.push(13),
                                'E' => digits.push(14),
                                'F' => digits.push(15),
                                '*' => digits.push(12),
                                '#' => digits.push(15),
                                _ => {}
                            }
                        }
                        dtmf.digits = digits;
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn update_dtmf_edit_buffer(&mut self) {
        if let AppMode::EditDTMF(field_idx) = self.mode {
            if let Some(preset_idx) = self.dtmf_edit_preset_idx {
                if let Some(dtmf) = self.dtmf_presets.get(preset_idx) {
                    self.edit_buffer = match field_idx {
                        0 => dtmf.label.clone(),
                        1 => dtmf.digits.iter().map(|d| format!("{:X}", d)).collect(),
                        _ => String::new(),
                    };
                }
            }
        }
    }

    pub fn commit_dtmf_edit(&mut self) {
        if let AppMode::EditDTMF(field_idx) = self.mode {
            self.save_current_dtmf_field_to_pending(field_idx);
        }

        if self.dtmf_edit_preset_idx.is_some() {
            self.dtmf_dirty = true;
            self.status_message = "DTMF preset changed (Unsaved)".to_string();
        }
        self.dtmf_edit_preset_idx = None;
        self.mode = AppMode::Main(MainTab::DTMF);
    }

    pub fn delete_selected_channel(&mut self) {
        if let Some(i) = self.channel_state.selected() {
            if i < self.channels.len() {
                let ch_num = self.channels[i].channel_num;
                if !self.deleted_channels.contains(&ch_num) {
                    self.deleted_channels.push(ch_num);
                    self.channels_dirty = true;
                    self.status_message =
                        format!("Channel {} marked for deletion (Unsaved)", ch_num);
                }
            }
        }
    }

    pub fn confirm_delete_channel(&mut self, idx: usize) {
        if idx < self.channels.len() {
            let ch_num = self.channels[idx].channel_num;
            if !self.deleted_channels.contains(&ch_num) {
                self.deleted_channels.push(ch_num);
                self.channels_dirty = true;
                self.status_message = format!("Channel {} marked for deletion (Unsaved)", ch_num);
            }
        }
        self.mode = AppMode::Main(MainTab::Channels);
    }

    pub fn undelete_channel(&mut self) {
        if let Some(i) = self.channel_state.selected() {
            if i < self.channels.len() {
                let ch_num = self.channels[i].channel_num;
                if let Some(pos) = self.deleted_channels.iter().position(|&x| x == ch_num) {
                    self.deleted_channels.remove(pos);
                    self.channels_dirty = !self.deleted_channels.is_empty();
                    self.status_message = format!("Channel {} undeleted", ch_num);
                }
            }
        }
    }

    pub fn change_channel_index(&mut self, new_index: u16) {
        if let Some(i) = self.channel_state.selected() {
            if i < self.channels.len() {
                let old_index = self.channels[i].channel_num;
                if new_index != old_index {
                    self.channels[i].channel_num = new_index;
                    self.channels_dirty = true;
                    self.status_message =
                        format!("Channel {} -> {} (Unsaved)", old_index, new_index);
                }
            }
        }
    }

    pub fn add_new_channel(&mut self, _at_index: Option<usize>) {
        if self.channels.len() >= 200 {
            self.status_message = "Cannot add more than 200 channels".to_string();
            return;
        }

        let new_channel = Channel {
            channel_num: (self.channels.len() + 1) as u16,
            name: "New Channel".to_string(),
            rx_freq: "0".to_string(),
            tx_freq: "0".to_string(),
            rx_tone: "Off".to_string(),
            tx_tone: "Off".to_string(),
            power: 0,
            bandwidth: "Wide".to_string(),
            modulation: "FM".to_string(),
            reverse: false,
            busy_lock: false,
            groups: [0; 4],
            ptt_id: 0,
            position: 0,
        };

        self.channels.push(new_channel);
        self.channels_dirty = true;
        update_selection_after_add(&mut self.channel_state, self.channels.len() - 1);
        self.status_message = format!("Channel {} added (Unsaved)", self.channels.len());
    }

    pub fn start_write_dirty_channels(&mut self) {
        if !self.channels_dirty && self.deleted_channels.is_empty() {
            self.status_message = "No changes to write".to_string();
            return;
        }
        self.start_write_multiple_channels(0, false);
    }

    pub fn start_write_dirty_dtmf(&mut self) {
        if !self.dtmf_dirty {
            self.status_message = "No DTMF changes to write".to_string();
            return;
        }
        self.start_write_dtmf();
        self.dtmf_dirty = false;
    }

    pub fn start_edit_bandplan(&mut self) {
        if let Some(i) = self.bandplan_state.selected() {
            if i < self.band_plans.len() {
                if let Some(bp) = self.band_plans.get(i).cloned() {
                    self.last_main_tab = MainTab::BandPlan;
                    self.editing_band_plan = Some(bp);
                    self.mode = AppMode::EditBandPlan(0);
                    self.edit_buffer.clear();
                    self.selection_index = 0;
                    self.update_bandplan_edit_buffer();
                }
            }
        }
    }

    pub fn update_bandplan_edit_buffer(&mut self) {
        if let AppMode::EditBandPlan(field_idx) = self.mode {
            if let Some(bp) = self.editing_band_plan.as_ref() {
                self.edit_buffer = match field_idx {
                    0 => bp.index.to_string(),
                    1 => format!("{:.5}", bp.start_freq as f64 / 100000.0),
                    2 => format!("{:.5}", bp.end_freq as f64 / 100000.0),
                    3 => bp.max_power.to_string(),
                    4 => {
                        self.selection_index = if bp.tx_allowed { 1 } else { 0 };
                        if bp.tx_allowed {
                            "Yes".to_string()
                        } else {
                            "No".to_string()
                        }
                    }
                    5 => {
                        self.selection_index = if bp.wrap { 1 } else { 0 };
                        if bp.wrap {
                            "Yes".to_string()
                        } else {
                            "No".to_string()
                        }
                    }
                    6 => {
                        self.selection_index = match bp.modulation {
                            1 => 1,
                            2 => 2,
                            _ => 0,
                        };
                        match bp.modulation {
                            1 => "AM".to_string(),
                            2 => "USB".to_string(),
                            _ => "FM".to_string(),
                        }
                    }
                    7 => {
                        self.selection_index = if bp.bandwidth == 1 { 1 } else { 0 };
                        if bp.bandwidth == 1 {
                            "Narrow".to_string()
                        } else {
                            "Wide".to_string()
                        }
                    }
                    _ => String::new(),
                };
            }
        }
    }

    pub fn save_current_bandplan_field(&mut self, field_idx: usize) {
        if let Some(bp) = self.editing_band_plan.as_mut() {
            match field_idx {
                0 => {
                    if let Ok(v) = self.edit_buffer.parse::<u8>() {
                        bp.index = v;
                    }
                }
                1 => {
                    if let Ok(freq) = self.edit_buffer.parse::<u32>() {
                        bp.start_freq = freq * 100000;
                    }
                }
                2 => {
                    if let Ok(freq) = self.edit_buffer.parse::<u32>() {
                        bp.end_freq = freq * 100000;
                    }
                }
                3 => {
                    if let Ok(power) = self.edit_buffer.parse::<u8>() {
                        bp.max_power = power.min(50);
                    }
                }
                4 => bp.tx_allowed = self.selection_index == 1,
                5 => bp.wrap = self.selection_index == 1,
                6 => {
                    bp.modulation = match self.selection_index {
                        1 => 1,
                        2 => 2,
                        _ => 0,
                    };
                }
                7 => bp.bandwidth = if self.selection_index == 1 { 1 } else { 0 },
                _ => {}
            }
        }
    }

    pub fn commit_bandplan_edit(&mut self) {
        if let AppMode::EditBandPlan(field_idx) = self.mode {
            self.save_current_bandplan_field(field_idx);
        }

        if let Some(edited_bp) = self.editing_band_plan.take() {
            if let Some(i) = self.bandplan_state.selected() {
                if i < self.band_plans.len() {
                    self.band_plans[i] = edited_bp;
                    self.status_message = format!("Band plan {} saved", i + 1);
                }
            }
        }
        self.mode = AppMode::Main(MainTab::BandPlan);
    }
}
