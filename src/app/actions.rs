use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;

use super::state::{App, AppEvent, AppMode, MainTab, RemoteInputState, WriteScope};
use crate::ble::{
    ble_scan_supported, disconnect_ble_bridge_for_device, ensure_ble_bridge_for_device,
    parse_ble_device_uri,
};
use crate::channel_file::{
    ChannelFileDocument, ChannelFileFormat, infer_channel_file_format,
    load_channel_document_from_path, save_channel_document_to_path, save_channels_to_path,
};
use crate::device::{
    ProgressEvent, flash_firmware, read_band_plans as read_radio_band_plans,
    read_channels_with_group_labels as read_radio_channels_with_group_labels,
    read_codeplug as read_radio_codeplug, read_dtmf_presets as read_radio_dtmf_presets,
    read_scan_presets_with_group_labels as read_radio_scan_presets_with_group_labels,
    read_settings as read_radio_settings, write_band_plans as write_radio_band_plans,
    write_channels as write_radio_channels, write_codeplug as write_radio_codeplug,
    write_dtmf_presets as write_radio_dtmf_presets, write_group_labels as write_radio_group_labels,
    write_scan_presets as write_radio_scan_presets, write_settings as write_radio_settings,
};
use crate::protocol::Endianness;
use crate::remote::{
    RemoteCaptureEvent, RemoteControlCommand, RemoteSessionOptions, RemoteSessionPhase,
    run_remote_session,
};

fn update_remote_input_phase(
    input: &std::sync::Mutex<RemoteInputState>,
    commands: &mpsc::Receiver<RemoteControlCommand>,
    phase: RemoteSessionPhase,
) {
    let mut input = input.lock().unwrap();
    input.ready = matches!(phase, RemoteSessionPhase::Armed | RemoteSessionPhase::Live);
    if matches!(
        phase,
        RemoteSessionPhase::Opening | RemoteSessionPhase::Recovering | RemoteSessionPhase::Stopped
    ) {
        // Never replay queued user input on a newly opened transport.
        while commands.try_recv().is_ok() {}
        input.pending = false;
    }
}

fn read_with_group_labels<T>(
    tx: &mpsc::Sender<AppEvent>,
    section: &str,
    read_bundle: impl FnOnce(
        &mut dyn FnMut(ProgressEvent),
    ) -> Result<(T, Endianness, Result<Vec<String>>)>,
    complete: impl FnOnce(T, Endianness) -> AppEvent,
) -> Result<()> {
    let mut last_progress = 0.0_f64;
    let mut report = |event| match event {
        ProgressEvent::Status(status) => {
            let _ = tx.send(AppEvent::Status(status));
        }
        ProgressEvent::Progress(value) => {
            last_progress = last_progress.max(value.clamp(0.0, 0.99));
            let _ = tx.send(AppEvent::Progress(last_progress));
        }
    };
    // Device progress already covers both sections; reserve only session cleanup.
    let (primary, endian, labels) = read_bundle(&mut report)?;
    let status = match labels {
        Ok(labels) => {
            let _ = tx.send(AppEvent::ReadGroupLabelsComplete(labels));
            format!("{section} and group names loaded")
        }
        Err(error) => {
            format!("{section} loaded, but group names could not be refreshed: {error}")
        }
    };
    let _ = tx.send(AppEvent::Progress(1.0));
    let _ = tx.send(complete(primary, endian));
    // Completion handlers set their own status; keep any partial-success warning visible.
    let _ = tx.send(AppEvent::Status(status));
    Ok(())
}

impl App {
    pub(crate) fn workspace_snapshot(&self) -> Option<Vec<u8>> {
        serde_json::to_vec(&(
            &self.channels,
            &self.settings,
            &self.scan_presets,
            &self.band_plans,
            &self.dtmf_presets,
            &self.group_labels,
            &self.deleted_channels,
            self.endian,
        ))
        .ok()
    }

    pub fn codeplug_snapshot_is_current(&self) -> bool {
        self.codeplug_data
            .as_ref()
            .is_some_and(|data| data.len() == crate::protocol::EEPROM_SIZE)
            && !self.channels_dirty
            && !self.settings_dirty
            && !self.dtmf_dirty
            && !self.group_labels_dirty
            && !self.scan_presets_dirty
            && !self.band_plans_dirty
            && self.deleted_channels.is_empty()
            && self.codeplug_workspace_baseline.is_some()
            && self.codeplug_workspace_baseline == self.workspace_snapshot()
    }

    pub(crate) fn hardware_action_allowed(&mut self) -> bool {
        let reason = if self.activity_active() {
            Some("Activity capture owns the radio; stop capture and wait for cleanup first")
        } else if self.demo_mode {
            Some("Demo mode: hardware I/O disabled")
        } else if self.remote_active || self.remote_tx.is_some() {
            Some(
                "Remote session owns the radio; stop Remote and wait for shutdown before another operation",
            )
        } else if matches!(
            self.mode,
            AppMode::Reading | AppMode::Writing | AppMode::BinFlashing
        ) {
            Some("A radio operation is still in progress; wait for it to finish")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.status_message = reason.to_string();
            return false;
        }
        true
    }

    pub fn refresh_radio_targets_from_tui(&mut self) {
        if !self.hardware_action_allowed() {
            return;
        }
        self.refresh_ports();
        self.start_ble_scan_from_tui();
    }

    pub fn start_ble_scan_from_tui(&mut self) {
        if !self.hardware_action_allowed() {
            return;
        }
        self.dialog_open = false;
        self.ble_scan_ui_suspended = false;

        if self.ble_scan_in_progress {
            self.status_message = "BLE scan already running...".to_string();
            return;
        }

        if !ble_scan_supported() {
            self.status_message =
                "Wireless scan is unavailable on this system. Use USB, or check that Bluetooth is enabled."
                    .to_string();
            return;
        }

        self.start_ble_scan(false);
        if self.ble_scan_in_progress {
            self.status_message = "Scanning for TD-H3 BLE radios...".to_string();
        }
    }

    pub(crate) fn active_protocol_port(&mut self) -> Option<String> {
        if !self.hardware_action_allowed() {
            return None;
        }
        if let Some(device_id) = self.selected_ble_device_id().map(str::to_string) {
            if self.ble_reconnect_required {
                let _ = disconnect_ble_bridge_for_device(&device_id);
                self.log(&format!(
                    "Refreshing BLE transport for {}",
                    self.selected_port_label()
                ));
            }

            match ensure_ble_bridge_for_device(&device_id) {
                Ok(bridge) => {
                    self.protocol_port_name = Some(bridge.tty_path.clone());
                    self.ble_reconnect_required = false;
                    Some(bridge.tty_path)
                }
                Err(error) => {
                    self.mode = AppMode::Error(format!(
                        "Failed to reconnect BLE radio {}: {}",
                        self.selected_port_label(),
                        error
                    ));
                    None
                }
            }
        } else {
            self.protocol_port_name.clone()
        }
    }

    fn mark_ble_reconnect_required(&mut self) {
        if self.ble_transport_selected() {
            self.ble_reconnect_required = true;
        }
    }

    pub fn select_port(&mut self) {
        if !self.hardware_action_allowed() {
            return;
        }
        if self.ports.is_empty() {
            self.mode = AppMode::Error("No radio targets found".to_string());
            return;
        }

        let Some(candidate) = self.selected_port_candidate().cloned() else {
            self.mode = AppMode::Error("No radio target selected".to_string());
            return;
        };

        let port_name = if let Some(device_id) = candidate.ble_device_id.as_deref() {
            match ensure_ble_bridge_for_device(device_id) {
                Ok(bridge) => bridge.tty_path,
                Err(error) => {
                    self.mode = AppMode::Error(format!(
                        "Failed to connect to BLE radio {}: {}",
                        candidate.port_name, error
                    ));
                    return;
                }
            }
        } else {
            candidate.port_name.clone()
        };

        let was_detected_radio = candidate.is_radio();
        self.mode = AppMode::Main(MainTab::Channels);
        self.ble_reconnect_required = false;
        self.status_message = if was_detected_radio {
            format!(
                "Using detected radio port {}. Radio opens on first action.",
                Self::display_port_name(&candidate.port_name)
            )
        } else if candidate.is_ble() {
            format!(
                "Using nearby radio {}. NicTUI will connect when you read, write, or start Remote.",
                candidate.port_name
            )
        } else {
            format!(
                "Selected {}. Radio opens on first action.",
                Self::display_port_name(&candidate.port_name)
            )
        };
        self.protocol_port_name = Some(port_name);
        self.active_port_candidate = Some(candidate.clone());
        self.log(&format!("Selected port {}", candidate.port_name));
    }

