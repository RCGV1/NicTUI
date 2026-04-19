use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::state::{App, AppEvent, AppMode, MainTab};
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

impl App {
    pub fn select_port(&mut self) {
        if self.ports.is_empty() {
            self.mode = AppMode::Error("No serial ports found".to_string());
            return;
        }

        let port_name = self.ports[self.selected_port_index].clone();
        let was_detected_radio = self
            .selected_port_candidate()
            .map(|candidate| candidate.is_radio())
            .unwrap_or(false);
        self.protocol_port_name = Some(port_name.clone());
        self.mode = AppMode::Main(MainTab::Channels);
        self.status_message = if was_detected_radio {
            format!(
                "Using detected radio port {}. Radio opens on first action.",
                port_name
            )
        } else {
            format!("Selected {}. Radio opens on first action.", port_name)
        };
        self.log(&format!("Selected port {}", port_name));
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
                    let _ = tx.send(AppEvent::WriteComplete);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to clear channel: {}", e)));
                }
            }
        });
    }

    pub fn start_read_channels(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
                    let _ = tx.send(AppEvent::WriteComplete);
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
                    let _ = tx.send(AppEvent::WriteComplete);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to write DTMF: {}", e)));
                }
            }
        });
    }

    pub fn start_read_settings(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let settings = match &self.settings {
            Some(s) => s.clone(),
            None => return,
        };
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
                    let _ = tx.send(AppEvent::WriteComplete);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to write settings: {}", e)));
                }
            }
        });
    }

    pub fn remote_on(&mut self) {
        if self.remote_active {
            self.status_message = "Remote mode already active".to_string();
            return;
        }

        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let (key_tx, key_rx) = mpsc::channel();
        self.remote_screen = Default::default();
        self.remote_tx = Some(key_tx);
        self.remote_active = true;
        self.status_message = "Starting remote mode...".to_string();
        self.remote_stop_signal.store(false, Ordering::SeqCst);
        let stop_signal = self.remote_stop_signal.clone();

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::RemoteStopped(format!(
                        "Failed to open remote port: {}",
                        e
                    )));
                    return;
                }
            };

            let log_tx = tx.clone();
            proto.log_callback = Some(Box::new(move |message| {
                let _ = log_tx.send(AppEvent::Log(message));
            }));

            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::RemoteStopped(
                    "Remote handshake failed".to_string(),
                ));
                return;
            }

            if !proto.remote_on().unwrap_or(false) {
                let _ = tx.send(AppEvent::RemoteStopped(
                    "Radio rejected remote mode".to_string(),
                ));
                return;
            }

            let _ = tx.send(AppEvent::Status("Remote mode ON".to_string()));

            loop {
                if stop_signal.load(Ordering::SeqCst) {
                    let _ = proto.remote_off();
                    let _ = tx.send(AppEvent::RemoteStopped("Remote mode OFF".to_string()));
                    break;
                }

                let mut key_failure = None;
                while let Ok(key) = key_rx.try_recv() {
                    if let Err(e) = proto.press_remote_key(key) {
                        key_failure = Some(format!("Failed to send remote key: {}", e));
                        break;
                    }
                }
                if let Some(message) = key_failure {
                    let _ = tx.send(AppEvent::RemoteStopped(message));
                    break;
                }

                match proto.parse_remote_packet() {
                    Ok(Some(pkt)) => {
                        let _ = tx.send(AppEvent::RemotePacket(pkt));
                    }
                    Ok(None) => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::RemoteStopped(format!(
                            "Remote connection lost: {}",
                            e
                        )));
                        break;
                    }
                }
            }
        });
    }

    pub fn remote_off(&mut self) {
        if self.remote_active {
            self.remote_stop_signal.store(true, Ordering::SeqCst);
            self.remote_active = false;
            self.remote_tx = None;
            self.status_message = "Stopping remote mode...".to_string();
        }
    }

    pub fn send_key(&mut self, key_code: u8) {
        if let Some(tx) = &self.remote_tx {
            if tx.send(key_code).is_ok() {
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
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
                    let _ = tx.send(AppEvent::WriteComplete);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to write channel: {}", e)));
                }
            }
        });
    }

    pub fn start_write_multiple_channels(&mut self, _start_index: usize, reboot: bool) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
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
                    let _ = tx.send(AppEvent::WriteComplete);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to update channels: {}", e)));
                }
            }
        });
    }

    pub fn start_write_csv_channels(&mut self, path: PathBuf) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
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
                    let _ = tx.send(AppEvent::WriteComplete);
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

        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => {
                let _ = self
                    .event_tx
                    .send(AppEvent::Error("Not connected to radio".to_string()));
                return;
            }
        };

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
                    let _ = tx.send(AppEvent::WriteComplete);
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

        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => {
                self.mode = AppMode::Error(
                    "Not connected to a port. Please select a port first.".to_string(),
                );
                return;
            }
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
        0x80 => "0",
        0x81 => "1",
        0x82 => "2",
        0x83 => "3",
        0x84 => "4",
        0x85 => "5",
        0x86 => "6",
        0x87 => "7",
        0x88 => "8",
        0x89 => "9",
        0x8A => "menu",
        0x8B => "up",
        0x8C => "down",
        0x8D => "exit",
        0x8E => "*",
        0x8F => "#",
        0x90 => "A/PTT",
        0x91 => "B/PTT",
        0x92 => "light",
        0x94 => "V/M",
        _ => "unknown",
    }
}
