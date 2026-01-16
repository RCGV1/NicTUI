use ratatui::widgets::TableState;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use crate::protocol::Endianness;

pub mod actions;
pub mod handlers;
pub mod navigation;
pub mod state;

pub use state::*;

impl App {
    pub fn new() -> Self {
        let ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.port_name)
            .collect();

        let (tx, rx) = mpsc::channel();

        Self {
            mode: AppMode::PortSelection,
            ports,
            selected_port_index: 0,
            channels: Vec::new(),
            deleted_channels: Vec::new(),
            channel_state: TableState::default(),
            scan_presets: Vec::new(),
            preset_state: TableState::default(),
            editing_scan_preset: None,
            scanning_group_state: TableState::default(),
            scanning_focus: 0,
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
            status_message: "Select a serial port to begin".to_string(),
            logs: Vec::new(),
            endian: Endianness::Big,
            edit_buffer: String::new(),
            selection_index: 0,
            event_tx: tx,
            event_rx: rx,
            remote_active: false,
            remote_stop_signal: Arc::new(AtomicBool::new(false)),
            remote_tx: None,
            last_main_tab: MainTab::Channels,
            settings_dirty: false,
            channels_dirty: false,
            dtmf_dirty: false,
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
        self.logs.push(format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            message
        ));
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
    }

    pub fn refresh_ports(&mut self) {
        self.ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.port_name)
            .collect();
        if self.selected_port_index >= self.ports.len() && !self.ports.is_empty() {
            self.selected_port_index = 0;
        }
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
