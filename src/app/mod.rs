use ratatui::widgets::TableState;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;

use crate::ble::{
    BleDevice, ble_error_suggests_permission_block, ble_scan_supported, default_scan_timeout,
    scan_td_h3_ble_devices,
};
use crate::device::{PortCandidate, PortKind, list_port_candidates};
use crate::protocol::{Endianness, GROUP_LABEL_COUNT};

const BLUETOOTH_SETTING_INDEX: usize = 30;

pub mod actions;
pub mod bandplan;
pub mod channels;
pub mod dtmf;
pub mod group_labels;
pub mod handlers;
pub mod navigation;
pub mod scan_presets;
pub mod settings;
pub mod state;

pub use state::*;

impl App {
    pub fn new() -> Self {
        let serial_port_candidates = list_port_candidates().unwrap_or_default();
        let ble_port_candidates = Vec::new();
        let port_candidates =
            Self::merge_port_candidates(&serial_port_candidates, &ble_port_candidates);
        let ports = Self::candidate_names(&port_candidates);
        let selected_port_index = Self::preferred_port_index(&port_candidates, None);
        let status_message = Self::port_selection_message(&port_candidates);

        let (tx, rx) = mpsc::channel();

        Self {
            serial_port_candidates,
            ble_port_candidates,
            mode: AppMode::PortSelection,
            port_candidates,
            ports,
            selected_port_index,
            channels: Vec::new(),
            deleted_channels: Vec::new(),
            channel_state: TableState::default(),
            group_labels: vec![String::new(); GROUP_LABEL_COUNT],
            scan_presets: Vec::new(),
            preset_state: TableState::default(),
            editing_scan_preset: None,
            editing_group_label_idx: None,
            scanning_group_state: TableState::default(),
            band_plans: Vec::new(),
            bandplan_state: TableState::default(),
            editing_band_plan: None,
            dtmf_presets: Vec::new(),
            dtmf_state: TableState::default(),
            settings: None,
            settings_state: TableState::default(),
            remote_screen: RemoteScreen::default(),
            protocol_port_name: None,
            ble_scan_in_progress: false,
            ble_scan_ui_suspended: false,
            ble_reconnect_required: false,
            progress: 0.0,
            status_message,
            logs: VecDeque::new(),
            endian: Endianness::Big,
            edit_buffer: String::new(),
            selection_index: 0,
            event_tx: tx,
            event_rx: rx,
            remote_active: false,
            remote_stop_signal: Arc::new(AtomicBool::new(false)),
            remote_tx: None,
            last_main_tab: MainTab::Channels,
            last_non_remote_tab: MainTab::Channels,
            settings_dirty: false,
            channels_dirty: false,
            dtmf_dirty: false,
            group_labels_dirty: false,
            codeplug_data: None,
            codeplug_path: None,
            bin_firmware_data: None,
            bin_file_path: None,
            dialog_open: false,
            pending_channel_edit: None,
            dtmf_edit_preset_idx: None,
        }
    }

