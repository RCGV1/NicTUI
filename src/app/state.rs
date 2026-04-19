use ratatui::widgets::TableState;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};

use crate::device::PortCandidate;
use crate::protocol::{
    BandPlan, Channel, DTMFPreset, Endianness, RemotePacket, ScanPreset, SettingsBlock,
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
    pub elements: VecDeque<RemotePacket>,
    pub signal_strength: u8,
    pub noise_level: u8,
    pub leds: u8,
    pub battery_level: Option<u8>,
    pub last_signal_update: Option<std::time::Instant>,
    pub last_noise_update: Option<std::time::Instant>,
    pub last_battery_update: Option<std::time::Instant>,
    pub last_led_update: Option<std::time::Instant>,
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
}

pub enum AppEvent {
    Progress(f64),
    Status(String),
    Log(String),
    ReadChannelsComplete(Vec<Channel>, Endianness),
    ReadGroupLabelsComplete(Vec<String>),
    ReadPresetsComplete(Vec<ScanPreset>),
    ReadBandPlanComplete(Vec<BandPlan>),
    ReadDTMFComplete(Vec<DTMFPreset>),
    ReadSettingsComplete(SettingsBlock, Endianness),
    RemotePacket(RemotePacket),
    RemoteStopped(String),
    WriteComplete,
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

pub struct App {
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
    pub protocol_port_name: Option<String>,
    pub progress: f64,
    pub status_message: String,
    pub logs: VecDeque<String>,
    pub endian: Endianness,
    pub edit_buffer: String,
    pub selection_index: usize,
    pub event_tx: Sender<AppEvent>,
    pub event_rx: Receiver<AppEvent>,
    pub remote_active: bool,
    pub remote_stop_signal: Arc<AtomicBool>,
    pub remote_tx: Option<Sender<u8>>,
    pub last_main_tab: MainTab,
    pub last_non_remote_tab: MainTab,
    pub settings_dirty: bool,
    pub channels_dirty: bool,
    pub dtmf_dirty: bool,
    pub group_labels_dirty: bool,
    pub codeplug_data: Option<Vec<u8>>,
    pub codeplug_path: Option<PathBuf>,
    pub bin_firmware_data: Option<Vec<u8>>,
    pub bin_file_path: Option<PathBuf>,
    pub dialog_open: bool,
    pub pending_channel_edit: Option<Channel>,
    pub dtmf_edit_preset_idx: Option<usize>,
}
