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
        let mut updated = self.poll_activity();
        while let Ok(event) = self.event_rx.try_recv() {
            updated = true;
            match event {
                AppEvent::Progress(p) => self.progress = p,
                AppEvent::Status(s) => self.status_message = s,
                AppEvent::Log(l) => self.log(&l),
                AppEvent::BleScanComplete(candidates) => {
                    self.finish_ble_scan_ui_handoff();
                    let can_select_discovered =
                        self.mode == AppMode::PortSelection && self.protocol_port_name.is_none();
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
                    if can_select_discovered
                        && current_ble.as_ref().is_none_or(|name| {
                            !self
                                .ble_port_candidates
                                .iter()
                                .any(|candidate| &candidate.port_name == name)
                        })
                        && let Some(first_ble) = first_ble
                        && let Some(index) = self
                            .port_candidates
                            .iter()
                            .position(|candidate| candidate.port_name == first_ble)
                    {
                        self.selected_port_index = index;
                    }
                    if can_select_discovered {
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
                    }
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
                    self.channel_state.select(if self.channels.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                    self.channels_dirty = false;
                    self.deleted_channels.clear();
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
                    self.scan_presets_dirty = false;
                    self.preset_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::Scanning);
                    self.status_message = "Presets read complete".to_string();
                }
                AppEvent::ReadBandPlanComplete(plans) => {
                    self.band_plans = plans;
                    self.band_plans_dirty = false;
                    self.bandplan_state.select(Some(0));
                    self.mode = AppMode::Main(MainTab::BandPlan);
                    self.status_message = "BandPlan read complete".to_string();
                }
                AppEvent::ReadDTMFComplete(presets) => {
                    self.dtmf_presets = presets;
                    self.dtmf_state.select(if self.dtmf_presets.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                    self.dtmf_dirty = false;
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
                    if matches!(
                        phase,
                        crate::remote::RemoteSessionPhase::Opening
                            | crate::remote::RemoteSessionPhase::Recovering
                    ) {
                        self.remote_screen.reset_session();
                    }
                    self.remote_screen.phase = phase;
                    if matches!(phase, crate::remote::RemoteSessionPhase::Live) {
                        self.remote_active = true;
                    }
                }
                AppEvent::RemoteControl(report) => {
                    self.remote_screen.last_control_report = Some(report.clone());
                    self.status_message = if !report.success
                        || report.evidence == crate::remote::RemoteEvidenceKind::CommandFailed
                    {
                        format!("{} failed; check the radio connection", report.label)
                    } else {
                        match report.evidence {
                            crate::remote::RemoteEvidenceKind::ControlConfirmed => {
                                format!("{} confirmed by the radio", report.label)
                            }
                            crate::remote::RemoteEvidenceKind::NoControlEvidence => {
                                format!("Radio updates received; {} not confirmed", report.label)
                            }
                            _ => format!("{} sent; awaiting radio response", report.label),
                        }
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
                AppEvent::RemoteReceived(received_at) => {
                    self.remote_screen.last_received = Some(
                        self.remote_screen
                            .last_received
                            .map_or(received_at, |last| last.max(received_at)),
                    );
                }
                AppEvent::RemotePacket(pkt) => {
                    let now = std::time::Instant::now();
                    self.remote_screen.display.apply(&pkt);
                    if matches!(
                        &pkt,
                        RemotePacket::DisplayText { .. }
                            | RemotePacket::DrawRectangle { .. }
                            | RemotePacket::DrawSymbol { .. }
                    ) {
                        self.remote_screen.has_display_updates = true;
                    }
                    match &pkt {
                        RemotePacket::SignalStrength { strength, .. } => {
                            self.remote_screen.signal_strength = *strength;
                            self.remote_screen.last_signal_update = Some(now);
                        }
                        RemotePacket::NoiseLevel { level, .. } => {
                            self.remote_screen.noise_level = *level;
                            self.remote_screen.last_noise_update = Some(now);
                        }
                        RemotePacket::DisplayText { text, y, .. } => {
                            self.remote_screen.last_text_update = Some(now);
                            if *y >= 60 && looks_like_battery_text(text) {
                                self.remote_screen.battery_text = Some(text.clone());
                                self.remote_screen.last_battery_update = Some(now);
                            }
                        }
                        RemotePacket::SmallStatus { id, value1, value2 } => {
                            self.remote_screen.last_small_status = Some((*id, *value1, *value2));
                            self.remote_screen.last_status_update = Some(now);
                            self.remote_screen.leds = id & 0x0F;
                            self.remote_screen.last_led_update = Some(now);
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
                    *self.remote_input_state.lock().unwrap() = Default::default();
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
                        WriteScope::ScanPresets => self.scan_presets_dirty = false,
                        WriteScope::BandPlans => self.band_plans_dirty = false,
                        WriteScope::Codeplug => {}
                        WriteScope::None => {}
                    }
                    if self.ble_reconnect_required && self.ble_transport_selected() {
                        self.status_message =
                            "Radio write finished. BLE will reconnect on the next action."
                                .to_string();
                    }
                }
                AppEvent::LoadCSV(path) => self.start_import_channels(path),
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
                    self.codeplug_workspace_baseline = self.workspace_snapshot();
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
                    self.endian =
                        crate::protocol::codeplug::infer_channel_endianness_from_codeplug(&data);
                    self.band_plans = crate::protocol::codeplug::extract_band_plans_from_codeplug(
                        &data,
                        self.endian,
                    );
                    self.dtmf_presets =
                        crate::protocol::codeplug::extract_dtmf_presets_from_codeplug(&data);
                    self.codeplug_data = Some(data);
                    self.codeplug_path = Some(path);
                    self.channels = channels;
                    self.settings = settings;
                    self.scan_presets = scan_presets;
                    self.group_labels = normalize_group_labels(&group_labels);
                    self.group_labels_dirty = false;
                    self.channels_dirty = false;
                    self.settings_dirty = false;
                    self.dtmf_dirty = false;
                    self.scan_presets_dirty = false;
                    self.band_plans_dirty = false;
                    self.deleted_channels.clear();
                    self.channel_state.select(if self.channels.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                    self.preset_state.select(if self.scan_presets.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                    self.bandplan_state.select(if self.band_plans.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                    self.dtmf_state.select(if self.dtmf_presets.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                    self.codeplug_workspace_baseline = self.workspace_snapshot();
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

    pub fn next_port(&mut self) {
        if self.activity_active() {
            self.hardware_action_allowed();
            return;
        }
        if !self.ports.is_empty() {
            self.selected_port_index = (self.selected_port_index + 1) % self.ports.len();
        }
    }

    pub fn previous_port(&mut self) {
        if self.activity_active() {
            self.hardware_action_allowed();
            return;
        }
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
                MainTab::Debug => MainTab::Activity,
                MainTab::Activity => MainTab::Channels,
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
                MainTab::Channels => MainTab::Activity,
                MainTab::Settings => MainTab::Channels,
                MainTab::Scanning => MainTab::Settings,
                MainTab::MemoryGroups => MainTab::Scanning,
                MainTab::BandPlan => MainTab::MemoryGroups,
                MainTab::DTMF => MainTab::BandPlan,
                MainTab::Remote => MainTab::DTMF,
                MainTab::Codeplug => MainTab::Remote,
                MainTab::BinFlash => MainTab::Codeplug,
                MainTab::Debug => MainTab::BinFlash,
                MainTab::Activity => MainTab::Debug,
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
                    self.selection_index = usize::from(ch.reverse);
                    if ch.reverse {
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
                        "FM" => 1,
                        "AM" => 2,
                        "USB" => 3,
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
                    if val_str.eq_ignore_ascii_case("Off") {
                        ch.power = 0;
                    } else if let Ok(power) = val_str.parse::<u8>() {
                        ch.power = power;
                    } else {
                        self.status_message = "Invalid power: enter Off or an integer from 0 to 255; previous value retained".to_string();
                    }
                }
                6 => ch.reverse = self.selection_index == 1,
                7 => {
                    ch.bandwidth = if self.selection_index == 1 {
                        "Narrow".to_string()
                    } else {
                        "Wide".to_string()
                    }
                }
                8 => {
                    ch.modulation = match self.selection_index {
                        0 => "Auto".to_string(),
                        1 => "FM".to_string(),
                        2 => "AM".to_string(),
                        3 => "USB".to_string(),
                        _ => {
                            self.status_message =
                                "Invalid modulation: choose Auto, FM, AM, or USB".to_string();
                            return;
                        }
                    }
                }
                9 => {
                    if self.selection_index > 15 {
                        self.status_message =
                            "Invalid group: radio supports groups 0 through 15".to_string();
                        return;
                    }
                    let slot = ch
                        .groups
                        .iter()
                        .position(|group| (1..=15).contains(group))
                        .unwrap_or(0);
                    ch.groups[slot] = self.selection_index as u8;
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
            if field_idx == 5
                && !self.edit_buffer.trim().eq_ignore_ascii_case("Off")
                && self.edit_buffer.trim().parse::<u8>().is_err()
            {
                self.status_message =
                    "Invalid power: enter Off or an integer from 0 to 255".to_string();
                return;
            }
            if (field_idx == 8 && self.selection_index > 3)
                || (field_idx == 9 && self.selection_index > 15)
            {
                self.status_message = "Invalid radio option; choose a supported value".to_string();
                return;
            }
            if field_idx == 10 && self.edit_buffer.parse::<u16>().is_err() {
                self.status_message =
                    "Invalid channel number: enter an integer from 1 to 198".to_string();
                return;
            }
            self.save_current_field_to_pending(field_idx);
        }

        let Some(pending_ch) = self.pending_channel_edit.as_ref() else {
            return;
        };
        if let Err(error) = crate::device::validate_channels(std::slice::from_ref(pending_ch)) {
            self.status_message = format!("Invalid channel: {error}");
            return;
        }
        let Some(i) = self
            .channel_state
            .selected()
            .filter(|&i| i < self.channels.len())
        else {
            self.status_message =
                "Channel selection is no longer valid; cancel and reopen the editor".to_string();
            return;
        };
        let new_channel_num = pending_ch.channel_num;
        if self
            .channels
            .iter()
            .enumerate()
            .any(|(idx, channel)| idx != i && channel.channel_num == new_channel_num)
        {
            self.status_message = format!("Channel {} already exists!", new_channel_num);
            return;
        }
        let changed = channel_changed(&self.channels[i], pending_ch);
        if let Some(pending_ch) = self.pending_channel_edit.take() {
            self.channels[i] = pending_ch;
        }
        self.channel_state.select(Some(i));
        if changed {
            self.channels_dirty = true;
            self.status_message = format!("Channel {} saved (Unsaved)", new_channel_num);
        } else {
            self.status_message = format!("Channel {} saved", new_channel_num);
        }
        self.mode = AppMode::Main(MainTab::Channels);
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
                self.channels_dirty = true;
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
    fn scan_and_band_write_completion_clears_only_its_own_scope() {
        for (scope, tab) in [
            (WriteScope::ScanPresets, MainTab::Scanning),
            (WriteScope::BandPlans, MainTab::BandPlan),
        ] {
            let mut app = App::new();
            app.last_main_tab = tab;
            app.mode = AppMode::Writing;
            app.scan_presets_dirty = true;
            app.band_plans_dirty = true;
            app.channels_dirty = true;
            app.settings_dirty = true;
            app.dtmf_dirty = true;
            app.group_labels_dirty = true;
            app.deleted_channels.push(1);
            app.event_tx.send(AppEvent::WriteComplete(scope)).unwrap();
            app.update();
            assert_eq!(app.mode, AppMode::Main(tab));
            assert_eq!(app.scan_presets_dirty, scope != WriteScope::ScanPresets);
            assert_eq!(app.band_plans_dirty, scope != WriteScope::BandPlans);
            assert!(
                app.channels_dirty
                    && app.settings_dirty
                    && app.dtmf_dirty
                    && app.group_labels_dirty
            );
            assert_eq!(app.deleted_channels, vec![1]);
        }
    }

    #[test]
    fn scan_and_band_reads_reset_only_their_own_dirty_flags() {
        let mut app = App::new();
        app.scan_presets_dirty = true;
        app.band_plans_dirty = true;
        app.event_tx
            .send(AppEvent::ReadPresetsComplete(Vec::new()))
            .unwrap();
        app.update();
        assert!(!app.scan_presets_dirty);
        assert!(app.band_plans_dirty);
        app.scan_presets_dirty = true;
        app.event_tx
            .send(AppEvent::ReadBandPlanComplete(Vec::new()))
            .unwrap();
        app.update();
        assert!(app.scan_presets_dirty);
        assert!(!app.band_plans_dirty);
    }

    #[test]
    fn failed_section_writes_retain_dirty_state() {
        let mut app = App::new();
        app.scan_presets_dirty = true;
        app.band_plans_dirty = true;
        app.mode = AppMode::Writing;
        app.event_tx
            .send(AppEvent::Error("Radio rejected write".to_string()))
            .unwrap();
        app.update();
        assert!(app.scan_presets_dirty && app.band_plans_dirty);
        assert!(matches!(app.mode, AppMode::Error(_)));
    }

    fn valid_channel() -> Channel {
        Channel {
            channel_num: 1,
            name: "TEST".to_string(),
            rx_freq: "145.35000".to_string(),
            tx_freq: "0".to_string(),
            rx_tone: "Off".to_string(),
            tx_tone: "Off".to_string(),
            bandwidth: "Wide".to_string(),
            modulation: "FM".to_string(),
            ..Channel::default()
        }
    }

    #[test]
    fn undelete_preserves_unrelated_channel_edits() {
        let mut app = App::new();
        let channel = valid_channel();
        app.channels = vec![
            channel.clone(),
            Channel {
                channel_num: 2,
                ..channel
            },
        ];
        app.channel_state.select(Some(0));
        app.start_edit_channel();
        app.edit_buffer = "CHANGED".to_string();
        app.commit_edit();
        assert!(app.channels_dirty);

        app.channel_state.select(Some(1));
        app.delete_selected_channel();
        app.undelete_channel();
        assert!(app.deleted_channels.is_empty());
        assert!(app.channels_dirty);
        assert_eq!(app.channels[0].name, "CHANGED");
    }

    #[test]
    fn undoing_the_only_deletion_conservatively_keeps_dirty_state() {
        let mut app = App::new();
        app.channels = vec![valid_channel()];
        app.channel_state.select(Some(0));
        app.delete_selected_channel();
        app.undelete_channel();
        assert!(app.deleted_channels.is_empty());
        assert!(app.channels_dirty);
    }

    #[test]
    fn undelete_without_a_matching_deletion_does_not_dirty_workspace() {
        let mut app = App::new();
        app.channels = vec![valid_channel()];
        app.channel_state.select(Some(0));
        app.undelete_channel();
        assert!(!app.channels_dirty);
    }

    #[test]
    fn invalid_channel_commits_keep_editor_and_draft_without_changing_workspace() {
        for (field, invalid) in [
            (0, "NAME TOO LONG"),
            (0, "caf\u{e9}"),
            (1, "not a frequency"),
            (1, "NaN"),
            (1, "0"),
            (1, "145.000001"),
            (2, "1301"),
            (3, "D089n"),
            (4, "301"),
            (10, "199"),
            (10, "invalid"),
        ] {
            let mut app = App::new();
            app.channels = vec![valid_channel()];
            app.channel_state.select(Some(0));
            app.start_edit_channel();
            app.mode = AppMode::EditChannel(field);
            app.edit_buffer = invalid.to_string();
            app.commit_edit();
            assert_eq!(app.mode, AppMode::EditChannel(field), "{field}: {invalid}");
            assert!(app.pending_channel_edit.is_some());
            assert_eq!(app.edit_buffer, invalid);
            assert_eq!(app.channels[0].name, "TEST");
            assert_eq!(app.channels[0].rx_freq, "145.35000");
            assert_eq!(app.channels[0].tx_freq, "0");
            assert_eq!(app.channels[0].rx_tone, "Off");
            assert_eq!(app.channels[0].tx_tone, "Off");
            assert_eq!(app.channels[0].channel_num, 1);
            assert!(!app.channels_dirty);
            assert!(
                app.status_message.contains("Invalid"),
                "{}",
                app.status_message
            );
        }
    }

    #[test]
    fn duplicate_channel_numbers_keep_pending_draft_open() {
        let mut app = App::new();
        let channel = valid_channel();
        app.channels = vec![
            channel.clone(),
            Channel {
                channel_num: 2,
                ..channel
            },
        ];
        app.channel_state.select(Some(0));
        app.start_edit_channel();
        app.mode = AppMode::EditChannel(10);
        app.edit_buffer = "2".to_string();
        app.commit_edit();
        assert_eq!(app.mode, AppMode::EditChannel(10));
        assert_eq!(app.pending_channel_edit.as_ref().unwrap().channel_num, 2);
        assert_eq!(app.channels[0].channel_num, 1);
        assert!(!app.channels_dirty);
        assert!(app.status_message.contains("already exists"));
    }

    #[test]
    fn stale_channel_selection_does_not_discard_a_valid_pending_draft() {
        let mut app = App::new();
        app.channels = vec![valid_channel()];
        app.channel_state.select(Some(0));
        app.start_edit_channel();
        app.channel_state.select(Some(usize::MAX));
        app.edit_buffer = "CHANGED".to_string();
        app.commit_edit();
        assert_eq!(app.mode, AppMode::EditChannel(0));
        assert!(app.pending_channel_edit.is_some());
        assert_eq!(app.channels[0].name, "TEST");
        assert!(!app.channels_dirty);
    }

    #[test]
    fn invalid_deleted_records_do_not_block_valid_pending_channel_commits() {
        let mut app = App::new();
        app.channels = vec![
            valid_channel(),
            Channel {
                channel_num: 2,
                ..Channel::default()
            },
        ];
        app.deleted_channels.push(2);
        app.channel_state.select(Some(0));
        app.start_edit_channel();
        app.edit_buffer = "CHANGED".to_string();
        app.commit_edit();
        assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
        assert!(app.pending_channel_edit.is_none());
        assert_eq!(app.channels[0].name, "CHANGED");
        assert!(app.channels_dirty);
        assert_eq!(app.deleted_channels, vec![2]);
    }

    #[test]
    fn discovery_does_not_retarget_an_active_usb_workspace() {
        let mut app = App::demo();
        app.demo_mode = false;
        let mut usb = app.port_candidates[0].clone();
        usb.port_name = "/dev/test-usb".to_string();
        usb.kind = crate::device::PortKind::Radio;
        usb.ble_device_id = None;
        app.active_port_candidate = Some(usb.clone());
        app.serial_port_candidates = vec![usb];
        app.protocol_port_name = Some("/dev/test-usb".to_string());
        app.status_message = "Workspace active".to_string();
        let ble = app.ble_port_candidates[0].clone();
        app.event_tx
            .send(AppEvent::BleScanComplete(vec![ble]))
            .unwrap();
        app.update();
        assert_eq!(app.selected_port_label(), "/dev/test-usb");
        assert!(!app.ble_transport_selected());
        assert_eq!(app.protocol_port_name.as_deref(), Some("/dev/test-usb"));
        assert_eq!(app.status_message, "Workspace active");
    }

    #[test]
    fn active_ble_identity_survives_disappearance_from_scan_results() {
        let mut app = App::demo();
        app.demo_mode = false;
        app.event_tx
            .send(AppEvent::BleScanComplete(Vec::new()))
            .unwrap();
        app.update();
        assert!(app.port_candidates.is_empty());
        assert_eq!(app.selected_ble_device_id(), Some("demo-td-h3"));
        assert_eq!(app.selected_port_label(), "TD-H3 #6CAF");
    }

    #[test]
    fn empty_and_stale_dtmf_and_bandplan_navigation_is_safe() {
        let directions: &[fn(&mut App)] = &[
            App::next_dtmf,
            App::prev_dtmf,
            App::next_bandplan,
            App::prev_bandplan,
        ];
        for direction in directions {
            let mut app = App::new();
            app.dtmf_state.select(Some(usize::MAX));
            app.bandplan_state.select(Some(usize::MAX));
            direction(&mut app);
            assert!(app.dtmf_state.selected().is_none() || app.bandplan_state.selected().is_none());
        }
    }

    #[test]
    fn reverse_editor_round_trips_without_changing_position() {
        for position in [0, 1] {
            for reverse in [false, true] {
                let mut app = App::new();
                let channel = Channel {
                    reverse,
                    position,
                    ..valid_channel()
                };
                app.pending_channel_edit = Some(channel.clone());
                app.mode = AppMode::EditChannel(6);
                app.update_edit_buffer();
                assert_eq!(app.selection_index, usize::from(reverse));
                assert_eq!(app.edit_buffer, if reverse { "On" } else { "Off" });
                app.save_current_field_to_pending(6);
                let pending = app.pending_channel_edit.as_ref().unwrap();
                assert_eq!(pending.reverse, reverse);
                assert_eq!(pending.position, position);
                assert!(!channel_changed(&channel, pending));
            }
        }
    }

    #[test]
    fn reverse_editor_commit_changes_only_the_reverse_wire_bit() {
        for endian in [
            crate::protocol::Endianness::Little,
            crate::protocol::Endianness::Big,
        ] {
            for position in [0, 1] {
                for reverse in [false, true] {
                    let mut app = App::new();
                    let channel = Channel {
                        position,
                        reverse,
                        busy_lock: true,
                        ptt_id: 2,
                        ..valid_channel()
                    };
                    let before = crate::protocol::RadioProtocol::pack_channel(&channel, endian);
                    app.channels = vec![channel];
                    app.channel_state.select(Some(0));
                    app.start_edit_channel();
                    app.mode = AppMode::EditChannel(6);
                    app.update_edit_buffer();
                    app.selection_index = usize::from(!reverse);
                    app.commit_edit();

                    assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
                    assert_eq!(app.channels[0].reverse, !reverse);
                    assert_eq!(app.channels[0].position, position);
                    assert!(app.channels_dirty);
                    assert!(app.deleted_channels.is_empty());
                    let mut expected = before;
                    expected[15] ^= 0x40;
                    assert_eq!(
                        crate::protocol::RadioProtocol::pack_channel(&app.channels[0], endian),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn modulation_editor_round_trips_all_supported_modes() {
        for (index, mode) in ["Auto", "FM", "AM", "USB"].into_iter().enumerate() {
            let mut app = App::new();
            app.pending_channel_edit = Some(Channel {
                modulation: mode.to_string(),
                ..Channel::default()
            });
            app.mode = AppMode::EditChannel(8);
            app.update_edit_buffer();
            assert_eq!(app.selection_index, index);
            app.save_current_field_to_pending(8);
            assert_eq!(app.pending_channel_edit.as_ref().unwrap().modulation, mode);
        }
    }

    #[test]
    fn visiting_group_field_preserves_all_memberships() {
        for groups in [[1, 2, 3, 4], [0, 2, 3, 0], [0, 0, 0, 0]] {
            let mut app = App::new();
            app.pending_channel_edit = Some(Channel {
                groups,
                ..Channel::default()
            });
            app.mode = AppMode::EditChannel(9);
            app.update_edit_buffer();
            app.save_current_field_to_pending(9);
            assert_eq!(app.pending_channel_edit.as_ref().unwrap().groups, groups);
        }
    }

    #[test]
    fn changing_primary_group_preserves_other_memberships() {
        let mut app = App::new();
        app.pending_channel_edit = Some(Channel {
            groups: [0, 2, 3, 4],
            ..Channel::default()
        });
        app.selection_index = 5;
        app.save_current_field_to_pending(9);
        assert_eq!(
            app.pending_channel_edit.as_ref().unwrap().groups,
            [0, 5, 3, 4]
        );
    }

    #[test]
    fn invalid_power_never_silently_disables_transmit() {
        for invalid in ["", "invalid", "256", "-1"] {
            let mut app = App::new();
            app.pending_channel_edit = Some(Channel {
                power: 128,
                ..Channel::default()
            });
            app.mode = AppMode::EditChannel(5);
            app.edit_buffer = invalid.to_string();
            app.save_current_field_to_pending(5);
            assert_eq!(app.pending_channel_edit.as_ref().unwrap().power, 128);
            assert!(app.status_message.contains("Invalid power"));
            app.commit_edit();
            assert_eq!(app.mode, AppMode::EditChannel(5));
            assert!(app.pending_channel_edit.is_some());
        }
    }

    #[test]
    fn snapshot_completion_cannot_clear_unrelated_workspace_edits() {
        let mut app = App::new();
        app.channels_dirty = true;
        app.settings_dirty = true;
        app.dtmf_dirty = true;
        app.group_labels_dirty = true;
        app.deleted_channels.push(1);
        app.event_tx
            .send(AppEvent::WriteComplete(WriteScope::Codeplug))
            .unwrap();
        app.update();
        assert!(
            app.channels_dirty && app.settings_dirty && app.dtmf_dirty && app.group_labels_dirty
        );
        assert_eq!(app.deleted_channels, vec![1]);
    }

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
            ..valid_channel()
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
    fn signal_meter_does_not_fabricate_battery_and_leds_decode_opcode() {
        let mut app = App::new();
        app.event_tx
            .send(AppEvent::RemotePacket(RemotePacket::SignalStrength {
                strength: 36,
                mode: 0,
                battery: 0,
            }))
            .unwrap();
        app.event_tx
            .send(AppEvent::RemotePacket(RemotePacket::SmallStatus {
                id: 0x76,
                value1: 0,
                value2: 0,
            }))
            .unwrap();
        assert!(app.update());
        assert_eq!(app.remote_screen.battery_level, None);
        assert_eq!(app.remote_screen.last_battery_update, None);
        assert_eq!(app.remote_screen.leds, 6);
        assert!(app.remote_screen.last_led_update.is_some());
    }

    #[test]
    fn display_survives_activity_history_eviction() {
        fn snapshot(app: &App) -> ratatui::buffer::Buffer {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(64, 32)).unwrap();
            terminal
                .draw(|frame| app.remote_screen.display.render(frame, frame.area()))
                .unwrap();
            terminal.backend().buffer().clone()
        }
        let mut app = App::new();
        app.event_tx
            .send(AppEvent::RemotePacket(RemotePacket::DrawRectangle {
                x: 0,
                y: 0,
                width: 32,
                height: 32,
                color: 0x001F,
            }))
            .unwrap();
        app.update();
        let painted = snapshot(&app);
        for _ in 0..60 {
            app.event_tx
                .send(AppEvent::RemotePacket(RemotePacket::SmallStatus {
                    id: 0x70,
                    value1: 0,
                    value2: 0,
                }))
                .unwrap();
        }
        app.update();
        assert_eq!(app.remote_screen.elements.len(), 50);
        assert_eq!(snapshot(&app), painted);
        app.event_tx
            .send(AppEvent::RemotePacket(RemotePacket::DrawRectangle {
                x: 0,
                y: 0,
                width: 32,
                height: 32,
                color: 0,
            }))
            .unwrap();
        app.update();
        assert_ne!(snapshot(&app), painted);
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

    #[test]
    fn receive_ticks_preserve_arrival_time_without_changing_content_timestamps() {
        let mut app = App::new();
        let received = std::time::Instant::now() - std::time::Duration::from_secs(4);
        app.event_tx
            .send(AppEvent::RemoteReceived(received))
            .unwrap();
        assert!(app.update());
        assert_eq!(app.remote_screen.last_received, Some(received));
        assert!(app.remote_screen.last_status_update.is_none());
        assert!(app.remote_screen.last_text_update.is_none());
        assert!(app.remote_screen.elements.is_empty());
    }

    #[test]
    fn reopening_remote_invalidates_previous_receive_and_control_evidence() {
        let mut app = App::demo();
        assert!(app.remote_screen.last_received.is_some());
        assert!(app.remote_screen.last_control_report.is_some());
        app.event_tx
            .send(AppEvent::RemotePhase(
                crate::remote::RemoteSessionPhase::Opening,
            ))
            .unwrap();
        app.update();
        assert!(app.remote_screen.last_received.is_none());
        assert!(app.remote_screen.last_control_report.is_none());
        assert!(app.remote_screen.last_delta.is_none());
    }
}
