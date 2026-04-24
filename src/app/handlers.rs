use super::navigation::{next_item, prev_item, update_selection_after_add};
use super::state::{App, AppEvent, AppMode, MainTab, WriteScope};
use crate::protocol::*;

impl App {
    fn finish_ble_scan_ui_handoff(&mut self) {
        if self.ble_scan_ui_suspended {
            self.resume_ui();
            self.dialog_open = false;
            self.ble_scan_ui_suspended = false;
        }
    }

    pub fn leave_remote_tab(&mut self) {
        if self.remote_active {
            self.remote_off();
        }
        self.mode = AppMode::Main(self.last_non_remote_tab);
        self.last_main_tab = self.last_non_remote_tab;
    }

    pub fn update(&mut self) -> bool {
        let mut updated = false;
        while let Ok(event) = self.event_rx.try_recv() {
            updated = true;
            match event {
                AppEvent::Progress(p) => self.progress = p,
                AppEvent::Status(s) => self.status_message = s,
                AppEvent::Log(l) => self.log(&l),
                AppEvent::BleScanComplete(candidates) => {
                    self.finish_ble_scan_ui_handoff();
                    let current_ble = self
                        .selected_port_candidate()
                        .filter(|candidate| candidate.is_ble())
                        .map(|candidate| candidate.port_name.clone());
                    let current = current_ble
                        .as_deref()
                        .or_else(|| self.ports.get(self.selected_port_index).map(String::as_str))
                        .map(str::to_string);
                    let first_ble = candidates
                        .first()
                        .map(|candidate| candidate.port_name.clone());
                    self.ble_scan_in_progress = false;
                    self.ble_port_candidates = candidates;
                    self.rebuild_port_candidates(current.as_deref());
                    if current_ble.as_ref().is_none_or(|name| {
                        !self
                            .ble_port_candidates
                            .iter()
                            .any(|candidate| &candidate.port_name == name)
                    }) && let Some(first_ble) = first_ble
                        && let Some(index) = self
                            .port_candidates
                            .iter()
                            .position(|candidate| candidate.port_name == first_ble)
                    {
                        self.selected_port_index = index;
                    }
                    self.status_message = match self.ble_port_candidates.len() {
                        0 => "No TD-H3 BLE radios found. Keep Bluetooth enabled on the radio and scan again. On macOS, open NicTUI.app once if permission is needed."
                            .to_string(),
                        1 => format!(
                            "Found 1 BLE radio ({}). Press Enter to connect.",
                            self.ble_port_candidates[0].port_name
                        ),
                        count => format!(
                            "{count} BLE radios found. Select one, then press Enter to connect."
                        ),
                    };
                    self.log(&format!(
                        "BLE scan complete: {} radio(s) found",
                        self.ble_port_candidates.len()
                    ));
                }
                AppEvent::BleScanFailed(error) => {
                    self.finish_ble_scan_ui_handoff();
                    self.ble_scan_in_progress = false;
                    self.log(&format!("BLE scan failed: {error}"));
                    self.status_message = Self::ble_scan_failure_message(
                        &error,
                        !self.ble_port_candidates.is_empty(),
                    );
                }
                AppEvent::ReadChannelsComplete(channels, endian) => {
                    self.channels = channels;
                    self.endian = endian;
                    self.channel_state.select(Some(0));
                    self.ensure_group_selection();
                    self.mode = AppMode::Main(self.last_main_tab);
                    self.status_message =
                        format!("Channels read complete: {} loaded", self.channels.len());
                }
                AppEvent::ReadGroupLabelsComplete(labels) => {
                    self.group_labels = normalize_group_labels(&labels);
                    self.group_labels_dirty = false;
                    if self.scanning_group_state.selected().is_none() {
                        self.scanning_group_state.select(Some(0));
                    }
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
                    self.settings_dirty = false;
                    self.settings_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::Settings);
                    self.status_message = "Settings read complete".to_string();
                }
                AppEvent::RemotePhase(phase) => {
                    self.remote_screen.phase = phase;
                    if matches!(phase, crate::remote::RemoteSessionPhase::Live) {
                        self.remote_active = true;
                    }
                }
                AppEvent::RemoteControl(report) => {
                    self.remote_screen.last_control_report = Some(report.clone());
                    self.status_message = if report.success {
                        if let Some(reaction) = report.reaction.as_ref() {
                            if reaction.deltas > 0 {
                                format!("Remote {} confirmed via {}", report.label, report.strategy)
                            } else if reaction.surfaced_packets > 0
                                || reaction.unknown_packets > 0
                                || reaction.rx_first_ms.is_some()
                            {
                                format!(
                                    "Remote {} sent via {}; telemetry only so far",
                                    report.label, report.strategy
                                )
                            } else {
                                format!(
                                    "Remote {} sent via {}; no reaction yet",
                                    report.label, report.strategy
                                )
                            }
                        } else {
                            format!("Remote {} sent via {}", report.label, report.strategy)
                        }
                    } else {
                        format!("Remote {} failed: {}", report.label, report.detail)
                    };
                    self.log(&format!(
                        "Remote {} [{}] {}",
                        report.label, report.strategy, report.detail
                    ));
                }
                AppEvent::RemoteDelta(delta) => {
                    self.remote_screen.last_delta = Some(delta.clone());
                    self.log(&format!("Remote delta: {delta}"));
                }
                AppEvent::RemotePacket(pkt) => {
                    let now = std::time::Instant::now();
                    match &pkt {
                        RemotePacket::SignalStrength {
                            strength, battery, ..
                        } => {
                            self.remote_screen.signal_strength = *strength;
                            self.remote_screen.battery_level = Some(*battery);
                            self.remote_screen.last_signal_update = Some(now);
                            self.remote_screen.last_battery_update = Some(now);
                        }
                        RemotePacket::NoiseLevel { level, .. } => {
                            self.remote_screen.noise_level = *level;
                            self.remote_screen.last_noise_update = Some(now);
                        }
                        RemotePacket::DisplayText { text, y, .. } => {
                            if *y >= 60 && looks_like_battery_text(text) {
                                self.remote_screen.battery_text = Some(text.clone());
                                self.remote_screen.last_text_update = Some(now);
                            }
                        }
                        RemotePacket::SmallStatus { id, value1, value2 } => {
                            self.remote_screen.last_small_status = Some((*id, *value1, *value2));
                            self.remote_screen.last_status_update = Some(now);
                        }
                        RemotePacket::UnknownFrame { .. } => {
                            self.remote_screen.unknown_packet_count += 1;
                        }
                        _ => {}
                    }

                    match pkt {
                        RemotePacket::SignalStrength { .. } | RemotePacket::NoiseLevel { .. } => {}
                        _ => {
                            self.remote_screen.elements.push_back(pkt);
                            if self.remote_screen.elements.len() > 50 {
                                self.remote_screen.elements.pop_front();
                            }
                        }
                    }
                }
                AppEvent::RemoteStopped { message, failure } => {
                    self.remote_active = false;
                    self.remote_tx = None;
                    self.remote_screen.phase = crate::remote::RemoteSessionPhase::Stopped;
                    self.remote_screen.last_failure = failure;
                    if self.ble_transport_selected() {
                        self.ble_reconnect_required = true;
                    }
                    self.status_message = message;
                }
                AppEvent::WriteComplete(scope) => {
                    self.mode = AppMode::Main(self.last_main_tab);
                    match scope {
                        WriteScope::Channels => {
                            self.channels_dirty = false;
                            self.deleted_channels.clear();
                        }
                        WriteScope::Dtmf => {
                            self.dtmf_dirty = false;
                        }
                        WriteScope::GroupLabels => {
                            self.group_labels_dirty = false;
                        }
                        WriteScope::Settings => {
                            self.settings_dirty = false;
                        }
                        WriteScope::Codeplug => {
                            self.channels_dirty = false;
                            self.deleted_channels.clear();
                            self.dtmf_dirty = false;
                            self.settings_dirty = false;
                            self.group_labels_dirty = false;
                        }
                        WriteScope::None => {}
                    }
                    if self.ble_reconnect_required && self.ble_transport_selected() {
                        self.status_message =
                            "Radio write finished. BLE will reconnect on the next action."
                                .to_string();
                    }
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
                AppEvent::CodeplugDataLoaded {
                    path,
                    data,
                    channels,
                    settings,
                    scan_presets,
                    group_labels,
                } => {
                    self.codeplug_data = Some(data);
                    self.codeplug_path = Some(path);
                    self.channels = channels;
                    self.settings = settings;
                    self.scan_presets = scan_presets;
                    self.group_labels = normalize_group_labels(&group_labels);
                    self.group_labels_dirty = false;
                    self.status_message = format!(
                        "Codeplug loaded: {} channels, {} scan presets, {} named groups",
                        self.channels.len(),
                        self.scan_presets.len(),
                        self.group_labels
                            .iter()
                            .filter(|label| !label.trim().is_empty())
                            .count()
                    );
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
                    if self.ble_transport_selected() {
                        self.ble_reconnect_required = true;
                    }
                    self.mode = AppMode::Error(e);
                }
                AppEvent::SuspendUI => self.suspend_ui(),
                AppEvent::ResumeUI => self.resume_ui(),
            }
        }
        updated
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

    pub fn next_scanning_item(&mut self) {
        if !self.scan_presets.is_empty() {
            next_item(self.scan_presets.len(), &mut self.preset_state);
        }
    }

    pub fn prev_scanning_item(&mut self) {
        if !self.scan_presets.is_empty() {
            prev_item(self.scan_presets.len(), &mut self.preset_state);
        }
    }

    pub fn next_group_item(&mut self) {
        next_item(GROUP_LABEL_COUNT, &mut self.scanning_group_state);
    }

    pub fn prev_group_item(&mut self) {
        prev_item(GROUP_LABEL_COUNT, &mut self.scanning_group_state);
    }

    pub fn ensure_group_selection(&mut self) {
        if self.scanning_group_state.selected().is_none() {
            self.scanning_group_state.select(Some(0));
        }
    }

    pub fn start_edit_scan_preset(&mut self) {
        if let Some(i) = self.preset_state.selected()
            && i < self.scan_presets.len()
            && let Some(sp) = self.scan_presets.get(i).cloned()
        {
            self.last_main_tab = MainTab::Scanning;
            self.editing_scan_preset = Some(sp);
            self.mode = AppMode::EditScanPreset(0);
            self.edit_buffer.clear();
            self.selection_index = 0;
            self.update_scan_preset_edit_buffer();
        }
    }

    pub fn update_scan_preset_edit_buffer(&mut self) {
        if let AppMode::EditScanPreset(field_idx) = self.mode
            && let Some(sp) = self.editing_scan_preset.as_ref()
        {
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

    pub fn save_current_scan_preset_field(&mut self, field_idx: usize) {
        if let Some(sp) = self.editing_scan_preset.as_mut() {
            match field_idx {
                0 => sp.label = self.edit_buffer.clone(),
                1 => {
                    if let Ok(freq) = self.edit_buffer.parse::<f64>() {
                        sp.start_freq = (freq * 100000.0).round() as u32;
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

        if let Some(edited_sp) = self.editing_scan_preset.take()
            && let Some(i) = self.preset_state.selected()
            && i < self.scan_presets.len()
        {
            self.scan_presets[i] = edited_sp;
            self.status_message = format!("Scan preset {} saved", i + 1);
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
            self.selected_port_index = if self.selected_port_index == 0 {
                self.ports.len() - 1
            } else {
                self.selected_port_index - 1
            };
        }
    }

    pub fn next_tab(&mut self) {
        if let AppMode::Main(tab) = self.mode {
            if tab != MainTab::Remote {
                self.last_non_remote_tab = tab;
            }
            if tab == MainTab::Remote {
                self.remote_off();
            }
            let next = match tab {
                MainTab::Channels => MainTab::Settings,
                MainTab::Settings => MainTab::Scanning,
                MainTab::Scanning => MainTab::MemoryGroups,
                MainTab::MemoryGroups => MainTab::BandPlan,
                MainTab::BandPlan => MainTab::DTMF,
                MainTab::DTMF => MainTab::Remote,
                MainTab::Remote => MainTab::Codeplug,
                MainTab::Codeplug => MainTab::BinFlash,
                MainTab::BinFlash => MainTab::Debug,
                MainTab::Debug => MainTab::Channels,
            };
            self.mode = AppMode::Main(next);
            self.last_main_tab = next;
            if next != MainTab::Remote {
                self.last_non_remote_tab = next;
            }
        }
    }

    pub fn prev_tab(&mut self) {
        if let AppMode::Main(tab) = self.mode {
            if tab != MainTab::Remote {
                self.last_non_remote_tab = tab;
            }
            if tab == MainTab::Remote {
                self.remote_off();
            }
            let prev = match tab {
                MainTab::Channels => MainTab::Debug,
                MainTab::Settings => MainTab::Channels,
                MainTab::Scanning => MainTab::Settings,
                MainTab::MemoryGroups => MainTab::Scanning,
                MainTab::BandPlan => MainTab::MemoryGroups,
                MainTab::DTMF => MainTab::BandPlan,
                MainTab::Remote => MainTab::DTMF,
                MainTab::Codeplug => MainTab::Remote,
                MainTab::BinFlash => MainTab::Codeplug,
                MainTab::Debug => MainTab::BinFlash,
            };
            self.mode = AppMode::Main(prev);
            self.last_main_tab = prev;
            if prev != MainTab::Remote {
                self.last_non_remote_tab = prev;
            }
        }
    }

    pub fn start_edit_channel(&mut self) {
        if let Some(i) = self.channel_state.selected()
            && let Some(ch) = self.channels.get(i).cloned()
        {
            self.last_main_tab = MainTab::Channels;
            self.pending_channel_edit = Some(ch);
            self.mode = AppMode::EditChannel(0);
            self.edit_buffer.clear();
            self.selection_index = 0;
            self.update_edit_buffer();
        }
    }

    pub fn update_edit_buffer(&mut self) {
        if let AppMode::EditChannel(field_idx) = self.mode
            && let Some(ch) = self.pending_channel_edit.as_ref()
        {
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
                    let group = ch
                        .groups
                        .iter()
                        .copied()
                        .find(|group| (1..=GROUP_LABEL_COUNT as u8).contains(group))
                        .unwrap_or(0);
                    self.selection_index = if (1..=GROUP_LABEL_COUNT as u8).contains(&group) {
                        group as usize
                    } else {
                        0
                    };
                    self.group_option_label(group)
                }
                10 => ch.channel_num.to_string(),
                _ => String::new(),
            };
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
                    let selected_group = if self.selection_index == 0 {
                        0
                    } else {
                        self.selection_index as u8
                    };
                    ch.groups = [selected_group, 0, 0, 0];
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
        if let AppMode::EditChannel(field_idx) = self.mode {
            self.save_current_field_to_pending(field_idx);
        }

        if let Some(pending_ch) = self.pending_channel_edit.take() {
            let new_channel_num = pending_ch.channel_num;

            if let Some(i) = self.channel_state.selected()
                && i < self.channels.len()
            {
                let duplicate = self
                    .channels
                    .iter()
                    .enumerate()
                    .any(|(idx, c)| idx != i && c.channel_num == new_channel_num);

                if duplicate {
                    self.status_message = format!("Channel {} already exists!", new_channel_num);
                } else {
                    let changed = channel_changed(&self.channels[i], &pending_ch);
                    self.channels[i] = pending_ch;
                    self.channel_state.select(Some(i));

                    if changed {
                        self.channels_dirty = true;
                        self.status_message =
                            format!("Channel {} saved (Unsaved)", new_channel_num);
                    } else {
                        self.status_message = format!("Channel {} saved", new_channel_num);
                    }
                }
            }
            self.mode = AppMode::Main(MainTab::Channels);
        }
    }

    pub fn renumber_channels(&mut self) {
        for (j, c) in self.channels.iter_mut().enumerate() {
            c.channel_num = (j + 1) as u16;
        }
    }

    fn group_option_label(&self, group: u8) -> String {
        if group == 0 || group == 0xFF {
            "None".to_string()
        } else if let Some(label) = group_label(&self.group_labels, group) {
            label.to_string()
        } else if let Some(letter) = group_letter(group) {
            letter.to_string()
        } else {
            group.to_string()
        }
    }

    pub fn save_current_dtmf_field_to_pending(&mut self, field_idx: usize) {
        if let Some(preset_idx) = self.dtmf_edit_preset_idx
            && let Some(dtmf) = self.dtmf_presets.get_mut(preset_idx)
        {
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

    pub fn update_dtmf_edit_buffer(&mut self) {
        if let AppMode::EditDTMF(field_idx) = self.mode
            && let Some(preset_idx) = self.dtmf_edit_preset_idx
            && let Some(dtmf) = self.dtmf_presets.get(preset_idx)
        {
            self.edit_buffer = match field_idx {
                0 => dtmf.label.clone(),
                1 => dtmf.digits.iter().map(|d| format!("{:X}", d)).collect(),
                _ => String::new(),
            };
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
        if let Some(i) = self.channel_state.selected()
            && i < self.channels.len()
        {
            let ch_num = self.channels[i].channel_num;
            if !self.deleted_channels.contains(&ch_num) {
                self.deleted_channels.push(ch_num);
                self.channels_dirty = true;
                self.status_message = format!("Channel {} marked for deletion (Unsaved)", ch_num);
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
        if let Some(i) = self.channel_state.selected()
            && i < self.channels.len()
        {
            let ch_num = self.channels[i].channel_num;
            if let Some(pos) = self.deleted_channels.iter().position(|&x| x == ch_num) {
                self.deleted_channels.remove(pos);
                self.channels_dirty = !self.deleted_channels.is_empty();
                self.status_message = format!("Channel {} undeleted", ch_num);
            }
        }
    }

    pub fn change_channel_index(&mut self, new_index: u16) {
        if let Some(i) = self.channel_state.selected()
            && i < self.channels.len()
        {
            let old_index = self.channels[i].channel_num;
            if new_index != old_index {
                self.channels[i].channel_num = new_index;
                self.channels_dirty = true;
                self.status_message = format!("Channel {} -> {} (Unsaved)", old_index, new_index);
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
    }

    pub fn start_edit_bandplan(&mut self) {
        if let Some(i) = self.bandplan_state.selected()
            && i < self.band_plans.len()
            && let Some(bp) = self.band_plans.get(i).cloned()
        {
            self.last_main_tab = MainTab::BandPlan;
            self.editing_band_plan = Some(bp);
            self.mode = AppMode::EditBandPlan(0);
            self.edit_buffer.clear();
            self.selection_index = 0;
            self.update_bandplan_edit_buffer();
        }
    }

    pub fn update_bandplan_edit_buffer(&mut self) {
        if let AppMode::EditBandPlan(field_idx) = self.mode
            && let Some(bp) = self.editing_band_plan.as_ref()
        {
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

    pub fn save_current_bandplan_field(&mut self, field_idx: usize) {
        if let Some(bp) = self.editing_band_plan.as_mut() {
            match field_idx {
                0 => {
                    if let Ok(v) = self.edit_buffer.parse::<u8>() {
                        bp.index = v;
                    }
                }
                1 => {
                    if let Ok(freq) = self.edit_buffer.parse::<f64>() {
                        bp.start_freq = (freq * 100000.0).round() as u32;
                    }
                }
                2 => {
                    if let Ok(freq) = self.edit_buffer.parse::<f64>() {
                        bp.end_freq = (freq * 100000.0).round() as u32;
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

        if let Some(edited_bp) = self.editing_band_plan.take()
            && let Some(i) = self.bandplan_state.selected()
            && i < self.band_plans.len()
        {
            self.band_plans[i] = edited_bp;
            self.status_message = format!("Band plan {} saved", i + 1);
        }
        self.mode = AppMode::Main(MainTab::BandPlan);
    }
}

fn looks_like_battery_text(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.ends_with('V')
        && trimmed
            .strip_suffix('V')
            .is_some_and(|value| value.parse::<f32>().is_ok())
}

fn channel_changed(left: &Channel, right: &Channel) -> bool {
    left.channel_num != right.channel_num
        || left.name != right.name
        || left.rx_freq != right.rx_freq
        || left.tx_freq != right.tx_freq
        || left.rx_tone != right.rx_tone
        || left.tx_tone != right.tx_tone
        || left.power != right.power
        || left.bandwidth != right.bandwidth
        || left.modulation != right.modulation
        || left.reverse != right.reverse
        || left.busy_lock != right.busy_lock
        || left.groups != right.groups
        || left.ptt_id != right.ptt_id
        || left.position != right.position
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn write_complete_clears_only_matching_dirty_scope() {
        let mut app = App::new();
        app.channels_dirty = true;
        app.deleted_channels.push(7);
        app.dtmf_dirty = true;
        app.group_labels_dirty = true;
        app.settings_dirty = true;

        app.event_tx
            .send(AppEvent::WriteComplete(WriteScope::Settings))
            .unwrap();

        assert!(app.update());
        assert!(app.channels_dirty);
        assert_eq!(app.deleted_channels, vec![7]);
        assert!(app.dtmf_dirty);
        assert!(app.group_labels_dirty);
        assert!(!app.settings_dirty);
    }

    #[test]
    fn committing_any_changed_channel_field_marks_channels_dirty() {
        let mut app = App::new();
        let channel = Channel {
            channel_num: 1,
            name: "OLD".to_string(),
            ..Channel::default()
        };
        app.channels = vec![channel.clone()];
        app.channel_state.select(Some(0));
        app.pending_channel_edit = Some(channel);
        app.mode = AppMode::EditChannel(0);
        app.edit_buffer = "NEW".to_string();

        app.commit_edit();

        assert!(app.channels_dirty);
        assert_eq!(app.channels[0].name, "NEW");
    }

    #[test]
    fn remote_off_keeps_session_active_until_stop_event() {
        let mut app = App::new();
        app.remote_active = true;
        app.remote_stop_signal.store(false, Ordering::SeqCst);

        app.remote_off();

        assert!(app.remote_active);
        assert!(app.remote_stop_signal.load(Ordering::SeqCst));

        app.remote_on();

        assert_eq!(app.status_message, "Remote mode is still stopping");
    }
}
