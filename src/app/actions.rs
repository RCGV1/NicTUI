use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::state::{App, AppEvent, AppMode, MainTab, WriteScope};
use crate::ble::{
    ble_scan_supported, disconnect_ble_bridge_for_device, ensure_ble_bridge_for_device,
};
use crate::channel_file::{load_channels_from_path, save_channels_to_path};
use crate::device::{
    ProgressEvent, read_band_plans as read_radio_band_plans, read_channels as read_radio_channels,
    read_dtmf_presets as read_radio_dtmf_presets, read_group_labels as read_radio_group_labels,
    read_scan_presets as read_radio_scan_presets, read_settings as read_radio_settings,
    write_channels as write_radio_channels, write_codeplug as write_radio_codeplug,
    write_dtmf_presets as write_radio_dtmf_presets, write_group_labels as write_radio_group_labels,
    write_settings as write_radio_settings,
};
use crate::protocol::{BIN_FLASH_BAUD_RATE, RadioProtocol};
use crate::remote::{
    RemoteCaptureEvent, RemoteControlCommand, RemoteSessionOptions, run_remote_session,
};

impl App {
    pub fn refresh_radio_targets_from_tui(&mut self) {
        self.refresh_ports();
        self.start_ble_scan_from_tui();
    }

    pub fn start_ble_scan_from_tui(&mut self) {
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

    fn active_protocol_port(&mut self) -> Option<String> {
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
            match read_radio_channels(&port_name, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok((channels, endian)) => {
                    let _ = tx.send(AppEvent::ReadChannelsComplete(channels, endian));
                    match read_radio_group_labels(&port_name, |event| match event {
                        ProgressEvent::Status(status) => {
                            let _ = tx.send(AppEvent::Status(status));
                        }
                        ProgressEvent::Progress(progress) => {
                            let _ = tx.send(AppEvent::Progress(progress));
                        }
                    }) {
                        Ok(labels) => {
                            let _ = tx.send(AppEvent::ReadGroupLabelsComplete(labels));
                            let _ = tx.send(AppEvent::Status(
                                "Channels and group names loaded".to_string(),
                            ));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Status(format!(
                                "Channels loaded, but group names could not be refreshed: {}",
                                e
                            )));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to read channels: {}", e)));
                }
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
            match read_radio_scan_presets(&port_name, |event| match event {
                ProgressEvent::Status(status) => {
                    let _ = tx.send(AppEvent::Status(status));
                }
                ProgressEvent::Progress(progress) => {
                    let _ = tx.send(AppEvent::Progress(progress));
                }
            }) {
                Ok((presets, _)) => {
                    let _ = tx.send(AppEvent::ReadPresetsComplete(presets));
                    match read_radio_group_labels(&port_name, |event| match event {
                        ProgressEvent::Status(status) => {
                            let _ = tx.send(AppEvent::Status(status));
                        }
                        ProgressEvent::Progress(progress) => {
                            let _ = tx.send(AppEvent::Progress(progress));
                        }
                    }) {
                        Ok(labels) => {
                            let _ = tx.send(AppEvent::ReadGroupLabelsComplete(labels));
                            let _ = tx.send(AppEvent::Status(
                                "Scan presets and group names loaded".to_string(),
                            ));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Status(format!(
                                "Scan presets loaded, but group names could not be refreshed: {}",
                                e
                            )));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to read presets: {}", e)));
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
        self.remote_screen = Default::default();
        self.remote_screen.phase = crate::remote::RemoteSessionPhase::Opening;
        self.remote_screen.last_failure = None;
        self.remote_tx = Some(key_tx);
        self.remote_active = true;
        self.status_message = "Opening remote session...".to_string();
        self.remote_stop_signal.store(false, Ordering::SeqCst);
        let stop_signal = self.remote_stop_signal.clone();

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
                |_| key_rx.try_recv().ok(),
                |event| match event {
                    RemoteCaptureEvent::Status(message) => {
                        let _ = tx.send(AppEvent::Status(message));
                    }
                    RemoteCaptureEvent::Log(message) => {
                        let _ = tx.send(AppEvent::Log(message));
                    }
                    RemoteCaptureEvent::Phase(phase) => {
                        let _ = tx.send(AppEvent::RemotePhase(phase));
                    }
                    RemoteCaptureEvent::Control(report) => {
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
        if self.remote_active {
            self.remote_stop_signal.store(true, Ordering::SeqCst);
            self.remote_tx = None;
            self.status_message = "Stopping remote mode...".to_string();
        }
    }

    pub fn send_key(&mut self, key_code: u8) {
        if let Some(tx) = &self.remote_tx {
            let label = remote_key_label(key_code);
            if tx
                .send(RemoteControlCommand::raw_key(label, key_code))
                .is_ok()
            {
                let label = remote_key_label(key_code);
                self.status_message = format!("Sent remote key {label}");
                self.log(&format!("Remote key {label}"));
            } else {
                self.remote_active = false;
                self.remote_tx = None;
                self.status_message = "Remote mode is not active".to_string();
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
        let Some(port_name) = self.active_protocol_port() else {
            return;
        };
        let tx = self.event_tx.clone();
        let endian = self.endian;
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let channels = match load_channels_from_path(&path) {
                Ok(channels) => channels,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to load channels: {}", e)));
                    return;
                }
            };

            match write_radio_channels(&port_name, &channels, &[], endian, false, |event| {
                match event {
                    ProgressEvent::Status(status) => {
                        let _ = tx.send(AppEvent::Status(status));
                    }
                    ProgressEvent::Progress(progress) => {
                        let _ = tx.send(AppEvent::Progress(progress));
                    }
                }
            }) {
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
        self.channels = load_channels_from_path(path)?;
        self.channel_state.select(Some(0));
        self.log(&format!("Loaded {} channels from CSV", self.channels.len()));
        Ok(())
    }

    pub fn start_import_and_write(&mut self, path: PathBuf) {
        match self.load_csv(&path) {
            Ok(_) => {
                self.start_write_multiple_channels(0, true);
            }
            Err(e) => {
                self.mode = AppMode::Error(format!("Failed to load CSV: {}", e));
            }
        }
    }

    pub fn export_csv(&mut self, path: PathBuf) -> Result<()> {
        save_channels_to_path(&path, &self.channels)?;
        self.log(&format!("Exported {} channels to CSV", self.channels.len()));
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

    fn show_save_dialog<F>(
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

    pub fn start_import_codeplug(&mut self, path: PathBuf) {
        use crate::protocol::codeplug;

        let endian = self.endian;
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
                    let channels = codeplug::extract_channels_from_codeplug(&data, endian);
                    let channel_count = channels.len();
                    let _ = tx.send(AppEvent::Status("Extracting channels...".to_string()));
                    let _ = tx.send(AppEvent::Progress(0.6));
                    let settings = codeplug::extract_settings_from_codeplug(&data, endian);
                    let settings_present = settings.is_some();
                    let _ = tx.send(AppEvent::Status("Extracting settings...".to_string()));
                    let _ = tx.send(AppEvent::Progress(0.8));
                    let scan_presets = codeplug::extract_scan_presets_from_codeplug(&data, endian);
                    let group_labels = codeplug::extract_group_labels_from_codeplug(&data);
                    let scan_preset_count = scan_presets.len();
                    let _ = tx.send(AppEvent::Progress(1.0));
                    let _ = tx.send(AppEvent::CodeplugDataLoaded {
                        path,
                        data,
                        channels,
                        settings,
                        scan_presets,
                        group_labels: group_labels.clone(),
                    });
                    let _ = tx.send(AppEvent::Status(format!(
                        "Codeplug loaded: {} channels, {} scan presets, {} named groups, settings {}",
                        channel_count,
                        scan_preset_count,
                        group_labels
                            .iter()
                            .filter(|label| !label.trim().is_empty())
                            .count(),
                        if settings_present {
                            "present"
                        } else {
                            "missing"
                        }
                    )));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to load codeplug: {}", e)));
                }
            }
        });
    }

    pub fn start_export_codeplug(&self, path: PathBuf) {
        use crate::protocol::codeplug;

        let has_channels = !self.channels.is_empty();
        let has_settings = self.settings.is_some();

        if !has_channels || !has_settings {
            let _ = self.event_tx.send(AppEvent::Error(
                "Please read channels and settings from radio before exporting codeplug"
                    .to_string(),
            ));
            return;
        }

        let channels = self.channels.clone();
        let settings = self.settings.clone().unwrap();
        let scan_presets = self.scan_presets.clone();
        let band_plans = self.band_plans.clone();
        let dtmf_presets = self.dtmf_presets.clone();
        let group_labels = self.group_labels.clone();
        let endian = self.endian;
        let tx = self.event_tx.clone();

        thread::spawn(move || {
            let _ = tx.send(AppEvent::Status("Creating codeplug...".to_string()));

            let codeplug_data = codeplug::create_codeplug(
                &channels,
                &settings,
                &scan_presets,
                &band_plans,
                &dtmf_presets,
                &group_labels,
                endian,
            );

            match codeplug::save_codeplug(&path, &codeplug_data) {
                Ok(_) => {
                    let _ = tx.send(AppEvent::Status(format!(
                        "Codeplug exported to {}",
                        path.display()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to save codeplug: {}", e)));
                }
            }
        });
    }

    pub fn start_write_codeplug(&mut self) {
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

    #[allow(unused_assignments)]
    pub fn start_bin_flash(&mut self) {
        let firmware_data = match &self.bin_firmware_data {
            Some(data) => data.clone(),
            None => {
                self.mode = AppMode::Error(
                    "No firmware loaded. Press 'i' to import a firmware file.".to_string(),
                );
                return;
            }
        };

        let Some(port_name) = self.active_protocol_port() else {
            self.mode =
                AppMode::Error("Not connected to a port. Please select a port first.".to_string());
            return;
        };

        let tx = self.event_tx.clone();
        self.progress = 0.0;
        let _ = tx.send(AppEvent::Status(
            "Waiting for radio handshake (0xA5)...".to_string(),
        ));
        self.mode = AppMode::BinFlashing;

        thread::spawn(move || {
            const INIT_SEQUENCE: [u8; 36] = [
                0xA0, 0xEE, 0x74, 0x71, 0x07, 0x74, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
                0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
                0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
            ];

            let rounded_len = firmware_data.len().div_ceil(32) * 32;
            if rounded_len > 0xf800 {
                let _ = tx.send(AppEvent::BinFlashFailed(
                    "Firmware file too large".to_string(),
                ));
                return;
            }
            let last_block = (rounded_len / 32) as u16;

            let _ = tx.send(AppEvent::Status("Opening serial port...".to_string()));

            let mut proto = match RadioProtocol::new_with_baud(&port_name, BIN_FLASH_BAUD_RATE) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::BinFlashFailed(format!(
                        "Failed to open port: {}",
                        e
                    )));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status(
                "Turn OFF radio, hold PTT(H3) or Flashlight(H8), turn ON radio with button held"
                    .to_string(),
            ));

            let mut detected = false;
            let mut flashing = false;
            let mut block: u16 = 0;
            let mut need_to_send_block = true;
            let mut last_block_sent = false;
            let mut a5_count = 0;
            let init_sent_time = std::time::Instant::now();
            let mut block_send_time = std::time::Instant::now();

            let start_time = std::time::Instant::now();
            let timeout_secs = 60;

            loop {
                if start_time.elapsed().as_secs() > timeout_secs {
                    let _ = tx.send(AppEvent::BinFlashFailed(
                        "Timeout waiting for radio".to_string(),
                    ));
                    return;
                }

                if flashing && need_to_send_block && block <= last_block {
                    let is_last = block == last_block;
                    let _ = tx.send(AppEvent::Status(format!(
                        "Flashing block: {} / {}",
                        block, last_block
                    )));

                    let mut packet = [0u8; 36];
                    packet[0] = if is_last { 0xA2 } else { 0xA1 };
                    packet[1] = ((block >> 8) & 0xFF) as u8;
                    packet[2] = (block & 0xFF) as u8;
                    let start_idx = (block as usize) * 32;
                    if start_idx + 32 <= firmware_data.len() {
                        packet[4..36].copy_from_slice(&firmware_data[start_idx..start_idx + 32]);
                    } else {
                        packet[4..4 + (firmware_data.len() - start_idx)]
                            .copy_from_slice(&firmware_data[start_idx..]);
                    }
                    let checksum = packet[4..36]
                        .iter()
                        .fold(0u8, |sum, &byte| sum.wrapping_add(byte));
                    packet[3] = checksum;

                    if let Err(e) = proto.send_bytes(&packet) {
                        let _ = tx.send(AppEvent::BinFlashFailed(format!("Send failed: {}", e)));
                        return;
                    }

                    let progress = (block + 1) as f64 / (last_block as f64 + 1.0);
                    let _ = tx.send(AppEvent::Progress(progress));

                    block += 1;
                    need_to_send_block = false;
                    block_send_time = std::time::Instant::now();

                    if is_last {
                        last_block_sent = true;
                    }
                }

                if last_block_sent
                    && !need_to_send_block
                    && block_send_time.elapsed() > Duration::from_millis(500)
                {
                    let _ = tx.send(AppEvent::Status(
                        "Flashing complete! Closing port...".to_string(),
                    ));
                    let _ = tx.send(AppEvent::Progress(1.0));

                    drop(proto);

                    std::thread::sleep(Duration::from_millis(200));

                    let _ = tx.send(AppEvent::Status("SUCCESS! Firmware flashed.\n\nTurn radio OFF then ON to boot new firmware.".to_string()));

                    let _ = tx.send(AppEvent::BinFlashComplete);
                    return;
                }

                if flashing
                    && !need_to_send_block
                    && !last_block_sent
                    && block_send_time.elapsed() > Duration::from_millis(500)
                {
                    block -= 1;
                    need_to_send_block = true;
                }

                match proto.read_byte() {
                    Ok(Some(byte)) => {
                        if byte == 0xA5 {
                            a5_count += 1;
                            if !detected && a5_count >= 3 {
                                detected = true;
                                let _ = tx.send(AppEvent::Status(
                                    "Handshake detected, sending init sequence...".to_string(),
                                ));
                                if let Err(e) = proto.send_bytes(&INIT_SEQUENCE) {
                                    let _ = tx.send(AppEvent::BinFlashFailed(format!(
                                        "Send failed: {}",
                                        e
                                    )));
                                    return;
                                }
                                a5_count = 0;
                            }
                        } else if byte == 0xA3 {
                            if detected
                                && !flashing
                                && init_sent_time.elapsed() > Duration::from_millis(100)
                            {
                                flashing = true;
                                let _ = tx.send(AppEvent::Status(
                                    "Radio ready, starting flash...".to_string(),
                                ));
                            } else if flashing && !need_to_send_block && !last_block_sent {
                                need_to_send_block = true;
                            }
                        }
                    }
                    Ok(None) => {
                        if detected
                            && !flashing
                            && init_sent_time.elapsed() > Duration::from_millis(400)
                        {
                            flashing = true;
                            let _ = tx.send(AppEvent::Status(
                                "Radio ready (timeout), starting flash...".to_string(),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::BinFlashFailed(format!("Read error: {}", e)));
                        return;
                    }
                }
            }
        });
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
        0x11 => "V/M",
        0x12 => "light",
        0x13 => "A/PTT",
        0x1A => "B/PTT",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