    pub fn log(&mut self, message: &str) {
        self.logs.push_back(format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            message
        ));
        if self.logs.len() > 100 {
            self.logs.pop_front();
        }
    }

    pub fn refresh_ports(&mut self) {
        let current = self.ports.get(self.selected_port_index).cloned();
        self.serial_port_candidates = list_port_candidates().unwrap_or_default();
        self.rebuild_port_candidates(current.as_deref());
        if self.mode == AppMode::PortSelection {
            self.status_message = Self::port_selection_message(&self.port_candidates);
        }
    }

    pub fn refresh_radio_targets(&mut self) {
        self.refresh_ports();
        self.start_ble_scan(cfg!(target_os = "macos"));
    }

    pub fn start_ble_scan(&mut self, user_initiated: bool) {
        if self.ble_scan_in_progress {
            if user_initiated {
                self.status_message = "BLE scan already running...".to_string();
            }
            return;
        }

        if !ble_scan_supported() {
            if user_initiated {
                self.status_message =
                    "BLE scan is unavailable on this system or blocked by Bluetooth permission."
                        .to_string();
            }
            return;
        }

        self.ble_scan_in_progress = true;
        if self.mode == AppMode::PortSelection {
            self.status_message = if user_initiated {
                "Scanning for TD-H3 BLE radios...".to_string()
            } else {
                "Auto-detecting nearby TD-H3 BLE radios...".to_string()
            };
        }

        if cfg!(target_os = "macos") && user_initiated && !self.ble_scan_ui_suspended {
            self.dialog_open = true;
            self.ble_scan_ui_suspended = true;
            self.suspend_ui();
        }

        let tx = self.event_tx.clone();
        thread::spawn(
            move || match scan_td_h3_ble_devices(default_scan_timeout()) {
                Ok(devices) => {
                    let candidates = devices.iter().map(App::ble_port_candidate).collect();
                    let _ = tx.send(AppEvent::BleScanComplete(candidates));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::BleScanFailed(error.to_string()));
                }
            },
        );
    }

    pub fn selected_port_label(&self) -> String {
        if let Some(candidate) = self.selected_port_candidate()
            && (candidate.is_ble() || self.protocol_port_name.is_none())
        {
            return candidate.port_name.clone();
        }

        self.protocol_port_name
            .clone()
            .or_else(|| {
                self.selected_port_candidate()
                    .map(|candidate| candidate.port_name.clone())
            })
            .unwrap_or_else(|| "not selected".to_string())
    }

    pub fn selected_port_short_label(&self) -> String {
        Self::display_port_name(&self.selected_port_label())
    }

    pub(crate) fn display_port_name(label: &str) -> String {
        let short = label.rsplit('/').next().unwrap_or(label);
        short
            .strip_prefix("cu.")
            .or_else(|| short.strip_prefix("tty."))
            .unwrap_or(short)
            .to_string()
    }

    pub fn selected_port_status(&self) -> String {
        let Some(candidate) = self.selected_port_candidate() else {
            return "No radio target".to_string();
        };

        if candidate.is_ble() {
            let connection_state =
                if self.protocol_port_name.is_some() && !self.ble_reconnect_required {
                    "BLE ready"
                } else if self.ble_reconnect_required {
                    "BLE reconnect needed"
                } else {
                    "BLE target"
                };

            return match candidate.ble_rssi {
                Some(rssi) => format!("{connection_state} {rssi} dBm"),
                None => connection_state.to_string(),
            };
        }

        if candidate.is_radio() {
            return if self.protocol_port_name.is_some() {
                "USB ready".to_string()
            } else {
                "USB target".to_string()
            };
        }

        "Target selected".to_string()
    }

    pub fn ble_transport_selected(&self) -> bool {
        self.selected_port_candidate()
            .is_some_and(|candidate| candidate.is_ble())
    }

    pub fn selected_ble_device_id(&self) -> Option<&str> {
        self.selected_port_candidate()
            .and_then(|candidate| candidate.ble_device_id.as_deref())
    }

    fn rebuild_port_candidates(&mut self, current_port_name: Option<&str>) {
        self.port_candidates =
            Self::merge_port_candidates(&self.serial_port_candidates, &self.ble_port_candidates);
        self.ports = Self::candidate_names(&self.port_candidates);
        self.selected_port_index =
            Self::preferred_port_index(&self.port_candidates, current_port_name);
    }

    fn merge_port_candidates(
        serial_port_candidates: &[PortCandidate],
        ble_port_candidates: &[PortCandidate],
    ) -> Vec<PortCandidate> {
        let mut merged = serial_port_candidates.to_vec();
        merged.extend_from_slice(ble_port_candidates);
        merged
    }

    fn candidate_names(candidates: &[PortCandidate]) -> Vec<String> {
        candidates
            .iter()
            .map(|candidate| candidate.port_name.clone())
            .collect()
    }

    fn ble_port_candidate(device: &BleDevice) -> PortCandidate {
        let alias = ble_alias(&device.device_id);
        let name = device.name.as_deref().unwrap_or("TD-H3 BLE");

        PortCandidate {
            port_name: format!("{name} {alias}"),
            kind: PortKind::Ble,
            score: 900,
            product: Some("BLE".to_string()),
            manufacturer: Some("TD-H3".to_string()),
            usb_vid: None,
            usb_pid: None,
            ble_device_id: Some(device.device_id.clone()),
            ble_rssi: device.rssi,
            handshake_ok: false,
            firmware_variant: None,
        }
    }

    pub fn connect_to_port_by_name(&mut self, port_name: &str) {
        match self
            .ports
            .iter()
            .position(|candidate| candidate == port_name)
        {
            Some(index) => self.selected_port_index = index,
            None => {
                self.ports.push(port_name.to_string());
                self.selected_port_index = self.ports.len().saturating_sub(1);
            }
        }
        if !self
            .port_candidates
            .iter()
            .any(|candidate| candidate.port_name == port_name)
        {
            self.refresh_ports();
            match self
                .ports
                .iter()
                .position(|candidate| candidate == port_name)
            {
                Some(index) => self.selected_port_index = index,
                None => {
                    self.port_candidates.push(PortCandidate {
                        port_name: port_name.to_string(),
                        kind: PortKind::Candidate,
                        score: 0,
                        product: None,
                        manufacturer: None,
                        usb_vid: None,
                        usb_pid: None,
                        ble_device_id: None,
                        ble_rssi: None,
                        handshake_ok: false,
                        firmware_variant: None,
                    });
                    self.ports.push(port_name.to_string());
                    self.selected_port_index = self.ports.len().saturating_sub(1);
                }
            }
        }
        self.select_port();
    }

    pub fn selected_port_candidate(&self) -> Option<&PortCandidate> {
        self.port_candidates.get(self.selected_port_index)
    }

    fn port_selection_message(candidates: &[PortCandidate]) -> String {
        let radio_ports: Vec<&PortCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.is_radio())
            .collect();
        let ble_ports: Vec<&PortCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.is_ble())
            .collect();
        match candidates.len() {
            0 => "No radio targets found. Connect USB and press r, or press b to scan for BLE."
                .to_string(),
            _ if radio_ports.len() == 1 && ble_ports.is_empty() => format!(
                "Ready: {}. Press Enter to continue.",
                Self::display_port_name(&radio_ports[0].port_name)
            ),
            _ if radio_ports.is_empty() && ble_ports.len() == 1 => format!(
                "Ready: BLE {}. Press Enter to connect.",
                ble_ports[0].port_name
            ),
            _ if radio_ports.len() > 1 && ble_ports.is_empty() => format!(
                "{} USB radios detected. Use ↑/↓ to choose one.",
                radio_ports.len()
            ),
            _ if ble_ports.len() > 1 && radio_ports.is_empty() => format!(
                "{} BLE radios detected. Use ↑/↓ to choose one, then press Enter.",
                ble_ports.len()
            ),
            1 => format!(
                "Ready: {}. Press Enter to connect.",
                Self::display_port_name(&candidates[0].port_name)
            ),
            count => format!(
                "{count} targets detected. USB is the clearest path today; BLE is available when radio Bluetooth is on."
            ),
        }
    }

    fn ble_scan_failure_message(error: &str, has_cached_results: bool) -> String {
        let cached_suffix = if has_cached_results {
            " Showing last successful BLE results."
        } else {
            ""
        };

        if ble_error_suggests_permission_block(error) {
            format!(
                "BLE scan looks blocked by macOS Bluetooth permission. Open NicTUI.app once, allow Bluetooth, then scan again.{cached_suffix}"
            )
        } else {
            format!(
                "BLE scan failed: {error}. Keep Bluetooth enabled on the radio and scan again.{cached_suffix}"
            )
        }
    }

    pub fn ble_target_count(&self) -> usize {
        self.ble_port_candidates.len()
    }

    pub fn ble_readiness_overview(&self) -> (String, String) {
        if !ble_scan_supported() {
            return (
                "BLE scan unavailable on this platform".to_string(),
                "Use USB today, or try BLE from a system that supports Bluetooth scanning."
                    .to_string(),
            );
        }

        if self.ble_scan_in_progress {
            return (
                "Checking nearby TD-H3 radios".to_string(),
                "Wait for the scan to finish. On macOS, open NicTUI.app once if Bluetooth permission is needed."
                    .to_string(),
            );
        }

        if let Some(candidate) = self
            .selected_port_candidate()
            .filter(|candidate| candidate.is_ble())
        {
            let summary = if self.protocol_port_name.is_some() && !self.ble_reconnect_required {
                "BLE radio selected"
            } else if self.ble_reconnect_required {
                "BLE radio selected; reconnect needed"
            } else {
                "BLE radio selected"
            };

            let hint = match candidate.ble_rssi {
                Some(rssi) => format!("Signal {rssi} dBm. Press Enter to connect."),
                None => "Press Enter to connect.".to_string(),
            };

            return (summary.to_string(), hint);
        }

        if self.ble_target_count() > 0 {
            return (
                format!("{} BLE radio(s) visible", self.ble_target_count()),
                "Use ↑/↓ to choose one, then press Enter to connect.".to_string(),
            );
        }

        if self
            .settings
            .as_ref()
            .is_some_and(|settings| settings.get_value(BLUETOOTH_SETTING_INDEX) == 0)
        {
            return (
                "Radio Bluetooth setting is off".to_string(),
                "Turn Bluetooth on from the Settings tab over USB, then press `b` to scan again."
                    .to_string(),
            );
        }

        if cfg!(target_os = "macos") {
            return (
                "No BLE radios visible yet".to_string(),
                "Press `b` to scan again. On macOS, open NicTUI.app once if Bluetooth permission is needed."
                    .to_string(),
            );
        }

        (
            "No BLE radios visible yet".to_string(),
            "Press `b` to scan again. Make sure Bluetooth is enabled on the radio.".to_string(),
        )
    }

    fn preferred_port_index(
        candidates: &[PortCandidate],
        current_port_name: Option<&str>,
    ) -> usize {
        if let Some(current_port_name) = current_port_name
            && let Some(index) = candidates
                .iter()
                .position(|candidate| candidate.port_name == current_port_name)
        {
            return index;
        }

        candidates
            .iter()
            .position(|candidate| candidate.is_radio())
            .or_else(|| candidates.iter().position(|candidate| candidate.is_ble()))
            .or_else(|| {
                candidates
                    .iter()
                    .position(|candidate| matches!(candidate.kind, PortKind::Candidate))
            })
            .unwrap_or(0)
    }

    pub fn suspend_ui(&self) {
        let _ = crossterm::terminal::disable_raw_mode();

        let mut stdout = std::io::stdout();
        let _ = crossterm::execute!(
            stdout,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,
            crossterm::style::ResetColor
        );

        let _ = stdout.flush();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    pub fn resume_ui(&self) {
        use std::time::Duration;

        let mut stdout = std::io::stdout();

        if cfg!(target_os = "macos") {
            std::thread::sleep(Duration::from_millis(150));

            let _ = crossterm::execute!(
                stdout,
                crossterm::cursor::Show,
                crossterm::style::ResetColor
            );

            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = crossterm::execute!(
            stdout,
            crossterm::event::EnableMouseCapture,
            crossterm::cursor::Hide
        );

        let _ = crossterm::terminal::enable_raw_mode();

        std::thread::sleep(Duration::from_millis(50));

        for _ in 0..20 {
            match crossterm::event::poll(Duration::from_millis(10)) {
                Ok(true) => {
                    let _ = crossterm::event::read();
                }
                Ok(false) => break,
                Err(_) => break,
            }
        }

        let _ = stdout.flush();
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn ble_alias(device_id: &str) -> String {
    let compact = device_id
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .collect::<String>();
    let suffix = if compact.len() >= 4 {
        &compact[compact.len() - 4..]
    } else if compact.is_empty() {
        "BLE"
    } else {
        compact.as_str()
    };
    format!("#{suffix}").to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ble_candidate(name: &str, device_id: &str) -> PortCandidate {
        PortCandidate {
            port_name: name.to_string(),
            kind: PortKind::Ble,
            score: 900,
            product: Some("BLE".to_string()),
            manufacturer: Some("BLE".to_string()),
            usb_vid: None,
            usb_pid: None,
            ble_device_id: Some(device_id.to_string()),
            ble_rssi: Some(-69),
            handshake_ok: false,
            firmware_variant: None,
        }
    }

    #[test]
    fn preferred_port_index_chooses_ble_before_generic_serial_candidates() {
        let candidates = vec![
            PortCandidate {
                port_name: "/dev/cu.usbmodem123".to_string(),
                kind: PortKind::Candidate,
                score: 100,
                product: None,
                manufacturer: None,
                usb_vid: None,
                usb_pid: None,
                ble_device_id: None,
                ble_rssi: None,
                handshake_ok: false,
                firmware_variant: None,
            },
            ble_candidate("TD-H3 #5678", "12345678-1234-5678-9ABC-DEF012345678"),
        ];

        assert_eq!(App::preferred_port_index(&candidates, None), 1);
    }

    #[test]
    fn ble_scan_complete_selects_found_ble_target_from_usb_selection() {
        let mut app = App::new();
        app.serial_port_candidates = vec![PortCandidate {
            port_name: "/dev/cu.usbmodem123".to_string(),
            kind: PortKind::Radio,
            score: 1000,
            product: Some("TD-H3".to_string()),
            manufacturer: Some("QYT".to_string()),
            usb_vid: Some(0x0483),
            usb_pid: Some(0x5740),
            ble_device_id: None,
            ble_rssi: None,
            handshake_ok: true,
            firmware_variant: None,
        }];
        app.ble_port_candidates = Vec::new();
        app.rebuild_port_candidates(Some("/dev/cu.usbmodem123"));

        app.event_tx
            .send(AppEvent::BleScanComplete(vec![ble_candidate(
                "TD-H3 #5678",
                "12345678-1234-5678-9ABC-DEF012345678",
            )]))
            .unwrap();
        app.update();

        assert_eq!(app.selected_port_label(), "TD-H3 #5678");
    }

    #[test]
    fn ble_scan_complete_preserves_existing_ble_selection_when_still_visible() {
        let mut app = App::new();
        app.serial_port_candidates = vec![PortCandidate {
            port_name: "/dev/cu.usbmodem123".to_string(),
            kind: PortKind::Radio,
            score: 1000,
            product: Some("TD-H3".to_string()),
            manufacturer: Some("QYT".to_string()),
            usb_vid: Some(0x0483),
            usb_pid: Some(0x5740),
            ble_device_id: None,
            ble_rssi: None,
            handshake_ok: true,
            firmware_variant: None,
        }];
        app.ble_port_candidates = vec![ble_candidate(
            "TD-H3 #AAAA",
            "12345678-1234-5678-9ABC-DEF01234AAAA",
        )];
        app.rebuild_port_candidates(Some("TD-H3 #AAAA"));

        app.event_tx
            .send(AppEvent::BleScanComplete(vec![
                ble_candidate("TD-H3 #BBBB", "12345678-1234-5678-9ABC-DEF01234BBBB"),
                ble_candidate("TD-H3 #AAAA", "12345678-1234-5678-9ABC-DEF01234AAAA"),
            ]))
            .unwrap();
        app.update();

        assert_eq!(app.selected_port_label(), "TD-H3 #AAAA");
    }

    #[test]
    fn selected_port_label_prefers_ble_target_name_over_transport_path() {
        let mut app = App::new();
        app.port_candidates = vec![ble_candidate(
            "TD-H3 #5678",
            "12345678-1234-5678-9ABC-DEF012345678",
        )];
        app.ports = vec!["TD-H3 #5678".to_string()];
        app.selected_port_index = 0;
        app.protocol_port_name = Some("/tmp/nictui-ble-12345678.tty".to_string());

        assert_eq!(app.selected_port_label(), "TD-H3 #5678");
    }

    #[test]
    fn selected_port_status_reflects_ble_signal_strength() {
        let mut app = App::new();
        app.port_candidates = vec![ble_candidate(
            "TD-H3 #5678",
            "12345678-1234-5678-9ABC-DEF012345678",
        )];
        app.ports = vec!["TD-H3 #5678".to_string()];
        app.selected_port_index = 0;

        assert_eq!(app.selected_port_status(), "BLE target -69 dBm");
    }

    #[test]
    fn selected_port_short_label_trims_paths() {
        let mut app = App::new();
        app.port_candidates = vec![PortCandidate {
            port_name: "/dev/cu.usbmodem123".to_string(),
            kind: PortKind::Radio,
            score: 1000,
            product: Some("TD-H3".to_string()),
            manufacturer: Some("QYT".to_string()),
            usb_vid: Some(0x0483),
            usb_pid: Some(0x5740),
            ble_device_id: None,
            ble_rssi: None,
            handshake_ok: true,
            firmware_variant: None,
        }];
        app.ports = vec!["/dev/cu.usbmodem123".to_string()];
        app.selected_port_index = 0;

        assert_eq!(app.selected_port_short_label(), "usbmodem123");
    }

    #[test]
    fn port_selection_message_uses_concise_ready_copy() {
        let candidates = vec![PortCandidate {
            port_name: "/dev/cu.usbmodem123".to_string(),
            kind: PortKind::Radio,
            score: 1000,
            product: Some("TD-H3".to_string()),
            manufacturer: Some("QYT".to_string()),
            usb_vid: Some(0x0483),
            usb_pid: Some(0x5740),
            ble_device_id: None,
            ble_rssi: None,
            handshake_ok: true,
            firmware_variant: None,
        }];

        assert_eq!(
            App::port_selection_message(&candidates),
            "Ready: usbmodem123. Press Enter to continue."
        );
    }

    #[test]
    fn port_selection_message_sets_expectation_for_ble_connection() {
        let candidates = vec![ble_candidate(
            "TD-H3 #5678",
            "12345678-1234-5678-9ABC-DEF012345678",
        )];

        assert_eq!(
            App::port_selection_message(&candidates),
            "Ready: BLE TD-H3 #5678. Press Enter to connect."
        );
    }

    #[test]
    fn ble_scan_failure_message_points_to_app_permission_for_permission_blocks() {
        let message = App::ble_scan_failure_message(
            "Timed out after 5s waiting for BLE adapter enumeration. On macOS this usually means CoreBluetooth never delivered its initial state update.",
            false,
        );

        assert!(message.contains("macOS Bluetooth permission"));
        assert!(message.contains("Open NicTUI.app once"));
    }

    #[test]
    fn ble_scan_failure_message_mentions_cached_results_when_available() {
        let message = App::ble_scan_failure_message("adapter busy", true);

        assert!(message.contains("Showing last successful BLE results."));
        assert!(message.contains("scan again"));
    }

    #[test]
    fn ble_readiness_overview_calls_out_disabled_radio_setting() {
        let mut app = App::new();
        let mut settings =
            crate::protocol::RadioProtocol::parse_settings_block(&[0; 0x67], Endianness::Big);
        settings.set_value(BLUETOOTH_SETTING_INDEX, 0);
        app.settings = Some(settings);

        let (summary, hint) = app.ble_readiness_overview();

        assert_eq!(summary, "Radio Bluetooth setting is off");
        assert!(hint.contains("Settings tab"));
        assert!(hint.contains("scan again"));
    }

    #[test]
    fn ble_readiness_overview_highlights_selected_ble_target() {
        let mut app = App::new();
        app.port_candidates = vec![ble_candidate(
            "TD-H3 #5678",
            "12345678-1234-5678-9ABC-DEF012345678",
        )];
        app.ble_port_candidates = app.port_candidates.clone();
        app.ports = vec!["TD-H3 #5678".to_string()];
        app.selected_port_index = 0;

        let (summary, hint) = app.ble_readiness_overview();

        assert_eq!(summary, "BLE radio selected");
        assert!(hint.contains("Signal -69 dBm"));
        assert!(hint.contains("Press Enter to connect"));
    }
}
