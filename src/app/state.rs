use ratatui::widgets::TableState;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::device::PortCandidate;
use crate::protocol::{
    BandPlan, Channel, DTMFPreset, Endianness, RemotePacket, ScanPreset, SettingsBlock,
};
use crate::remote::{
    RemoteControlCommand, RemoteControlReport, RemoteSessionFailure, RemoteSessionPhase,
};

#[derive(PartialEq, Clone, Debug)]
pub enum AppMode {
    PortSelection,
    Main(MainTab),
    Reading,
    Writing,
    BinFlashing,
    EditChannel(usize),
    EditSetting(usize),
    EditDTMF(usize),
    EditScanPreset(usize),
    EditGroupLabel(usize),
    EditBandPlan(usize),
    DeleteChannelConfirm(usize),
    Error(String),
}

#[derive(Clone, Default)]
pub struct RemoteScreen {
    pub display: crate::remote_display::RadioDisplay,
    pub simulator_lcd_error: Option<String>,
    /// Ordered UART reconstruction retained while the native simulator LCD is shown.
    pub simulator_uart_display: Option<crate::remote_display::RadioDisplay>,
    /// Drawing observed this session, not evidence of a complete screen snapshot.
    pub has_display_updates: bool,
    /// Non-battery drawing observed this session, not proof of a full snapshot.
    pub has_non_battery_drawings: bool,
    pub show_diagnostics: bool,
    pub elements: VecDeque<RemotePacket>,
    pub signal_strength: u8,
    pub noise_level: u8,
    pub leds: u8,
    pub battery_level: Option<u8>,
    pub battery_text: Option<String>,
    pub last_small_status: Option<(u8, u8, u8)>,
    pub last_signal_update: Option<std::time::Instant>,
    pub last_noise_update: Option<std::time::Instant>,
    pub last_battery_update: Option<std::time::Instant>,
    pub last_text_update: Option<std::time::Instant>,
    pub last_status_update: Option<std::time::Instant>,
    pub last_led_update: Option<std::time::Instant>,
    pub last_received: Option<std::time::Instant>,
    pub phase: RemoteSessionPhase,
    pub last_control_report: Option<RemoteControlReport>,
    pub last_failure: Option<RemoteSessionFailure>,
    pub last_delta: Option<String>,
    pub unknown_packet_count: usize,
}

impl RemoteScreen {
    pub(crate) fn reset_session(&mut self) {
        *self = Self {
            display: self.display.blank_with_configuration(),
            show_diagnostics: self.show_diagnostics,
            ..Self::default()
        };
    }
}

#[derive(Default)]
pub(crate) struct RemoteInputState {
    pub ready: bool,
    pub pending: bool,
    pub held_key: Option<u8>,
}

#[derive(PartialEq, Clone, Debug, Copy)]
pub enum MainTab {
    Channels,
    Settings,
    Scanning,
    MemoryGroups,
    BandPlan,
    DTMF,
    Remote,
    Codeplug,
    BinFlash,
    Debug,
    Activity,
}