    pub fn pick_import_file(&mut self) {
        let _ = self.event_tx.send(AppEvent::ShowImportDialog);
    }

    pub fn pick_write_file(&mut self) {
        let _ = self.event_tx.send(AppEvent::ShowWriteDialog);
    }

    pub fn pick_export_file(&mut self) {
        let _ = self.event_tx.send(AppEvent::ShowExportDialog);
    }

    pub fn start_clear_channel(&mut self, channel_num: u16) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_channels(
                &port_name,
                &[],
                &[channel_num],
                crate::protocol::Endianness::Big,
                false,
                |event| match event {
                    ProgressEvent::Status(status) => {
                        let _ = tx.send(AppEvent::Status(status));
                    }
                    ProgressEvent::Progress(progress) => {
                        let _ = tx.send(AppEvent::Progress(progress));
                    }
                },
            ) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Progress(1.0));
                    let _ = tx.send(AppEvent::Status("Channel cleared successfully".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::Channels));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to clear channel: {}", e)));
                }
            }
        });
    }

    pub fn start_read_channels(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        if let AppMode::Main(tab) = self.mode {
            self.last_main_tab = tab;
            if tab != MainTab::Remote {
                self.last_non_remote_tab = tab;
            }
        }
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            if let Err(error) = read_with_group_labels(
                &tx,
                "Channels",
                |report| read_radio_channels_with_group_labels(&port_name, report),
                AppEvent::ReadChannelsComplete,
            ) {
                let _ = tx.send(AppEvent::Error(format!("Failed to read channels: {error}")));
            }
        });
    }

    pub fn start_read_presets(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            if let Err(error) = read_with_group_labels(
                &tx,
                "Scan presets",
                |report| read_radio_scan_presets_with_group_labels(&port_name, report),
                |presets, _| AppEvent::ReadPresetsComplete(presets),
            ) {
                let _ = tx.send(AppEvent::Error(format!("Failed to read presets: {error}")));
            }
        });
    }

    pub fn start_write_scan_presets(&mut self) {
        if !self.hardware_action_allowed() {
            return;
        }
        if !self.scan_presets_dirty {
            self.status_message = "No scan preset changes to write".to_string();
            return;
        }
        if self.scan_presets.is_empty() {
            self.status_message = "Read scan presets before writing".to_string();
            return;
        }
        if let Err(error) = crate::device::validate_scan_presets(&self.scan_presets) {
            self.status_message = format!("Invalid scan presets: {error}");
            return;
        }
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let presets = self.scan_presets.clone();
        let endian = self.endian;
        let tx = self.event_tx.clone();
        self.last_main_tab = MainTab::Scanning;
        self.mode = AppMode::Writing;
        self.progress = 0.0;
        thread::spawn(move || {
            match write_radio_scan_presets(&port_name, &presets, endian, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Status("Scan presets saved".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::ScanPresets));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to save scan presets: {error}"
                    )));
                }
            }
        });
    }

    pub fn start_write_band_plans(&mut self) {
        if !self.hardware_action_allowed() {
            return;
        }
        if !self.band_plans_dirty {
            self.status_message = "No band plan changes to write".to_string();
            return;
        }
        if self.band_plans.is_empty() {
            self.status_message = "Read band plans before writing".to_string();
            return;
        }
        if let Err(error) = crate::device::validate_band_plans_payload(&self.band_plans) {
            self.status_message = format!("Invalid band plans: {error}");
            return;
        }
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let plans = self.band_plans.clone();
        let endian = self.endian;
        let tx = self.event_tx.clone();
        self.last_main_tab = MainTab::BandPlan;
        self.mode = AppMode::Writing;
        self.progress = 0.0;
        thread::spawn(move || {
            match write_radio_band_plans(&port_name, &plans, endian, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Status("Band plans saved".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::BandPlans));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to save band plans: {error}"
                    )));
                }
            }
        });
    }

    pub fn start_write_group_labels(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        let group_labels = self.group_labels.clone();
        self.last_main_tab = MainTab::MemoryGroups;
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_group_labels(&port_name, &group_labels, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Status("Group names saved".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::GroupLabels));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to save group names: {}",
                        e
                    )));
                }
            }
        });
    }

    pub fn start_read_bandplan(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            match read_radio_band_plans(&port_name, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok((plans, _)) => {
                    let _ = tx.send(AppEvent::ReadBandPlanComplete(plans));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to read bandplan: {}", e)));
                }
            }
        });
    }

    pub fn start_read_dtmf(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            match read_radio_dtmf_presets(&port_name, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok(presets) => {
                    let _ = tx.send(AppEvent::ReadDTMFComplete(presets));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to read DTMF: {}", e)));
                }
            }
        });
    }

    pub fn start_write_dtmf(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        let dtmf_presets = self.dtmf_presets.clone();
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_dtmf_presets(&port_name, &dtmf_presets, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Status("DTMF written successfully".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::Dtmf));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to write DTMF: {}", e)));
                }
            }
        });
    }

    pub fn start_read_settings(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            match read_radio_settings(&port_name, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok((settings, endian)) => {
                    let _ = tx.send(AppEvent::ReadSettingsComplete(settings, endian));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to read settings: {}", e)));
                }
            }
        });
    }

    pub fn start_write_settings_and_reboot(&mut self) {
        let settings = match &self.settings {
            Some(s) => s.clone(),
            None => {
                self.status_message = "Read settings before writing".to_string();
                return;
            }
        };
        if !self.settings_dirty {
            self.status_message = "No settings changes to write".to_string();
            return;
        }
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        self.mark_ble_reconnect_required();
        let tx = self.event_tx.clone();
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_settings(
                &port_name,
                &settings,
                crate::protocol::Endianness::Big,
                true,
                |event| match event {
                    ProgressEvent::Status(status) => {
                        let _ = tx.send(AppEvent::Status(status));
                    }
                    ProgressEvent::Progress(progress) => {
                        let _ = tx.send(AppEvent::Progress(progress));
                    }
                },
            ) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Status(
                        "Settings written successfully".to_string(),
                    ));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::Settings));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to write settings: {}", e)));
                }
            }
        });
    }

    pub fn remote_on(&mut self) {
        if self.activity_active() {
            self.hardware_action_allowed();
            return;
        }
        if self.demo_mode {
            self.remote_active = true;
            self.remote_screen.phase = crate::remote::RemoteSessionPhase::Live;
            self.status_message = "Demo remote session active · no hardware I/O".to_string();
            return;
        }
        if self.remote_active {
            self.status_message = if self.remote_tx.is_some() {
                "Remote mode already active".to_string()
            } else {
                "Remote mode is still stopping".to_string()
            };
            return;
        }

        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        let (key_tx, key_rx) = mpsc::channel();
        self.remote_screen.reset_session();
        self.remote_screen.phase = crate::remote::RemoteSessionPhase::Opening;
        self.remote_screen.last_failure = None;
        self.remote_tx = Some(key_tx);
        self.remote_active = true;
        *self.remote_input_state.lock().unwrap() = RemoteInputState::default();
        self.status_message = "Opening remote session...".to_string();
        self.remote_stop_signal.store(false, Ordering::SeqCst);
        let stop_signal = self.remote_stop_signal.clone();
        let input_state = self.remote_input_state.clone();

        thread::spawn(move || {
            let result = run_remote_session(
                &port_name,
                &RemoteSessionOptions {
                    include_raw_logs: false,
                    disable_radio_before_remote: false,
                    recover_retries: 3,
                    suppress_repeated_idle: true,
                    ..RemoteSessionOptions::default()
                },
                |_| stop_signal.load(Ordering::SeqCst),
                |_| {
                    let input = input_state.lock().unwrap();
                    if stop_signal.load(Ordering::SeqCst) || !input.ready {
                        None
                    } else {
                        key_rx.try_recv().ok()
                    }
                },
                |event| match event {
                    RemoteCaptureEvent::Received(received_at) => {
                        let _ = tx.send(AppEvent::RemoteReceived(received_at));
                    }
                    RemoteCaptureEvent::Status(message) => {
                        let _ = tx.send(AppEvent::Status(message));
                    }
                    RemoteCaptureEvent::Log(message) => {
                        let _ = tx.send(AppEvent::Log(message));
                    }
                    RemoteCaptureEvent::Phase(phase) => {
                        update_remote_input_phase(&input_state, &key_rx, phase);
                        let _ = tx.send(AppEvent::RemotePhase(phase));
                    }
                    RemoteCaptureEvent::Control(report) => {
                        input_state.lock().unwrap().pending = false;
                        let _ = tx.send(AppEvent::RemoteControl(report));
                    }
                    RemoteCaptureEvent::Packet(packet) => {
                        let _ = tx.send(AppEvent::RemotePacket(packet));
                    }
                    RemoteCaptureEvent::Delta(delta) => {
                        let _ = tx.send(AppEvent::RemoteDelta(delta));
                    }
                },
            );

            match result {
                Ok(_) => {
                    let _ = tx.send(AppEvent::RemoteStopped {
                        message: "Remote mode OFF".to_string(),
                        failure: None,
                    });
                }
                Err(failure) => {
                    let _ = tx.send(AppEvent::RemoteStopped {
                        message: format!("Remote session {}: {}", failure.kind, failure.summary),
                        failure: Some(failure),
                    });
                }
            }
        });
    }

    pub fn remote_off(&mut self) {
        if self.demo_mode {
            self.remote_active = false;
            self.remote_screen.phase = crate::remote::RemoteSessionPhase::Stopped;
            self.status_message = "Demo remote session stopped".to_string();
            return;
        }
        if self.remote_active {
            self.remote_stop_signal.store(true, Ordering::SeqCst);
            self.remote_tx = None;
            self.status_message = "Stopping remote mode...".to_string();
        }
    }

    pub fn remote_controls_ready(&self) -> bool {
        if self.activity_active() {
            return false;
        }
        if self.mode != AppMode::Main(MainTab::Remote)
            || self.dialog_open
            || !self.remote_active
            || self.remote_stop_signal.load(Ordering::SeqCst)
            || !matches!(
                self.remote_screen.phase,
                RemoteSessionPhase::Armed | RemoteSessionPhase::Live
            )
        {
            return false;
        }
        if self.demo_mode {
            return true;
        }
        let input = self.remote_input_state.lock().unwrap();
        self.remote_tx.is_some() && input.ready && !input.pending
    }

    pub fn send_key(&mut self, key_code: u8) {
        if self.activity_active() {
            return;
        }
        if self.mode != AppMode::Main(MainTab::Remote) || self.dialog_open {
            return;
        }
        if !self.remote_active || self.remote_stop_signal.load(Ordering::SeqCst) {
            self.status_message = if self.remote_active {
                "Remote mode is still stopping".to_string()
            } else {
                "Remote mode is not active".to_string()
            };
            return;
        }
        if !matches!(
            self.remote_screen.phase,
            RemoteSessionPhase::Armed | RemoteSessionPhase::Live
        ) {
            self.status_message =
                "Remote is not ready for keys; wait for the connection".to_string();
            return;
        }
        if self.demo_mode {
            self.status_message = format!("Demo key: {}", remote_key_label(key_code));
            return;
        }
        let mut input = self.remote_input_state.lock().unwrap();
        if !input.ready || input.pending {
            self.status_message = if input.pending {
                "Remote key still in progress; wait before sending another".to_string()
            } else {
                "Remote is not ready for keys; wait for the connection".to_string()
            };
            return;
        }
        if let Some(tx) = &self.remote_tx {
            let label = remote_key_label(key_code);
            if tx
                .send(RemoteControlCommand::raw_key(label, key_code))
                .is_ok()
            {
                input.pending = true;
                drop(input);
                let label = remote_key_label(key_code);
                self.status_message = format!("Queued remote key {label}");
                self.log(&format!("Remote key {label}"));
            } else {
                self.remote_tx = None;
                self.remote_stop_signal.store(true, Ordering::SeqCst);
                self.status_message = "Waiting for remote session shutdown".to_string();
            }
        } else {
            self.status_message = "Remote mode is not active".to_string();
        }
    }

    pub fn start_write_channel(&mut self, index: usize) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        let channel = match self.channels.get(index) {
            Some(ch) => ch.clone(),
            None => return,
        };
        let endian = self.endian;
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_channels(
                &port_name,
                std::slice::from_ref(&channel),
                &[],
                endian,
                false,
                |event| match event {
                    ProgressEvent::Status(status) => {
                        let _ = tx.send(AppEvent::Status(status));
                    }
                    ProgressEvent::Progress(progress) => {
                        let _ = tx.send(AppEvent::Progress(progress));
                    }
                },
            ) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Progress(1.0));
                    let _ = tx.send(AppEvent::Status("Channel written successfully".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::Channels));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to write channel: {}", e)));
                }
            }
        });
    }

    pub fn start_write_multiple_channels(&mut self, _start_index: usize, reboot: bool) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        if reboot {
            self.mark_ble_reconnect_required();
        }
        let tx = self.event_tx.clone();
        let channels = self.channels.clone();
        let deleted_channels = self.deleted_channels.clone();
        let endian = self.endian;
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_channels(
                &port_name,
                &channels,
                &deleted_channels,
                endian,
                reboot,
                |event| match event {
                    ProgressEvent::Status(status) => {
                        let _ = tx.send(AppEvent::Status(status));
                    }
                    ProgressEvent::Progress(progress) => {
                        let _ = tx.send(AppEvent::Progress(progress));
                    }
                },
            ) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Status(
                        "Channels updated successfully".to_string(),
                    ));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::Channels));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to update channels: {}", e)));
                }
            }
        });
    }

    pub fn start_write_csv_channels(&mut self, path: PathBuf) {
        if !self.hardware_action_allowed() {
            return;
        }
        let document = match load_channel_document_from_path(&path).and_then(|document| {
            if document.group_labels.is_some() {
                anyhow::bail!("Channel file contains GroupLabels metadata. Import it into the workspace and save group names explicitly; direct channel writes do not write group names.");
            }
            crate::device::validate_channels(&document.channels)?;
            Ok(document)
        }) {
            Ok(document) => document,
            Err(error) => {
                self.mode = AppMode::Error(format!("Failed to load channels: {error}"));
                return;
            }
        };
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        let endian = self.endian;
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_channels(
                &port_name,
                &document.channels,
                &document.inactive_channels,
                endian,
                false,
                |event| match event {
                    ProgressEvent::Status(status) => {
                        let _ = tx.send(AppEvent::Status(status));
                    }
                    ProgressEvent::Progress(progress) => {
                        let _ = tx.send(AppEvent::Progress(progress));
                    }
                },
            ) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::Status(
                        "CSV Channels written successfully".to_string(),
                    ));
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::None));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to write CSV channels: {}",
                        e
                    )));
                }
            }
        });
    }

    pub fn load_csv(&mut self, path: &Path) -> Result<()> {
        let document = load_channel_document_from_path(path)?;
        crate::device::validate_channels(&document.channels)?;
        self.channels = document.channels;
        self.deleted_channels = document.inactive_channels;
        if let Some(labels) = document.group_labels {
            self.group_labels = crate::protocol::normalize_group_labels(&labels);
            self.group_labels_dirty = true;
        }
        self.channel_state.select(if self.channels.is_empty() {
            None
        } else {
            Some(0)
        });
        self.ensure_group_selection();
        self.channels_dirty = true;
        self.log(&format!(
            "Loaded {} channels and {} explicit deletions",
            self.channels.len(),
            self.deleted_channels.len()
        ));
        Ok(())
    }

    pub fn start_import_channels(&mut self, path: PathBuf) {
        match self.load_csv(&path) {
            Ok(_) => {
                self.mode = AppMode::Main(MainTab::Channels);
                self.status_message = format!(
                    "Imported {} channels and {} explicit deletions into workspace; press w to write channels{}",
                    self.channels.len(),
                    self.deleted_channels.len(),
                    if self.group_labels_dirty {
                        "; group names remain unsaved until written separately"
                    } else {
                        ""
                    }
                );
            }
            Err(e) => {
                self.mode = AppMode::Error(format!("Failed to load CSV: {}", e));
            }
        }
    }

    pub fn export_csv(&mut self, path: PathBuf) -> Result<()> {
        let channels: Vec<_> = self
            .channels
            .iter()
            .filter(|channel| !self.deleted_channels.contains(&channel.channel_num))
            .cloned()
            .collect();
        let exported_count = channels.len();
        let labels_known = self.group_labels_dirty
            || self.group_labels.iter().any(|label| !label.is_empty())
            || self
                .codeplug_data
                .as_ref()
                .is_some_and(|data| data.len() == crate::protocol::EEPROM_SIZE);
        if infer_channel_file_format(&path)? == ChannelFileFormat::Json
            && self.deleted_channels.is_empty()
            && !labels_known
        {
            save_channels_to_path(&path, &channels)?;
        } else {
            save_channel_document_to_path(
                &path,
                &ChannelFileDocument {
                    channels,
                    inactive_channels: self.deleted_channels.clone(),
                    group_labels: labels_known.then(|| self.group_labels.clone()),
                },
            )?;
        }
        self.log(&format!("Exported {exported_count} channels to CSV"));
        Ok(())
    }

    fn show_file_dialog<F>(&mut self, _title: &str, filters: &[(&str, &[&str])], on_select: F)
    where
        F: FnOnce(Option<PathBuf>),
    {
        self.dialog_open = true;
        self.suspend_ui();

        let mut dialog = rfd::FileDialog::new();
        for (name, extensions) in filters {
            dialog = dialog.add_filter(name.to_string(), extensions);
        }
        let res = dialog.pick_file();

        self.resume_ui();
        self.dialog_open = false;

        on_select(res);
    }

    pub(crate) fn show_save_dialog<F>(
        &mut self,
        _title: &str,
        default_name: &str,
        filters: &[(&str, &[&str])],
        on_select: F,
    ) where
        F: FnOnce(Option<PathBuf>),
    {
        self.dialog_open = true;
        self.suspend_ui();

        let mut dialog = rfd::FileDialog::new().set_file_name(default_name);
        for (name, extensions) in filters {
            dialog = dialog.add_filter(name.to_string(), extensions);
        }
        let res = dialog.save_file();

        self.resume_ui();
        self.dialog_open = false;

        on_select(res);
    }

    pub fn show_import_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Import CSV", &[("CSV", &["csv"])], move |path| {
            if let Some(p) = path {
                let _ = tx.send(AppEvent::LoadCSV(p));
            }
        });
    }

    pub fn show_export_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_save_dialog(
            "Export Channels",
            "channels.csv",
            &[("CSV", &["csv"])],
            move |path| {
                if let Some(p) = path {
                    let _ = tx.send(AppEvent::ExportCSV(p));
                }
            },
        );
    }

    pub fn show_write_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Write CSV", &[("CSV", &["csv"])], move |path| {
            if let Some(p) = path {
                let _ = tx.send(AppEvent::WriteCSV(p));
            }
        });
    }

    pub fn show_codeplug_import_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Import Codeplug", &[("Codeplug", &["nfw"])], move |path| {
            if let Some(p) = path {
                let _ = tx.send(AppEvent::LoadCodeplug(p));
            }
        });
    }

    pub fn show_codeplug_export_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_save_dialog(
            "Export Codeplug",
            "radio_codeplug.nfw",
            &[("Codeplug", &["nfw"])],
            move |path| {
                if let Some(p) = path {
                    let _ = tx.send(AppEvent::ExportCodeplug(p));
                }
            },
        );
    }

    pub fn start_read_codeplug(&mut self) {
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        self.last_main_tab = MainTab::Codeplug;
        self.mode = AppMode::Reading;
        self.progress = 0.0;
        self.status_message = "Reading full radio snapshot...".to_string();
        let tx = self.event_tx.clone();
        thread::spawn(move || {
            match read_radio_codeplug(&port_name, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok((data, _)) => {
                    let _ = tx.send(loaded_codeplug_event(PathBuf::from("Radio snapshot"), data));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to read full snapshot: {error}"
                    )));
                }
            }
        });
    }

    pub fn start_import_codeplug(&mut self, path: PathBuf) {
        use crate::protocol::codeplug;

        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let _ = tx.send(AppEvent::Status("Starting codeplug import...".to_string()));
            let _ = tx.send(AppEvent::Progress(0.0));
            let _ = tx.send(AppEvent::Status("Reading codeplug file...".to_string()));
            let _ = tx.send(AppEvent::Progress(0.2));

            match codeplug::load_codeplug(&path) {
                Ok(data) => {
                    let _ = tx.send(AppEvent::Status("Parsing codeplug data...".to_string()));
                    let _ = tx.send(AppEvent::Progress(0.4));
                    let event = loaded_codeplug_event(path, data);
                    let _ = tx.send(AppEvent::Progress(1.0));
                    let _ = tx.send(event);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to load codeplug: {}", e)));
                }
            }
        });
    }

    pub fn start_export_codeplug(&mut self, path: PathBuf) {
        use crate::protocol::codeplug;

        let Some(data) = self
            .codeplug_data
            .as_ref()
            .filter(|data| data.len() == crate::protocol::EEPROM_SIZE)
        else {
            self.status_message =
                "Export requires a full snapshot; read/import Codeplug first".to_string();
            return;
        };
        if !self.codeplug_snapshot_is_current() {
            self.status_message = "Export blocked: workspace differs from the full snapshot. Edits cannot be safely merged; read/import Codeplug first. No file was written.".to_string();
            return;
        }

        let codeplug_data = data.clone();
        let tx = self.event_tx.clone();

        thread::spawn(
            move || match codeplug::save_codeplug(&path, &codeplug_data) {
                Ok(_) => {
                    let _ = tx.send(AppEvent::Status(format!(
                        "Full snapshot exported byte-for-byte to {}",
                        path.display()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to save codeplug: {}", e)));
                }
            },
        );
    }

    pub fn start_write_codeplug(&mut self) {
        if !self.hardware_action_allowed() {
            return;
        }
        if self.codeplug_data.is_some() && !self.codeplug_snapshot_is_current() {
            self.status_message = "Codeplug write blocked: workspace differs from the original snapshot. Save section edits or reload the codeplug before writing the snapshot.".to_string();
            return;
        }
        if self.codeplug_data.is_none() {
            let _ = self.event_tx.send(AppEvent::Error(
                "No codeplug loaded. Please import a codeplug first.".to_string(),
            ));
            return;
        }

        let Some(port_name) = self.active_protocol_port() else {
            let _ = self
                .event_tx
                .send(AppEvent::Error("Not connected to radio".to_string()));
            return;
        };
        self.mark_ble_reconnect_required();

        let codeplug_data = self.codeplug_data.clone().unwrap();
        let tx = self.event_tx.clone();
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            match write_radio_codeplug(&port_name, &codeplug_data, true, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::WriteComplete(WriteScope::Codeplug));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to write codeplug: {}", e)));
                }
            }
        });
    }

    pub fn show_bin_firmware_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Select Firmware", &[("Firmware", &["bin"])], move |path| {
            if let Some(path) = path {
                let _ = tx.send(AppEvent::LoadBinFirmware(path));
            } else {
                let _ = tx.send(AppEvent::Status(
                    "Firmware file selection cancelled.".to_string(),
                ));
            }
        });
    }

    pub fn start_load_bin_firmware(&mut self, path: PathBuf) {
        let tx = self.event_tx.clone();

        thread::spawn(move || {
            let _ = tx.send(AppEvent::Status("Loading firmware file...".to_string()));

            match std::fs::read(&path) {
                Ok(data) => {
                    let _ = tx.send(AppEvent::Status("Firmware file loaded".to_string()));
                    let _ = tx.send(AppEvent::BinFirmwareLoaded(path, data));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to read firmware file: {}",
                        e
                    )));
                }
            }
        });
    }

    pub fn start_bin_flash(&mut self) {
        if !self.hardware_action_allowed() {
            return;
        }
        let firmware_data = match &self.bin_firmware_data {
            Some(data) => data.clone(),
            None => {
                self.mode = AppMode::Error(
                    "No firmware loaded. Press 'i' to import a firmware file.".to_string(),
                );
                return;
            }
        };

        if firmware_data.is_empty() || firmware_data.len() > 0xF800 {
            self.status_message = "Firmware must contain 1..=63488 bytes".to_string();
            return;
        }

        if self.ble_transport_selected()
            || self
                .protocol_port_name
                .as_deref()
                .is_some_and(|port| parse_ble_device_uri(port).is_some())
        {
            self.status_message = "Firmware flashing requires a USB serial connection".to_string();
            return;
        }
        let Some(port_name) = self.active_protocol_port() else {
            self.mode =
                AppMode::Error("Not connected to a port. Please select a port first.".to_string());
            return;
        };
        let tx = self.event_tx.clone();
        self.progress = 0.0;
        self.mode = AppMode::BinFlashing;
        thread::spawn(move || {
            match flash_firmware(&port_name, &firmware_data, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok(()) => {
                    let _ = tx.send(AppEvent::BinFlashComplete);
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::BinFlashFailed(error.to_string()));
                }
            }
        });
    }
}

