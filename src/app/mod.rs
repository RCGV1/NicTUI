use ratatui::widgets::TableState;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use crate::device::{PortCandidate, list_port_candidates};
use crate::protocol::{Endianness, GROUP_LABEL_COUNT};

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
        let port_candidates = list_port_candidates().unwrap_or_default();
        let ports = port_candidates
            .iter()
            .map(|candidate| candidate.port_name.clone())
            .collect::<Vec<_>>();
        let selected_port_index = Self::preferred_port_index(&port_candidates, None);
        let status_message = Self::port_selection_message(&port_candidates);

        let (tx, rx) = mpsc::channel();

        Self {
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
        self.port_candidates = list_port_candidates().unwrap_or_default();
        self.ports = self
            .port_candidates
            .iter()
            .map(|candidate| candidate.port_name.clone())
            .collect();
        self.selected_port_index =
            Self::preferred_port_index(&self.port_candidates, current.as_deref());
        if self.mode == AppMode::PortSelection {
            self.status_message = Self::port_selection_message(&self.port_candidates);
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
            if let Some(index) = self
                .ports
                .iter()
                .position(|candidate| candidate == port_name)
            {
                self.selected_port_index = index;
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
        match candidates.len() {
            0 => "No serial ports detected. Connect the radio and press r to refresh.".to_string(),
            _ if radio_ports.len() == 1 => format!(
                "Radio detected on {}. Press Enter to continue.",
                radio_ports[0].port_name
            ),
            _ if radio_ports.len() > 1 => {
                format!(
                    "{} responsive radios detected. Select one to begin.",
                    radio_ports.len()
                )
            }
            1 => format!(
                "1 serial port detected ({}). Press Enter to connect.",
                candidates[0].port_name
            ),
            count => {
                format!("{count} serial ports detected. The most likely radio port is preselected.")
            }
        }
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
            .or_else(|| {
                candidates.iter().position(|candidate| {
                    matches!(candidate.kind, crate::device::PortKind::Candidate)
                })
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