pub enum AppEvent {
    Progress(f64),
    Status(String),
    Log(String),
    BleScanComplete(Vec<PortCandidate>),
    BleScanFailed(String),
    ReadChannelsComplete(Vec<Channel>, Endianness),
    ReadGroupLabelsComplete(Vec<String>),
    ReadGroupLabelsOnlyComplete(Vec<String>),
    ReadPresetsComplete(Vec<ScanPreset>),
    ReadBandPlanComplete(Vec<BandPlan>),
    ReadDTMFComplete(Vec<DTMFPreset>),
    ReadSettingsComplete(SettingsBlock, Endianness),
    ReadAllFailed(String),
    RemotePhase(RemoteSessionPhase),
    RemoteControl(RemoteControlReport),
    RemoteDelta(String),
    RemoteReceived(std::time::Instant),
    RemotePacket(RemotePacket),
    SimulatorLcdFrame(Vec<u16>),
    SimulatorLcdUnavailable(String),
    RemoteStopped {
        message: String,
        failure: Option<RemoteSessionFailure>,
    },
    WriteComplete(WriteScope),
    LoadCSV(PathBuf),
    WriteCSV(PathBuf),
    ExportCSV(PathBuf),
    LoadCodeplug(PathBuf),
    ExportCodeplug(PathBuf),
    CodeplugLoaded(PathBuf, Vec<u8>),
    CodeplugDataLoaded {
        path: PathBuf,
        data: Vec<u8>,
        channels: Vec<Channel>,
        settings: Option<SettingsBlock>,
        scan_presets: Vec<ScanPreset>,
        group_labels: Vec<String>,
    },
    LoadBinFirmware(PathBuf),
    BinFirmwareLoaded(PathBuf, Vec<u8>),
    BinFlashComplete,
    BinFlashFailed(String),
    Error(String),
    ShowImportDialog,
    ShowExportDialog,
    ShowWriteDialog,
    ShowCodeplugImportDialog,
    ShowCodeplugExportDialog,
    ShowBinFirmwareDialog,
    SuspendUI,
    ResumeUI,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteScope {
    Channels,
    Dtmf,
    GroupLabels,
    Settings,
    ScanPresets,
    BandPlans,
    Codeplug,
    None,
}

pub(crate) struct ReadAllRequest {
    pub return_tab: MainTab,
    pub workspace: Vec<u8>,
}

pub struct App {
    pub serial_port_candidates: Vec<PortCandidate>,
    pub ble_port_candidates: Vec<PortCandidate>,
    pub mode: AppMode,
    pub port_candidates: Vec<PortCandidate>,
    pub ports: Vec<String>,
    pub selected_port_index: usize,
    pub channels: Vec<Channel>,
    pub deleted_channels: Vec<u16>,
    pub channel_state: TableState,
    pub group_labels: Vec<String>,
    pub scan_presets: Vec<ScanPreset>,
    pub preset_state: TableState,
    pub editing_scan_preset: Option<ScanPreset>,
    pub editing_group_label_idx: Option<usize>,
    pub scanning_group_state: TableState,
    pub band_plans: Vec<BandPlan>,
    pub bandplan_state: TableState,
    pub editing_band_plan: Option<BandPlan>,
    pub dtmf_presets: Vec<DTMFPreset>,
    pub dtmf_state: TableState,
    pub settings: Option<SettingsBlock>,
    pub settings_state: TableState,
    pub remote_screen: RemoteScreen,
    pub activity: Arc<Mutex<super::activity::ActivitySnapshot>>,
    pub activity_state: TableState,
    pub(crate) activity_worker: Option<super::activity::ActivityWorker>,
    pub protocol_port_name: Option<String>,
    pub active_port_candidate: Option<PortCandidate>,
    pub ble_scan_in_progress: bool,
    pub ble_scan_ui_suspended: bool,
    pub ble_reconnect_required: bool,
    pub progress: f64,
    pub status_message: String,
    pub logs: VecDeque<String>,
    pub help: crate::app::help::HelpState,
    pub log_view: crate::app::logs::LogView,
    pub endian: Endianness,
    pub edit_buffer: String,
    pub selection_index: usize,
    pub event_tx: Sender<AppEvent>,
    pub event_rx: Receiver<AppEvent>,
    pub remote_active: bool,
    pub remote_keyboard_release_supported: bool,
    pub remote_stop_signal: Arc<AtomicBool>,
    pub remote_tx: Option<Sender<RemoteControlCommand>>,
    pub(crate) remote_input_state: Arc<Mutex<RemoteInputState>>,
    pub last_main_tab: MainTab,
    pub last_non_remote_tab: MainTab,
    pub settings_dirty: bool,
    pub channels_dirty: bool,
    pub dtmf_dirty: bool,
    pub group_labels_dirty: bool,
    pub scan_presets_dirty: bool,
    pub band_plans_dirty: bool,
    pub codeplug_data: Option<Vec<u8>>,
    pub codeplug_workspace_baseline: Option<Vec<u8>>,
    pub(crate) read_all_request: Option<ReadAllRequest>,
    pub codeplug_path: Option<PathBuf>,
    pub bin_firmware_data: Option<Vec<u8>>,
    pub bin_file_path: Option<PathBuf>,
    pub dialog_open: bool,
    pub pending_channel_edit: Option<Channel>,
    pub dtmf_edit_preset_idx: Option<usize>,
    pub pending_dtmf_edit: Option<DTMFPreset>,
    pub demo_mode: bool,
    pub simulator_mode: bool,
    pub simulator_http_endpoint: Option<String>,
}