fn loaded_codeplug_event(path: PathBuf, data: Vec<u8>) -> AppEvent {
    use crate::protocol::codeplug;

    let endian = codeplug::infer_channel_endianness_from_codeplug(&data);
    AppEvent::CodeplugDataLoaded {
        path,
        channels: codeplug::extract_channels_from_codeplug(&data, endian),
        settings: codeplug::extract_settings_from_codeplug(&data, endian),
        scan_presets: codeplug::extract_scan_presets_from_codeplug(&data, endian),
        group_labels: codeplug::extract_group_labels_from_codeplug(&data),
        data,
    }
}

fn remote_key_label(key_code: u8) -> &'static str {
    match key_code {
        0x01 => "0",
        0x02 => "1",
        0x03 => "2",
        0x04 => "3",
        0x05 => "4",
        0x06 => "5",
        0x07 => "6",
        0x08 => "7",
        0x09 => "8",
        0x0A => "9",
        0x0B => "menu",
        0x0C => "up",
        0x0D => "down",
        0x0E => "exit",
        0x0F => "*",
        0x10 => "#",
        0x11 => "B",
        0x12 => "F",
        0x13 => "A/PTT",
        0x1A => "B/PTT",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_reads_keep_progress_monotonic_until_labels_finish() {
        for scan in [false, true] {
            let (tx, rx) = mpsc::channel();
            let mut events = Vec::new();
            read_with_group_labels(
                &tx,
                if scan { "Scan presets" } else { "Channels" },
                |report| {
                    report(ProgressEvent::Status("Reading primary data".into()));
                    for value in [0.0, 0.5, 0.25, 0.9] {
                        report(ProgressEvent::Progress(value));
                    }
                    events.extend(rx.try_iter());
                    assert!(events.iter().all(|event| !matches!(event,
                        AppEvent::Progress(value) if *value >= 1.0)));
                    report(ProgressEvent::Status("Reading group names".into()));
                    for value in [0.95, 0.9, 1.0] {
                        report(ProgressEvent::Progress(value));
                    }
                    Ok((
                        if scan {
                            AppEvent::ReadPresetsComplete(Vec::new())
                        } else {
                            AppEvent::ReadChannelsComplete(Vec::new(), Endianness::Big)
                        },
                        Endianness::Big,
                        Ok(vec!["LOCAL".into()]),
                    ))
                },
                |event, _| event,
            )
            .unwrap();
            events.extend(rx.try_iter());
            let progress: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    AppEvent::Progress(value) => Some(*value),
                    _ => None,
                })
                .collect();
            assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
            assert_eq!(progress, [0.0, 0.5, 0.5, 0.9, 0.95, 0.95, 0.99, 1.0]);
            assert_eq!(progress.last(), Some(&1.0));
            assert!(
                progress[..progress.len() - 1]
                    .iter()
                    .all(|value| *value < 1.0)
            );
            assert!(events.iter().any(|event| matches!(event,
                AppEvent::ReadGroupLabelsComplete(labels) if labels == &["LOCAL"])));
            assert!(
                matches!(&events[events.len() - 2], AppEvent::ReadPresetsComplete(_) if scan)
                    || matches!(&events[events.len() - 2], AppEvent::ReadChannelsComplete(_, _) if !scan)
            );
            assert!(matches!(events.last(), Some(AppEvent::Status(status))
                if status.ends_with("and group names loaded")));
        }
    }

    #[test]
    fn bundled_reads_apply_channel_data_labels_and_returned_endianness() {
        let demo = App::demo();
        let mut app = App::new();
        app.mode = AppMode::Reading;
        app.group_labels_dirty = true;
        read_with_group_labels(
            &app.event_tx,
            "Channels",
            |report| {
                report(ProgressEvent::Progress(1.0));
                Ok((
                    demo.channels.clone(),
                    Endianness::Little,
                    Ok(vec!["LOCAL".into()]),
                ))
            },
            AppEvent::ReadChannelsComplete,
        )
        .unwrap();
        app.update();
        assert_eq!(
            serde_json::to_value(&app.channels).unwrap(),
            serde_json::to_value(&demo.channels).unwrap()
        );
        assert_eq!(app.endian, Endianness::Little);
        assert_eq!(app.group_labels[0], "LOCAL");
        assert!(!app.group_labels_dirty);
        assert!(!matches!(app.mode, AppMode::Reading | AppMode::Error(_)));
        assert_eq!(app.progress, 1.0);
        assert_eq!(app.status_message, "Channels and group names loaded");
    }

    #[test]
    fn bundled_reads_retain_primary_data_and_warning_when_labels_fail() {
        for scan in [false, true] {
            for partial_labels in [false, true] {
                let demo = App::demo();
                let mut app = App::new();
                app.mode = AppMode::Reading;
                app.group_labels[0] = "LOCAL".into();
                app.group_labels_dirty = true;
                read_with_group_labels(
                    &app.event_tx,
                    if scan { "Scan presets" } else { "Channels" },
                    |report| {
                        report(ProgressEvent::Progress(0.9));
                        if partial_labels {
                            report(ProgressEvent::Progress(0.95));
                        }
                        Ok((
                            if scan {
                                AppEvent::ReadPresetsComplete(demo.scan_presets.clone())
                            } else {
                                AppEvent::ReadChannelsComplete(demo.channels.clone(), demo.endian)
                            },
                            demo.endian,
                            Err(anyhow::anyhow!("group read failed")),
                        ))
                    },
                    |event, _| event,
                )
                .unwrap();
                app.update();
                assert!(!matches!(app.mode, AppMode::Reading | AppMode::Error(_)));
                assert_eq!(app.progress, 1.0);
                assert_eq!(app.group_labels[0], "LOCAL");
                assert!(app.group_labels_dirty);
                assert!(
                    app.status_message
                        .contains("group names could not be refreshed")
                );
                assert!(app.status_message.contains("group read failed"));
                if scan {
                    assert_eq!(
                        serde_json::to_value(&app.scan_presets).unwrap(),
                        serde_json::to_value(&demo.scan_presets).unwrap()
                    );
                } else {
                    assert_eq!(
                        serde_json::to_value(&app.channels).unwrap(),
                        serde_json::to_value(&demo.channels).unwrap()
                    );
                    assert_eq!(app.endian, demo.endian);
                }
            }
        }
    }

    #[test]
    fn bundled_reads_do_not_complete_on_primary_or_session_cleanup_failure() {
        for cleanup_failure in [false, true] {
            let (tx, rx) = mpsc::channel();
            let result = read_with_group_labels::<()>(
                &tx,
                "Channels",
                |report| {
                    report(ProgressEvent::Progress(if cleanup_failure {
                        1.0
                    } else {
                        0.5
                    }));
                    anyhow::bail!("read session failed")
                },
                |_, _| panic!("failed read session must not complete"),
            );
            assert_eq!(result.unwrap_err().to_string(), "read session failed");
            assert!(rx.try_iter().all(|event| matches!(event,
            AppEvent::Progress(value) if value < 1.0)));
        }
    }

    fn snapshot_test_path() -> PathBuf {
        static NEXT_PATH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "nictui-snapshot-{}-{nonce}-{sequence}.nfw",
            std::process::id()
        ))
    }

    fn wait_for_app(app: &mut App, complete: impl Fn(&App) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            app.update();
            assert!(!matches!(app.mode, AppMode::Error(_)), "{:?}", app.mode);
            if complete(app) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{}",
                app.status_message
            );
            thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn mixed_endian_snapshot() -> Vec<u8> {
        let app = App::demo();
        let mut data = app.codeplug_data.unwrap();
        let settings = crate::protocol::RadioProtocol::pack_settings_block(
            app.settings.as_ref().unwrap(),
            crate::protocol::Endianness::Little,
        );
        data[0x1900..0x1900 + settings.len()].copy_from_slice(&settings);
        let tail = data.len() - 32;
        for (index, byte) in data[tail..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37);
        }
        data
    }

    fn assert_mixed_snapshot_loaded(app: &App, expected: &[u8]) {
        assert_eq!(app.endian, crate::protocol::Endianness::Big);
        assert_eq!(app.channels.len(), 48);
        assert_eq!(app.channels[0].rx_freq, "145.33500");
        assert_eq!(app.settings.as_ref().unwrap().magic, 0xD82F);
        assert_eq!(app.settings.as_ref().unwrap().step, 500);
        assert_eq!(app.codeplug_data.as_deref(), Some(expected));
        assert_eq!(app.mode, AppMode::Main(MainTab::Codeplug));
        assert!(app.codeplug_snapshot_is_current());
    }

    #[test]
    fn partial_workspaces_cannot_export_or_overwrite_a_backup() {
        let path = snapshot_test_path();
        std::fs::write(&path, b"existing backup").unwrap();
        for data in [None, Some(vec![0; 32])] {
            let mut app = App::demo();
            app.codeplug_data = data;
            app.start_export_codeplug(path.clone());
            assert_eq!(
                app.status_message,
                "Export requires a full snapshot; read/import Codeplug first"
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"existing backup");
            assert!(app.event_rx.try_recv().is_err());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn modified_or_untracked_snapshots_cannot_export_or_drop_edits() {
        let edits: &[fn(&mut App)] = &[
            |app| app.channels_dirty = true,
            |app| app.settings_dirty = true,
            |app| app.dtmf_dirty = true,
            |app| app.group_labels_dirty = true,
            |app| app.scan_presets_dirty = true,
            |app| app.band_plans_dirty = true,
            |app| app.channels[0].name = "CHANGED".to_string(),
            |app| app.settings.as_mut().unwrap().squelch += 1,
            |app| app.scan_presets[0].label = "CHANGED".to_string(),
            |app| app.band_plans[0].max_power = 254,
            |app| app.dtmf_presets[0].digits.push(1),
            |app| app.group_labels[0] = "CHANGED".to_string(),
            |app| app.deleted_channels.push(1),
            |app| app.codeplug_workspace_baseline = None,
        ];
        let path = snapshot_test_path();
        std::fs::write(&path, b"existing backup").unwrap();
        for edit in edits {
            let mut app = App::demo();
            edit(&mut app);
            let workspace = app.workspace_snapshot();
            app.start_export_codeplug(path.clone());
            assert!(app.status_message.contains("Export blocked"));
            assert_eq!(app.workspace_snapshot(), workspace);
            assert_eq!(std::fs::read(&path).unwrap(), b"existing backup");
            assert!(app.event_rx.try_recv().is_err());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn imported_mixed_endian_snapshot_exports_every_original_byte() {
        let input = snapshot_test_path();
        let output = snapshot_test_path();
        let data = mixed_endian_snapshot();
        std::fs::write(&input, &data).unwrap();
        let mut app = App::new();
        app.endian = crate::protocol::Endianness::Little;
        app.scan_presets_dirty = true;
        app.band_plans_dirty = true;
        app.start_import_codeplug(input.clone());
        wait_for_app(&mut app, |app| app.codeplug_data.is_some());
        assert_mixed_snapshot_loaded(&app, &data);
        app.start_export_codeplug(output.clone());
        wait_for_app(&mut app, |app| {
            app.status_message.contains("exported byte-for-byte")
        });
        assert_eq!(std::fs::read(&output).unwrap(), data);
        std::fs::remove_file(input).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn full_snapshot_read_respects_remote_and_operation_ownership() {
        for mode in [AppMode::Reading, AppMode::Writing, AppMode::BinFlashing] {
            let mut app = App::new();
            app.mode = mode.clone();
            app.start_read_codeplug();
            assert_eq!(app.mode, mode);
            assert!(app.status_message.contains("still in progress"));
            assert!(app.event_rx.try_recv().is_err());
        }
        let mut app = App::new();
        app.remote_active = true;
        app.start_read_codeplug();
        assert!(app.status_message.contains("Remote session owns"));
        assert!(app.event_rx.try_recv().is_err());
    }

    #[test]
    fn scan_and_band_writers_respect_remote_and_operation_ownership() {
        let writers: &[fn(&mut App)] =
            &[App::start_write_scan_presets, App::start_write_band_plans];
        for writer in writers {
            for mode in [AppMode::Reading, AppMode::Writing, AppMode::BinFlashing] {
                let mut app = App::new();
                app.mode = mode.clone();
                app.scan_presets_dirty = true;
                app.band_plans_dirty = true;
                writer(&mut app);
                assert_eq!(app.mode, mode);
                assert!(app.scan_presets_dirty && app.band_plans_dirty);
                assert!(app.status_message.contains("still in progress"));
                assert!(app.event_rx.try_recv().is_err());
            }
            let mut app = App::new();
            app.remote_active = true;
            app.scan_presets_dirty = true;
            app.band_plans_dirty = true;
            writer(&mut app);
            assert!(app.scan_presets_dirty && app.band_plans_dirty);
            assert!(app.status_message.contains("Remote session owns"));
            assert!(app.event_rx.try_recv().is_err());
        }
    }

    #[test]
    fn scan_and_band_writers_reject_empty_or_invalid_data_before_connection() {
        let mut app = App::new();
        assert!(!app.scan_presets_dirty && !app.band_plans_dirty);
        app.start_write_scan_presets();
        assert_eq!(app.status_message, "No scan preset changes to write");
        app.start_write_band_plans();
        assert_eq!(app.status_message, "No band plan changes to write");
        app.scan_presets_dirty = true;
        app.band_plans_dirty = true;
        app.start_write_scan_presets();
        assert_eq!(app.status_message, "Read scan presets before writing");
        app.start_write_band_plans();
        assert_eq!(app.status_message, "Read band plans before writing");
        app.scan_presets = vec![crate::protocol::ScanPreset {
            ultrascan: 21,
            ..Default::default()
        }];
        app.start_write_scan_presets();
        assert!(app.status_message.contains("Invalid scan presets"));
        app.band_plans = vec![crate::protocol::BandPlan {
            start_freq: 20_000_000,
            end_freq: 10_000_000,
            ..Default::default()
        }];
        app.start_write_band_plans();
        assert!(app.status_message.contains("Invalid band plans"));
        assert!(app.scan_presets_dirty && app.band_plans_dirty);
        assert_eq!(app.mode, AppMode::PortSelection);
        assert!(app.protocol_port_name.is_none());
        assert!(app.event_rx.try_recv().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn full_snapshot_read_decodes_mixed_sections_and_exports_untouched_bytes() {
        use serialport::SerialPort;
        use std::io::{Read, Write};

        let data = mixed_endian_snapshot();
        let expected = data.clone();
        let (mut master, slave) = serialport::TTYPort::pair().unwrap();
        master
            .set_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let port = slave.name().unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            loop {
                let mut command = [0];
                master.read_exact(&mut command).unwrap();
                match command[0] {
                    0x01 | 0x45 | 0x46 => {
                        requests.push((command[0], None));
                        master.write_all(&command).unwrap();
                        if command[0] == 0x46 {
                            break;
                        }
                    }
                    0x30 => {
                        let mut block = [0];
                        master.read_exact(&mut block).unwrap();
                        requests.push((command[0], Some(block[0])));
                        let start = usize::from(block[0]) * crate::protocol::BLOCK_SIZE;
                        let payload = &data[start..start + crate::protocol::BLOCK_SIZE];
                        let mut response = vec![0x30];
                        response.extend_from_slice(payload);
                        response.push(
                            payload
                                .iter()
                                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
                        );
                        master.write_all(&response).unwrap();
                    }
                    other => panic!("Unexpected snapshot command {other:#04x}"),
                }
            }
            // Keep the PTY alive until the client has consumed its final response.
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(20));
            requests
        });
        let mut app = App::new();
        app.protocol_port_name = Some(port);
        app.endian = crate::protocol::Endianness::Little;
        app.mode = AppMode::Main(MainTab::Codeplug);
        app.start_read_codeplug();
        assert_eq!(app.mode, AppMode::Reading);
        wait_for_app(&mut app, |app| app.codeplug_data.is_some());
        release_tx.send(()).unwrap();
        let requests = server.join().unwrap();
        drop(slave);
        let mut expected_requests = vec![(0x01, None)];
        // Validation precedes the pause; the full payload must be read while paused.
        let settings_start = (0x1900 / crate::protocol::BLOCK_SIZE) as u8;
        expected_requests
            .extend((settings_start..settings_start + 4).map(|block| (0x30, Some(block))));
        expected_requests.push((0x45, None));
        expected_requests
            .extend((0..crate::protocol::TOTAL_BLOCKS).map(|block| (0x30, Some(block as u8))));
        expected_requests.push((0x46, None));
        assert_eq!(requests, expected_requests);
        assert_mixed_snapshot_loaded(&app, &expected);
        assert_eq!(app.codeplug_path, Some(PathBuf::from("Radio snapshot")));
        let output = snapshot_test_path();
        app.start_export_codeplug(output.clone());
        wait_for_app(&mut app, |app| {
            app.status_message.contains("exported byte-for-byte")
        });
        assert_eq!(std::fs::read(&output).unwrap(), expected);
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn constructors_do_not_discover_or_connect_hardware() {
        let app = App::new();
        assert!(app.port_candidates.is_empty());
        assert!(app.ports.is_empty());
        assert!(app.protocol_port_name.is_none());
        assert!(!app.ble_scan_in_progress);
        assert!(app.event_rx.try_recv().is_err());

        let app = App::demo();
        assert!(app.demo_mode);
        assert!(!app.ble_scan_in_progress);
        assert!(app.event_rx.try_recv().is_err());
    }

    #[test]
    fn demo_hardware_actions_are_blocked_before_transport_or_workers() {
        let actions: &[fn(&mut App)] = &[
            App::refresh_ports,
            App::refresh_radio_targets,
            App::refresh_radio_targets_from_tui,
            App::start_ble_scan_from_tui,
            |app| app.start_ble_scan(true),
            |app| app.connect_to_port_by_name("/dev/not-a-radio"),
            App::select_port,
            App::start_read_channels,
            App::start_read_codeplug,
            App::start_read_presets,
            App::start_read_bandplan,
            App::start_read_dtmf,
            App::start_read_settings,
            App::start_write_group_labels,
            App::start_write_scan_presets,
            App::start_write_band_plans,
            App::start_write_dtmf,
            App::start_write_settings_and_reboot,
            |app| app.start_clear_channel(1),
            |app| app.start_write_channel(0),
            |app| app.start_write_multiple_channels(0, true),
            |app| app.start_write_csv_channels(PathBuf::from("missing.csv")),
            App::start_write_codeplug,
            App::start_bin_flash,
        ];
        for action in actions {
            let mut app = App::demo();
            app.settings_dirty = true;
            app.scan_presets_dirty = true;
            app.band_plans_dirty = true;
            app.bin_firmware_data = Some(vec![0; 33]);
            let original_port = app.protocol_port_name.clone();
            action(&mut app);
            assert!(
                app.status_message.contains("hardware I/O disabled"),
                "{}",
                app.status_message
            );
            assert_eq!(app.protocol_port_name, original_port);
            assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
            assert!(!app.ble_scan_in_progress);
            assert!(app.remote_tx.is_none());
            assert!(app.event_rx.try_recv().is_err());
        }
    }

    #[test]
    fn remote_shutdown_owns_transport_until_stop_event() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.prev_tab();
        assert!(app.remote_stop_signal.load(Ordering::SeqCst));
        app.start_read_dtmf();
        assert_eq!(app.mode, AppMode::Main(MainTab::DTMF));
        assert!(app.status_message.contains("Remote session owns"));
        assert!(!app.hardware_action_allowed());

        app.event_tx
            .send(AppEvent::RemoteStopped {
                message: "Remote mode OFF".to_string(),
                failure: None,
            })
            .unwrap();
        app.update();
        assert!(app.hardware_action_allowed());
    }

    #[test]
    fn closed_remote_command_channel_does_not_release_session_ownership() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_screen.phase = RemoteSessionPhase::Live;
        app.remote_input_state.lock().unwrap().ready = true;
        let (tx, rx) = mpsc::channel();
        drop(rx);
        app.remote_tx = Some(tx);
        app.remote_active = true;
        app.send_key(0x0B);
        assert!(app.remote_active);
        assert!(app.remote_stop_signal.load(Ordering::SeqCst));
        assert!(!app.hardware_action_allowed());
    }

    #[test]
    fn remote_keys_require_readiness_and_only_one_command_can_be_pending() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        let (tx, rx) = mpsc::channel();
        app.remote_tx = Some(tx);
        for phase in [
            RemoteSessionPhase::Opening,
            RemoteSessionPhase::Recovering,
            RemoteSessionPhase::Probing,
            RemoteSessionPhase::Stopped,
        ] {
            app.remote_screen.phase = phase;
            app.send_key(0x0B);
            assert!(rx.try_recv().is_err(), "unexpected command in {phase}");
        }

        app.remote_screen.phase = RemoteSessionPhase::Armed;
        update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Armed);
        assert!(app.remote_controls_ready());
        app.send_key(0x0B);
        assert!(!app.remote_controls_ready());
        app.send_key(0x0C);
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
        assert!(app.status_message.contains("still in progress"));
    }

    #[test]
    fn remote_recovery_discards_queued_keys_before_the_ui_receives_its_phase() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.remote_screen.phase = RemoteSessionPhase::Live;
        let (tx, rx) = mpsc::channel();
        app.remote_tx = Some(tx);
        update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Live);
        assert!(app.remote_controls_ready());
        app.send_key(0x0B);
        assert!(app.remote_input_state.lock().unwrap().pending);

        update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Recovering);
        assert!(!app.remote_controls_ready());
        app.send_key(0x0C);
        assert!(rx.try_recv().is_err());
        assert!(!app.remote_input_state.lock().unwrap().pending);
        assert!(app.status_message.contains("not ready"));

        update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Armed);
        app.send_key(0x0E);
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn remote_keys_are_suppressed_by_dialogs_other_tabs_and_shutdown() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.remote_screen.phase = RemoteSessionPhase::Live;
        let (tx, rx) = mpsc::channel();
        app.remote_tx = Some(tx);
        update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Live);

        app.dialog_open = true;
        assert!(!app.remote_controls_ready());
        app.send_key(0x13);
        app.dialog_open = false;
        app.mode = AppMode::Main(MainTab::Channels);
        assert!(!app.remote_controls_ready());
        app.send_key(0x13);
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_stop_signal.store(true, Ordering::SeqCst);
        assert!(!app.remote_controls_ready());
        app.send_key(0x13);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn channel_document_import_export_preserves_only_explicit_deletions_and_labels() {
        for extension in ["csv", "json"] {
            let input = snapshot_test_path().with_extension(extension);
            let output = snapshot_test_path().with_extension(extension);
            let demo = App::demo();
            let document = ChannelFileDocument {
                channels: demo.channels[..1].to_vec(),
                inactive_channels: vec![198],
                group_labels: Some(crate::protocol::normalize_group_labels(&["LOCAL".into()])),
            };
            save_channel_document_to_path(&input, &document).unwrap();
            let mut app = App::new();
            app.deleted_channels.push(197);
            app.start_import_channels(input.clone());
            assert_eq!(app.channels.len(), 1);
            assert_eq!(app.deleted_channels, [198]);
            assert!(app.channels_dirty && app.group_labels_dirty);
            assert_eq!(app.group_labels, document.group_labels.clone().unwrap());
            assert!(app.status_message.contains("group names remain unsaved"));
            assert!(app.event_rx.try_recv().is_err());
            app.export_csv(output.clone()).unwrap();
            let exported = load_channel_document_from_path(&output).unwrap();
            assert_eq!(
                serde_json::to_value(exported).unwrap(),
                serde_json::to_value(document).unwrap()
            );
            assert!(app.channels_dirty && app.group_labels_dirty);
            std::fs::remove_file(input).unwrap();
            std::fs::remove_file(output).unwrap();
        }
    }

    #[test]
    fn channel_document_export_excludes_rows_marked_deleted_by_app() {
        for extension in ["csv", "json"] {
            for known_labels in [false, true] {
                let path = snapshot_test_path().with_extension(extension);
                let demo = App::demo();
                let mut app = App::new();
                if known_labels {
                    app.group_labels = crate::protocol::normalize_group_labels(&["LOCAL".into()]);
                    app.group_labels_dirty = true;
                }
                app.channels = demo.channels[..3].to_vec();
                app.channel_state.select(Some(1));
                let deleted = app.channels[1].channel_num;
                let remaining = [app.channels[0].channel_num, app.channels[2].channel_num];
                app.delete_selected_channel();
                assert_eq!(app.channels.len(), 3);
                assert_eq!(app.deleted_channels, [deleted]);
                app.export_csv(path.clone()).unwrap();
                let document = load_channel_document_from_path(&path).unwrap();
                assert_eq!(
                    document
                        .channels
                        .iter()
                        .map(|channel| channel.channel_num)
                        .collect::<Vec<_>>(),
                    remaining
                );
                assert_eq!(document.inactive_channels, [deleted]);
                assert_eq!(document.group_labels.is_some(), known_labels);
                assert_eq!(app.channels.len(), 3);
                assert_eq!(app.deleted_channels, [deleted]);
                assert!(app.channels_dirty);
                std::fs::remove_file(path).unwrap();
            }
        }
    }

    #[test]
    fn channel_document_inactive_only_import_keeps_deletions_explicit() {
        let path = snapshot_test_path().with_extension("csv");
        save_channel_document_to_path(
            &path,
            &ChannelFileDocument {
                inactive_channels: vec![198],
                ..Default::default()
            },
        )
        .unwrap();
        let mut app = App::demo();
        app.load_csv(&path).unwrap();
        assert!(app.channels.is_empty());
        assert_eq!(app.deleted_channels, [198]);
        assert!(app.channels_dirty);
        assert_eq!(app.channel_state.selected(), None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn channel_document_import_preserves_explicit_empty_label_clear() {
        for extension in ["csv", "json"] {
            let path = snapshot_test_path().with_extension(extension);
            save_channel_document_to_path(
                &path,
                &ChannelFileDocument {
                    group_labels: Some(Vec::new()),
                    ..Default::default()
                },
            )
            .unwrap();
            let mut app = App::demo();
            app.load_csv(&path).unwrap();
            assert!(app.group_labels.iter().all(String::is_empty));
            assert!(app.group_labels_dirty);
            assert!(app.deleted_channels.is_empty());
            app.export_csv(path.clone()).unwrap();
            assert!(
                load_channel_document_from_path(&path)
                    .unwrap()
                    .group_labels
                    .is_some()
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn channel_document_export_omits_unknown_default_labels_and_keeps_plain_json_array() {
        for extension in ["csv", "json"] {
            let path = snapshot_test_path().with_extension(extension);
            let mut app = App::new();
            app.channels = App::demo().channels[..1].to_vec();
            app.export_csv(path.clone()).unwrap();
            let document = load_channel_document_from_path(&path).unwrap();
            assert!(document.group_labels.is_none());
            assert!(document.inactive_channels.is_empty());
            if extension == "json" {
                let value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                assert!(value.is_array());
            }
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn channel_document_export_retains_known_empty_labels_from_edits_or_snapshot() {
        for snapshot in [false, true] {
            let path = snapshot_test_path().with_extension("csv");
            let mut app = App::new();
            if snapshot {
                app.codeplug_data = App::demo().codeplug_data;
            } else {
                app.group_labels_dirty = true;
            }
            app.export_csv(path.clone()).unwrap();
            let labels = load_channel_document_from_path(&path)
                .unwrap()
                .group_labels
                .unwrap();
            assert_eq!(labels.len(), crate::protocol::GROUP_LABEL_COUNT);
            assert!(labels.iter().all(String::is_empty));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn channel_document_import_without_labels_preserves_existing_unsaved_labels() {
        let path = snapshot_test_path().with_extension("json");
        let mut app = App::demo();
        app.group_labels[0] = "LOCAL".into();
        app.group_labels_dirty = true;
        let labels = app.group_labels.clone();
        save_channels_to_path(&path, &app.channels[..1]).unwrap();
        app.load_csv(&path).unwrap();
        assert_eq!(app.group_labels, labels);
        assert!(app.group_labels_dirty);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn channel_document_direct_write_rejects_labels_before_connection_resolution() {
        let path = snapshot_test_path().with_extension("csv");
        save_channel_document_to_path(
            &path,
            &ChannelFileDocument {
                group_labels: Some(vec!["LOCAL".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        let mut app = App::new();
        app.start_write_csv_channels(path.clone());
        assert!(
            matches!(&app.mode, AppMode::Error(error) if error.contains("GroupLabels") && error.contains("Import"))
        );
        assert!(app.protocol_port_name.is_none());
        assert!(app.event_rx.try_recv().is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn channel_document_invalid_import_keeps_workspace_unchanged() {
        let path = snapshot_test_path().with_extension("json");
        let mut app = App::demo();
        let mut invalid = app.channels[..1].to_vec();
        invalid[0].rx_freq = "NaN".into();
        save_channels_to_path(&path, &invalid).unwrap();
        let original = app.workspace_snapshot();
        assert!(app.load_csv(&path).is_err());
        assert_eq!(app.workspace_snapshot(), original);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn csv_import_only_updates_workspace_and_clears_old_deletions() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("nictui-import-{}-{nonce}.csv", std::process::id()));
        let mut app = App::demo();
        save_channels_to_path(&path, &app.channels[..2]).unwrap();
        app.deleted_channels.push(1);
        app.event_tx.send(AppEvent::LoadCSV(path.clone())).unwrap();
        app.update();
        std::fs::remove_file(path).unwrap();
        assert_eq!(app.channels.len(), 2);
        assert!(app.deleted_channels.is_empty());
        assert!(app.channels_dirty);
        assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
        assert!(app.status_message.contains("press w"));
        assert!(app.event_rx.try_recv().is_err());
    }

    #[test]
    fn snapshot_write_rejects_workspace_changes_even_after_dirty_flags_are_cleared() {
        let edits: &[fn(&mut App)] = &[
            |app| app.channels[0].name = "Changed".to_string(),
            |app| app.settings.as_mut().unwrap().squelch += 1,
            |app| app.scan_presets[0].label = "Changed".to_string(),
            |app| app.band_plans[0].tx_allowed = !app.band_plans[0].tx_allowed,
            |app| app.dtmf_presets[0].digits.push(1),
            |app| app.group_labels[0] = "Changed".to_string(),
            |app| app.deleted_channels.push(1),
        ];
        for edit in edits {
            let mut app = App::demo();
            app.demo_mode = false;
            app.mode = AppMode::Main(MainTab::Codeplug);
            let original = app.codeplug_data.clone();
            edit(&mut app);
            app.start_write_codeplug();
            assert!(app.status_message.contains("Codeplug write blocked"));
            assert_eq!(app.codeplug_data, original);
            assert_eq!(app.mode, AppMode::Main(MainTab::Codeplug));
            assert!(app.event_rx.try_recv().is_err());
        }
    }

    #[test]
    fn firmware_ble_targets_are_rejected_before_connection_or_worker_start() {
        for port in [None, Some("ble://fixture-only"), Some("ble:fixture-only")] {
            let mut app = if port.is_none() {
                App::demo()
            } else {
                App::new()
            };
            app.demo_mode = false;
            app.mode = AppMode::Main(MainTab::BinFlash);
            app.bin_firmware_data = Some(vec![0x42; 32]);
            app.protocol_port_name = port.map(str::to_string);
            app.start_bin_flash();
            assert_eq!(app.mode, AppMode::Main(MainTab::BinFlash));
            assert!(
                app.status_message
                    .contains("requires a USB serial connection")
            );
            assert!(app.event_rx.try_recv().is_err());
        }
    }

    #[test]
    fn invalid_firmware_is_rejected_before_connection() {
        for data in [Vec::new(), vec![0; 0xF801]] {
            let mut app = App::new();
            app.mode = AppMode::Main(MainTab::BinFlash);
            app.bin_firmware_data = Some(data);
            app.start_bin_flash();
            assert_eq!(app.mode, AppMode::Main(MainTab::BinFlash));
            assert!(app.status_message.contains("Firmware must contain"));
            assert!(app.event_rx.try_recv().is_err());
        }
    }

    #[test]
    fn tui_ble_scan_action_does_not_leave_scan_modal_state_when_scan_is_running() {
        let mut app = App::new();
        app.ble_scan_in_progress = true;
        app.dialog_open = true;
        app.ble_scan_ui_suspended = true;

        app.start_ble_scan_from_tui();

        assert!(!app.dialog_open);
        assert!(!app.ble_scan_ui_suspended);
        assert_eq!(app.status_message, "BLE scan already running...");
    }
}
