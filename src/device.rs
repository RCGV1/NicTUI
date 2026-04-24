use std::cell::Cell;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serialport::SerialPortType;

use crate::protocol::codeplug::{
    extract_band_plans_from_codeplug, extract_channels_from_codeplug,
    extract_dtmf_presets_from_codeplug, extract_group_labels_from_codeplug,
    extract_scan_presets_from_codeplug, extract_settings_from_codeplug,
    extract_vfo_memories_from_codeplug, infer_channel_endianness_from_codeplug,
    infer_endianness_from_codeplug,
};
use crate::protocol::radio::SETTINGS_MAGIC;
use crate::protocol::{
    BAND_PLAN_RECORD_COUNT, BAND_PLAN_RECORD_SIZE, BIN_FLASH_BAUD_RATE, BLOCK_SIZE, BandPlan,
    Channel, DTMFPreset, EEPROM_SIZE, Endianness, GROUP_LABEL_COUNT, GROUP_LABEL_SIZE,
    GROUP_LABELS_BLOCK_COUNT, GROUP_LABELS_BLOCK_OFFSET, GROUP_LABELS_BLOCK_START,
    GROUP_LABELS_OFFSET, RadioProtocol, RemotePacket, SCAN_PRESET_RECORD_COUNT,
    SCAN_PRESET_RECORD_SIZE, ScanPreset, SettingsBlock, TOTAL_BLOCKS,
};
use crate::remote::{
    RemoteCaptureEvent, RemoteCaptureSummary, RemoteControlCommand, RemoteControlReport,
    RemoteSessionFailure, RemoteSessionOptions, RemoteSessionPhase, run_remote_session,
};

const CHANNEL_START_BLOCK: usize = 2;
const CHANNEL_RECORD_COUNT: usize = 198;
const CHANNEL_DATA_OFFSET: usize = CHANNEL_START_BLOCK * BLOCK_SIZE;
const CHANNEL_DATA_SIZE: usize = CHANNEL_RECORD_COUNT * BLOCK_SIZE;
const SETTINGS_BLOCK_START: usize = 0x1900 / BLOCK_SIZE;
const SETTINGS_BLOCK_COUNT: usize = 4;
const SETTINGS_OFFSET: usize = 0x1900;
const SETTINGS_SIZE: usize = SETTINGS_BLOCK_COUNT * BLOCK_SIZE;
const SCAN_PRESET_START: usize = 0x1B00 / BLOCK_SIZE;
const SCAN_PRESET_BLOCK_COUNT: usize =
    (SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE) / BLOCK_SIZE;
const SCAN_PRESET_OFFSET: usize = 0x1B00;
const SCAN_PRESET_SIZE: usize = SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE;
const BAND_PLAN_START: usize = 0x1A00 / BLOCK_SIZE;
const BAND_PLAN_BLOCK_COUNT: usize = 7;
const BAND_PLAN_OFFSET: usize = 0x1A02;
const BAND_PLAN_SIZE: usize = BAND_PLAN_RECORD_COUNT * BAND_PLAN_RECORD_SIZE;
const DTMF_START: usize = 0x1CF0 / BLOCK_SIZE;
const DTMF_BLOCK_COUNT: usize = 9;
const DTMF_PRESET_COUNT: usize = 20;
const DTMF_OFFSET: usize = 0x1CF0;
const DTMF_SIZE: usize = DTMF_PRESET_COUNT * 13;
const NICSURE_REQUIRED_HINT: &str = "NicTUI requires NicSure mod firmware for live radio reads and writes. Install the NicSure firmware and try again.";

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Status(String),
    Progress(f64),
}

#[derive(Debug, Clone)]
pub enum RemoteMonitorEvent {
    Status(String),
    Log(String),
    Phase(RemoteSessionPhase),
    Control(RemoteControlReport),
    Packet(RemotePacket),
    Delta(String),
}

#[derive(Debug, Clone)]
pub struct RemoteMonitorOptions {
    pub duration: Duration,
    pub include_raw_logs: bool,
    pub suppress_idle_zero_logs: bool,
    pub scripted_commands: Vec<RemoteControlCommand>,
    pub command_start_delay: Duration,
    pub key_interval: Duration,
    pub disable_radio_before_remote: bool,
    pub recover_retries: usize,
}

impl Default for RemoteMonitorOptions {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(8),
            include_raw_logs: false,
            suppress_idle_zero_logs: true,
            scripted_commands: Vec::new(),
            command_start_delay: Duration::from_millis(250),
            key_interval: Duration::from_millis(350),
            disable_radio_before_remote: false,
            recover_retries: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteKeySendReport {
    pub control: RemoteControlReport,
}

#[derive(Debug, Clone)]
struct RemoteCommandSchedule {
    next_command_at: std::cell::Cell<Duration>,
}

impl RemoteCommandSchedule {
    fn new(start_delay: Duration) -> Self {
        Self {
            next_command_at: std::cell::Cell::new(start_delay),
        }
    }

    fn reset(&self, start_delay: Duration) {
        self.next_command_at.set(start_delay);
    }

    fn command_due(&self, elapsed: Duration) -> bool {
        elapsed >= self.next_command_at.get()
    }

    fn mark_sent(&self, elapsed: Duration, key_interval: Duration) {
        self.next_command_at.set(elapsed + key_interval);
    }
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub port: String,
    pub handshake_ok: bool,
    pub endian: Option<Endianness>,
    pub channel_endian: Option<Endianness>,
    pub firmware_variant: Option<FirmwareVariant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveModeCapability {
    Unverified,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    Radio,
    Ble,
    Candidate,
    System,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct PortCandidate {
    pub port_name: String,
    pub kind: PortKind,
    pub score: i32,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub usb_vid: Option<u16>,
    pub usb_pid: Option<u16>,
    pub ble_device_id: Option<String>,
    pub ble_rssi: Option<i32>,
    pub handshake_ok: bool,
    pub firmware_variant: Option<FirmwareVariant>,
}

impl PortCandidate {
    pub fn badge(&self) -> &'static str {
        match self.kind {
            PortKind::Radio => "radio",
            PortKind::Ble => "ble",
            PortKind::Candidate => "likely",
            PortKind::System => "system",
            PortKind::Unknown => "other",
        }
    }

    pub fn is_radio(&self) -> bool {
        matches!(self.kind, PortKind::Radio)
    }

    pub fn is_ble(&self) -> bool {
        matches!(self.kind, PortKind::Ble)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareVariant {
    NicSure,
    Stock,
}

impl fmt::Display for FirmwareVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FirmwareVariant::NicSure => write!(f, "NicSure"),
            FirmwareVariant::Stock => write!(f, "stock/original"),
        }
    }
}

impl fmt::Display for LiveModeCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LiveModeCapability::Unverified => write!(f, "unverified"),
            LiveModeCapability::Unsupported => write!(f, "unsupported"),
        }
    }
}

pub fn live_mode_capability(probe: &ProbeResult) -> LiveModeCapability {
    if !probe.handshake_ok || matches!(probe.firmware_variant, Some(FirmwareVariant::Stock)) {
        LiveModeCapability::Unsupported
    } else {
        LiveModeCapability::Unverified
    }
}

pub fn live_mode_hint(probe: &ProbeResult) -> Option<String> {
    match live_mode_capability(probe) {
        LiveModeCapability::Unsupported => Some(
            "Install NicSure mod firmware before using NicTUI live read/write commands."
                .to_string(),
        ),
        LiveModeCapability::Unverified => Some(
            "Run `nictui remote pvojh-sweep --gap-ms 0,50,100 --json` before relying on live-mode block access. Some NicSure builds route the public PVOJH opener into remote-mode parsing (`4A`)."
                .to_string(),
        ),
    }
}

pub fn list_ports() -> Result<Vec<String>> {
    let mut ports: Vec<String> = list_port_candidates()?
        .into_iter()
        .map(|port| port.port_name)
        .collect();
    ports.sort_by_key(|port| port_sort_key(port));
    ports.dedup();
    Ok(ports)
}

pub fn list_port_candidates() -> Result<Vec<PortCandidate>> {
    let ports = serialport::available_ports().context("Failed to enumerate serial ports")?;
    let mut candidates = dedupe_port_candidates(
        ports
            .into_iter()
            .map(port_candidate_from_info)
            .collect::<Vec<_>>(),
    );

    let mut probe_order: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| should_probe_candidate(candidate))
        .map(|(index, _)| index)
        .collect();
    probe_order.sort_by(|&left, &right| {
        candidates[right]
            .score
            .cmp(&candidates[left].score)
            .then_with(|| candidates[left].port_name.cmp(&candidates[right].port_name))
    });
    probe_order.truncate(3);

    for index in probe_order {
        if let Ok(probe) = probe_port(&candidates[index].port_name) {
            candidates[index].handshake_ok = probe.handshake_ok;
            candidates[index].firmware_variant = probe.firmware_variant;
            if probe.handshake_ok {
                candidates[index].kind = PortKind::Radio;
                candidates[index].score += 1_000;
            }
        }
    }

    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| port_sort_key(&left.port_name).cmp(&port_sort_key(&right.port_name)))
    });
    Ok(candidates)
}

pub fn resolve_port(requested: Option<&str>) -> Result<String> {
    if let Some(port) = requested {
        return Ok(port.to_string());
    }

    let candidates = list_port_candidates()?;
    resolve_port_from_candidates(&candidates)
}

pub fn probe_port(port_name: &str) -> Result<ProbeResult> {
    let mut proto = open_protocol(port_name)?;
    let handshake_ok = proto.handshake().context("Handshake attempt failed")?;
    let (endian, channel_endian, firmware_variant) = if handshake_ok {
        let endian = proto.detect_endianness().ok();
        let channel_endian = proto.detect_channel_endianness().ok().or(endian);
        let firmware_variant = detect_firmware_variant(&mut proto).ok();
        (endian, channel_endian, firmware_variant)
    } else {
        (None, None, None)
    };

    Ok(ProbeResult {
        port: port_name.to_string(),
        handshake_ok,
        endian,
        channel_endian,
        firmware_variant,
    })
}

pub fn read_codeplug<F>(port_name: &str, mut report: F) -> Result<(Vec<u8>, Endianness)>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading full radio memory",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let mut eeprom = vec![0u8; EEPROM_SIZE];

    for block in 0..TOTAL_BLOCKS {
        let data = proto
            .read_block(block as u8)
            .with_context(|| format!("Failed to read block {}", block))?;
        let start = block * BLOCK_SIZE;
        eeprom[start..start + BLOCK_SIZE].copy_from_slice(&data);
        report(ProgressEvent::Progress(
            (block + 1) as f64 / TOTAL_BLOCKS as f64,
        ));
    }

    let endian = infer_endianness_from_codeplug(&eeprom);
    report(ProgressEvent::Status(format!(
        "Read {} bytes from radio",
        eeprom.len()
    )));

    Ok((eeprom, endian))
}

pub fn write_codeplug<F>(
    port_name: &str,
    codeplug_data: &[u8],
    reboot: bool,
    mut report: F,
) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    if codeplug_data.len() != EEPROM_SIZE {
        bail!(
            "Invalid codeplug size: expected {} bytes, got {} bytes",
            EEPROM_SIZE,
            codeplug_data.len()
        );
    }

    report(ProgressEvent::Status(format!(
        "Opening {} and writing full radio memory",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    for block in 0..TOTAL_BLOCKS {
        let offset = block * BLOCK_SIZE;
        let acknowledged = proto
            .write_block(block as u8, &codeplug_data[offset..offset + BLOCK_SIZE])
            .with_context(|| format!("Failed to write block {}", block))?;
        if !acknowledged {
            bail!("Radio rejected block {}", block);
        }
        report(ProgressEvent::Progress(
            (block + 1) as f64 / TOTAL_BLOCKS as f64,
        ));
    }

    if reboot {
        report(ProgressEvent::Status("Rebooting radio".to_string()));
        proto.reboot().context("Failed to reboot radio")?;
    }

    report(ProgressEvent::Status("Codeplug write complete".to_string()));
    Ok(())
}

pub fn read_channels<F>(port_name: &str, mut report: F) -> Result<(Vec<Channel>, Endianness)>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading channels",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let mut eeprom = vec![0u8; (CHANNEL_START_BLOCK + CHANNEL_RECORD_COUNT) * BLOCK_SIZE];

    for offset in 0..CHANNEL_RECORD_COUNT {
        let block = CHANNEL_START_BLOCK + offset;
        let data = proto
            .read_block(block as u8)
            .with_context(|| format!("Failed to read block {}", block))?;
        let start = block * BLOCK_SIZE;
        eeprom[start..start + BLOCK_SIZE].copy_from_slice(&data);
        report(ProgressEvent::Progress(
            (offset + 1) as f64 / CHANNEL_RECORD_COUNT as f64,
        ));
    }

    let channel_endian = infer_channel_endianness_from_codeplug(&eeprom);
    let channels = extract_channels_from_codeplug(&eeprom, channel_endian);

    report(ProgressEvent::Status(format!(
        "Loaded {} channels",
        channels.len()
    )));

    Ok((channels, channel_endian))
}

pub fn write_channels<F>(
    port_name: &str,
    channels: &[Channel],
    deleted_channels: &[u16],
    default_endian: Endianness,
    reboot: bool,
    mut report: F,
) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_channels(channels)?;

    let total_operations = channels.len() + deleted_channels.len();
    if total_operations == 0 {
        report(ProgressEvent::Status(
            "No channel changes to write".to_string(),
        ));
        return Ok(());
    }

    report(ProgressEvent::Status(format!(
        "Opening {} and writing channel data",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let channel_endian = proto.detect_channel_endianness().unwrap_or(default_endian);
    let mut completed = 0usize;

    for channel in channels {
        let block = (channel.channel_num + 1) as u8;
        let data = RadioProtocol::pack_channel(channel, channel_endian);
        let acknowledged = proto
            .write_block(block, &data)
            .with_context(|| format!("Failed to write channel {}", channel.channel_num))?;
        if !acknowledged {
            bail!("Radio rejected channel {}", channel.channel_num);
        }
        completed += 1;
        report(ProgressEvent::Progress(
            completed as f64 / total_operations as f64,
        ));
    }

    for channel_num in deleted_channels {
        let block = (channel_num + 1) as u8;
        let empty_channel = build_cleared_channel(*channel_num);
        let empty_data = RadioProtocol::pack_channel(&empty_channel, channel_endian);
        let acknowledged = proto
            .write_block(block, &empty_data)
            .with_context(|| format!("Failed to clear channel {}", channel_num))?;
        if !acknowledged {
            bail!("Radio rejected clear for channel {}", channel_num);
        }
        completed += 1;
        report(ProgressEvent::Progress(
            completed as f64 / total_operations as f64,
        ));
    }

    if reboot {
        report(ProgressEvent::Status("Rebooting radio".to_string()));
        proto.reboot().context("Failed to reboot radio")?;
    }

    report(ProgressEvent::Status("Channel write complete".to_string()));
    Ok(())
}

fn build_cleared_channel(channel_num: u16) -> Channel {
    Channel {
        channel_num,
        name: String::new(),
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
    }
}

pub fn read_settings<F>(port_name: &str, mut report: F) -> Result<(SettingsBlock, Endianness)>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading settings",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let data = read_block_range(
        &mut proto,
        SETTINGS_BLOCK_START,
        SETTINGS_BLOCK_COUNT,
        &mut report,
    )?;
    let endian = RadioProtocol::infer_settings_endianness(&data);
    let settings = RadioProtocol::parse_settings_block(&data, endian);

    report(ProgressEvent::Status("Settings read complete".to_string()));
    Ok((settings, endian))
}

pub fn write_settings<F>(
    port_name: &str,
    settings: &SettingsBlock,
    _endian: Endianness,
    reboot: bool,
    mut report: F,
) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and writing settings",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let current = read_block_range(
        &mut proto,
        SETTINGS_BLOCK_START,
        SETTINGS_BLOCK_COUNT,
        &mut report,
    )?;
    let endian = RadioProtocol::infer_settings_endianness(&current);
    let packed = RadioProtocol::pack_settings_block(settings, endian);
    write_block_range(
        &mut proto,
        SETTINGS_BLOCK_START,
        &packed,
        &mut report,
        "settings",
    )?;

    if reboot {
        report(ProgressEvent::Status("Rebooting radio".to_string()));
        proto.reboot().context("Failed to reboot radio")?;
    }

    report(ProgressEvent::Status("Settings write complete".to_string()));
    Ok(())
}

pub fn read_scan_presets<F>(port_name: &str, mut report: F) -> Result<(Vec<ScanPreset>, Endianness)>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading scan presets",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let data = read_block_range(
        &mut proto,
        SCAN_PRESET_START,
        SCAN_PRESET_BLOCK_COUNT,
        &mut report,
    )?;
    let endian = RadioProtocol::infer_scan_preset_endianness(&data);

    let mut presets = Vec::new();
    for index in 0..SCAN_PRESET_RECORD_COUNT {
        let start = index * SCAN_PRESET_RECORD_SIZE;
        let end = start + SCAN_PRESET_RECORD_SIZE;
        if end <= data.len() {
            presets.push(RadioProtocol::parse_scan_preset(
                &data[start..end],
                index as u8,
                endian,
            ));
        }
    }

    report(ProgressEvent::Status(format!(
        "Read {} scan presets",
        presets.len()
    )));
    Ok((presets, endian))
}

pub fn write_scan_presets<F>(
    port_name: &str,
    presets: &[ScanPreset],
    _endian: Endianness,
    mut report: F,
) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_scan_presets(presets)?;

    report(ProgressEvent::Status(format!(
        "Opening {} and writing scan presets",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let existing = read_block_range(
        &mut proto,
        SCAN_PRESET_START,
        SCAN_PRESET_BLOCK_COUNT,
        &mut report,
    )?;
    let endian = RadioProtocol::infer_scan_preset_endianness(&existing);
    let mut packed = vec![0xFFu8; SCAN_PRESET_BLOCK_COUNT * BLOCK_SIZE];

    for preset in presets {
        let offset = preset.index as usize * SCAN_PRESET_RECORD_SIZE;
        let bytes = RadioProtocol::pack_scan_preset(preset, endian);
        packed[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    write_block_range(
        &mut proto,
        SCAN_PRESET_START,
        &packed,
        &mut report,
        "scan presets",
    )?;

    report(ProgressEvent::Status(
        "Scan preset write complete".to_string(),
    ));
    Ok(())
}

pub fn update_scan_preset<F>(port_name: &str, preset: &ScanPreset, mut report: F) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_scan_presets(std::slice::from_ref(preset))?;

    report(ProgressEvent::Status(format!(
        "Opening {} and updating scan preset {}",
        port_name, preset.index
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let mut packed = read_block_range(
        &mut proto,
        SCAN_PRESET_START,
        SCAN_PRESET_BLOCK_COUNT,
        &mut report,
    )?;
    let endian = RadioProtocol::infer_scan_preset_endianness(&packed);
    let offset = preset.index as usize * SCAN_PRESET_RECORD_SIZE;
    let bytes = RadioProtocol::pack_scan_preset(preset, endian);
    packed[offset..offset + bytes.len()].copy_from_slice(&bytes);

    write_block_range(
        &mut proto,
        SCAN_PRESET_START,
        &packed,
        &mut report,
        "scan presets",
    )?;

    report(ProgressEvent::Status(format!(
        "Scan preset {} write complete",
        preset.index
    )));
    Ok(())
}

pub fn read_band_plans<F>(port_name: &str, mut report: F) -> Result<(Vec<BandPlan>, Endianness)>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading band plans",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let data = read_block_range(
        &mut proto,
        BAND_PLAN_START,
        BAND_PLAN_BLOCK_COUNT,
        &mut report,
    )?;
    let endian = RadioProtocol::infer_bandplan_endianness(&data[2..]);

    let mut band_plans = Vec::new();
    for index in 0..BAND_PLAN_RECORD_COUNT {
        let start = 2 + (index * BAND_PLAN_RECORD_SIZE);
        let end = start + BAND_PLAN_RECORD_SIZE;
        if end <= data.len() {
            band_plans.push(RadioProtocol::parse_bandplan(
                &data[start..end],
                index as u8,
                endian,
            ));
        }
    }

    report(ProgressEvent::Status(format!(
        "Read {} band plans",
        band_plans.len()
    )));
    Ok((band_plans, endian))
}

pub fn write_band_plans<F>(
    port_name: &str,
    band_plans: &[BandPlan],
    _endian: Endianness,
    mut report: F,
) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_band_plans_payload(band_plans)?;

    report(ProgressEvent::Status(format!(
        "Opening {} and writing band plans",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let existing = read_block_range(
        &mut proto,
        BAND_PLAN_START,
        BAND_PLAN_BLOCK_COUNT,
        &mut report,
    )?;
    let endian = RadioProtocol::infer_bandplan_endianness(&existing[2..]);
    let mut packed = existing;

    for plan in band_plans {
        let offset = 2 + (plan.index as usize * BAND_PLAN_RECORD_SIZE);
        let bytes = RadioProtocol::pack_bandplan(plan, endian);
        packed[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    write_block_range(
        &mut proto,
        BAND_PLAN_START,
        &packed,
        &mut report,
        "band plans",
    )?;

    report(ProgressEvent::Status(
        "Band plan write complete".to_string(),
    ));
    Ok(())
}

pub fn read_dtmf_presets<F>(port_name: &str, mut report: F) -> Result<Vec<DTMFPreset>>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading DTMF presets",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let data = read_block_range(&mut proto, DTMF_START, DTMF_BLOCK_COUNT, &mut report)?;

    let mut presets = Vec::new();
    for index in 0..DTMF_PRESET_COUNT {
        let start = index * 13;
        let end = start + 13;
        if end <= data.len() {
            presets.push(RadioProtocol::parse_dtmf_preset(
                &data[start..end],
                index as u8,
            ));
        }
    }

    report(ProgressEvent::Status(format!(
        "Read {} DTMF presets",
        presets.len()
    )));
    Ok(presets)
}

pub fn read_group_labels<F>(port_name: &str, mut report: F) -> Result<Vec<String>>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading group names",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let data = read_block_range(
        &mut proto,
        GROUP_LABELS_BLOCK_START,
        GROUP_LABELS_BLOCK_COUNT,
        &mut report,
    )?;
    let start = GROUP_LABELS_BLOCK_OFFSET;
    let end = start + GROUP_LABEL_COUNT * GROUP_LABEL_SIZE;
    let labels = RadioProtocol::parse_group_labels(&data[start..end]);

    report(ProgressEvent::Status(format!(
        "Loaded {} group names",
        labels
            .iter()
            .filter(|label| !label.trim().is_empty())
            .count()
    )));
    Ok(labels)
}

pub fn write_group_labels<F>(port_name: &str, labels: &[String], mut report: F) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and saving group names",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let mut packed = read_block_range(
        &mut proto,
        GROUP_LABELS_BLOCK_START,
        GROUP_LABELS_BLOCK_COUNT,
        &mut report,
    )?;
    let bytes = RadioProtocol::pack_group_labels(labels);
    let start = GROUP_LABELS_BLOCK_OFFSET;
    let end = start + bytes.len();
    packed[start..end].copy_from_slice(&bytes);

    write_block_range(
        &mut proto,
        GROUP_LABELS_BLOCK_START,
        &packed,
        &mut report,
        "group names",
    )?;
    report(ProgressEvent::Status("Group names saved".to_string()));
    Ok(())
}

pub fn write_dtmf_presets<F>(port_name: &str, presets: &[DTMFPreset], mut report: F) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_dtmf_presets_payload(presets)?;

    report(ProgressEvent::Status(format!(
        "Opening {} and writing DTMF presets",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let mut packed = vec![0xFFu8; DTMF_BLOCK_COUNT * BLOCK_SIZE];

    for preset in presets {
        let offset = preset.index as usize * 13;
        let bytes = RadioProtocol::pack_dtmf_preset(preset);
        packed[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    write_block_range(&mut proto, DTMF_START, &packed, &mut report, "DTMF presets")?;
    report(ProgressEvent::Status("DTMF write complete".to_string()));
    Ok(())
}

pub fn update_dtmf_preset<F>(port_name: &str, preset: &DTMFPreset, mut report: F) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_dtmf_presets_payload(std::slice::from_ref(preset))?;

    report(ProgressEvent::Status(format!(
        "Opening {} and updating DTMF preset {}",
        port_name, preset.index
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let mut packed = read_block_range(&mut proto, DTMF_START, DTMF_BLOCK_COUNT, &mut report)?;
    let offset = preset.index as usize * 13;
    let bytes = RadioProtocol::pack_dtmf_preset(preset);
    packed[offset..offset + bytes.len()].copy_from_slice(&bytes);

    write_block_range(&mut proto, DTMF_START, &packed, &mut report, "DTMF presets")?;
    report(ProgressEvent::Status(format!(
        "DTMF preset {} write complete",
        preset.index
    )));
    Ok(())
}

pub fn flash_firmware<F>(port_name: &str, firmware_data: &[u8], mut report: F) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    const INIT_SEQUENCE: [u8; 36] = [
        0xA0, 0xEE, 0x74, 0x71, 0x07, 0x74, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    ];

    let rounded_len = firmware_data.len().div_ceil(32) * 32;
    if rounded_len > 0xF800 {
        bail!("Firmware file too large");
    }

    let last_block = (rounded_len / 32) as u16;
    report(ProgressEvent::Status(format!(
        "Opening {} at {} baud for firmware flashing",
        port_name, BIN_FLASH_BAUD_RATE
    )));

    let mut proto = RadioProtocol::new_with_baud(port_name, BIN_FLASH_BAUD_RATE)
        .map_err(|error| explain_open_error(port_name, error))?;

    report(ProgressEvent::Status(
        "Turn the radio off, hold the required key, and power it on in flash mode".to_string(),
    ));

    let start_time = Instant::now();
    let timeout = Duration::from_secs(60);
    let mut detected = false;
    let mut flashing = false;
    let mut block: u16 = 0;
    let mut need_to_send_block = true;
    let mut last_block_sent = false;
    let mut a5_count = 0usize;
    let mut block_sent_at = Instant::now();

    loop {
        if start_time.elapsed() > timeout {
            bail!("Timeout waiting for radio bootloader");
        }

        if flashing && need_to_send_block && block <= last_block {
            let is_last = block == last_block;
            let mut packet = [0u8; 36];
            packet[0] = if is_last { 0xA2 } else { 0xA1 };
            packet[1] = ((block >> 8) & 0xFF) as u8;
            packet[2] = (block & 0xFF) as u8;

            let start = block as usize * 32;
            let end = (start + 32).min(firmware_data.len());
            packet[4..4 + (end - start)].copy_from_slice(&firmware_data[start..end]);
            packet[3] = packet[4..36]
                .iter()
                .fold(0u8, |accumulator, value| accumulator.wrapping_add(*value));

            proto
                .send_bytes(&packet)
                .context("Failed to send firmware block")?;
            block_sent_at = Instant::now();
            need_to_send_block = false;

            if is_last {
                last_block_sent = true;
            } else {
                block += 1;
            }

            report(ProgressEvent::Progress(
                (block as f64 + if last_block_sent { 1.0 } else { 0.0 })
                    / (last_block as f64 + 1.0),
            ));
        }

        if last_block_sent
            && !need_to_send_block
            && block_sent_at.elapsed() > Duration::from_millis(500)
        {
            report(ProgressEvent::Progress(1.0));
            report(ProgressEvent::Status(
                "Firmware flashing completed. Power-cycle the radio to boot the new image."
                    .to_string(),
            ));
            return Ok(());
        }

        if flashing
            && !need_to_send_block
            && !last_block_sent
            && block_sent_at.elapsed() > Duration::from_millis(500)
        {
            need_to_send_block = true;
        }

        match proto
            .read_byte()
            .context("Failed while waiting for bootloader")?
        {
            Some(0xA5) => {
                a5_count += 1;
                if !detected && a5_count >= 3 {
                    detected = true;
                    proto
                        .send_bytes(&INIT_SEQUENCE)
                        .context("Failed to send bootloader init sequence")?;
                    report(ProgressEvent::Status(
                        "Bootloader handshake detected. Starting transfer.".to_string(),
                    ));
                } else if detected {
                    flashing = true;
                }
            }
            Some(_) => {}
            None => {}
        }
    }
}

pub fn send_remote_key(port_name: &str, key_code: u8) -> Result<()> {
    send_remote_key_with_report(port_name, key_code)
        .map(|_| ())
        .map_err(anyhow::Error::from)
}

pub fn send_remote_key_with_report(
    port_name: &str,
    key_code: u8,
) -> std::result::Result<RemoteKeySendReport, RemoteSessionFailure> {
    let command = RemoteControlCommand::raw_key(remote_key_name(key_code), key_code);
    let options = RemoteMonitorOptions {
        duration: Duration::from_millis(650) + command.estimated_duration(),
        scripted_commands: vec![command],
        command_start_delay: Duration::from_millis(120),
        recover_retries: 1,
        suppress_idle_zero_logs: false,
        ..RemoteMonitorOptions::default()
    };
    let mut control = None;

    monitor_remote_with_summary(port_name, &options, |event| {
        if let RemoteMonitorEvent::Control(report) = event {
            control = Some(report);
        }
    })?;

    let control = control.ok_or_else(|| RemoteSessionFailure {
        kind: crate::remote::RemoteSessionFailureKind::StreamLost,
        summary: "Remote key session finished without producing a command report.".to_string(),
        detail: "The remote session ended before the command outcome could be reported."
            .to_string(),
    })?;
    Ok(RemoteKeySendReport { control })
}

pub fn monitor_remote<F>(
    port_name: &str,
    options: &RemoteMonitorOptions,
    on_event: F,
) -> Result<usize>
where
    F: FnMut(RemoteMonitorEvent),
{
    monitor_remote_with_summary(port_name, options, on_event)
        .map(|summary| summary.packet_count)
        .map_err(anyhow::Error::from)
}

pub fn monitor_remote_with_summary<F>(
    port_name: &str,
    options: &RemoteMonitorOptions,
    mut on_event: F,
) -> std::result::Result<RemoteCaptureSummary, RemoteSessionFailure>
where
    F: FnMut(RemoteMonitorEvent),
{
    let schedule = RemoteCommandSchedule::new(options.command_start_delay);
    let scripted_command_index = Rc::new(Cell::new(0usize));
    let command_in_flight = Rc::new(Cell::new(false));
    let command_index_for_next = Rc::clone(&scripted_command_index);
    let command_in_flight_for_next = Rc::clone(&command_in_flight);
    let command_index_for_event = Rc::clone(&scripted_command_index);
    let command_in_flight_for_event = Rc::clone(&command_in_flight);

    let summary = run_remote_session(
        port_name,
        &RemoteSessionOptions {
            include_raw_logs: options.include_raw_logs,
            suppress_idle_zero_logs: options.suppress_idle_zero_logs,
            disable_radio_before_remote: options.disable_radio_before_remote,
            recover_retries: options.recover_retries,
            suppress_repeated_idle: true,
            ..RemoteSessionOptions::default()
        },
        |elapsed| elapsed >= options.duration,
        |elapsed| {
            let index = command_index_for_next.get();
            if !command_in_flight_for_next.get()
                && index < options.scripted_commands.len()
                && schedule.command_due(elapsed)
            {
                let command = options.scripted_commands[index].clone();
                command_in_flight_for_next.set(true);
                schedule.mark_sent(elapsed, options.key_interval);
                Some(command)
            } else {
                None
            }
        },
        |event| {
            if let RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed) = &event {
                schedule.reset(options.command_start_delay);
            }

            match event {
                RemoteCaptureEvent::Status(message) => {
                    on_event(RemoteMonitorEvent::Status(message));
                }
                RemoteCaptureEvent::Log(message) => on_event(RemoteMonitorEvent::Log(message)),
                RemoteCaptureEvent::Phase(phase) => on_event(RemoteMonitorEvent::Phase(phase)),
                RemoteCaptureEvent::Control(report) => {
                    command_in_flight_for_event.set(false);
                    if report.success {
                        command_index_for_event.set(command_index_for_event.get() + 1);
                    }
                    on_event(RemoteMonitorEvent::Control(report));
                }
                RemoteCaptureEvent::Packet(packet) => on_event(RemoteMonitorEvent::Packet(packet)),
                RemoteCaptureEvent::Delta(delta) => on_event(RemoteMonitorEvent::Delta(delta)),
            }
        },
    )?;
    Ok(summary)
}

fn remote_key_name(key_code: u8) -> &'static str {
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
        0x11 => "v/m",
        0x12 => "flashlight",
        0x13 => "ptt-a",
        0x1A => "ptt-b",
        _ => "unknown",
    }
}

pub fn inspect_codeplug(data: &[u8]) -> Result<CodeplugInspection> {
    if data.len() != EEPROM_SIZE {
        bail!(
            "Invalid codeplug size: expected {} bytes, got {} bytes",
            EEPROM_SIZE,
            data.len()
        );
    }

    let endian = infer_endianness_from_codeplug(data);
    let channel_endian = infer_channel_endianness_from_codeplug(data);
    let vfo_memories = extract_vfo_memories_from_codeplug(data, channel_endian)
        .into_iter()
        .map(|(slot, channel)| VfoMemory { slot, channel })
        .collect::<Vec<_>>();
    let channels = extract_channels_from_codeplug(data, channel_endian);
    let settings = extract_settings_from_codeplug(data, endian);
    let scan_presets = extract_scan_presets_from_codeplug(data, endian);
    let band_plans = extract_band_plans_from_codeplug(data, endian);
    let dtmf_presets = extract_dtmf_presets_from_codeplug(data);
    let group_labels = extract_group_labels_from_codeplug(data);
    let regions = build_codeplug_regions(data);
    let unknown_region_count = regions
        .iter()
        .filter(|region| matches!(region.kind, CodeplugRegionKind::Unknown))
        .count();
    let unknown_regions_with_live_data = regions
        .iter()
        .filter(|region| {
            matches!(region.kind, CodeplugRegionKind::Unknown) && region.non_ff_bytes > 0
        })
        .count();

    Ok(CodeplugInspection {
        size: data.len(),
        endian,
        channel_endian,
        vfo_memory_count: vfo_memories.len(),
        channel_count: channels.len(),
        settings_present: settings.is_some(),
        scan_preset_count: scan_presets.len(),
        band_plan_count: band_plans.len(),
        dtmf_preset_count: dtmf_presets.len(),
        unknown_region_count,
        unknown_regions_with_live_data,
        vfo_memories,
        channels,
        settings,
        scan_presets,
        band_plans,
        dtmf_presets,
        group_labels,
        regions,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CodeplugInspection {
    pub size: usize,
    pub endian: Endianness,
    pub channel_endian: Endianness,
    pub vfo_memory_count: usize,
    pub channel_count: usize,
    pub settings_present: bool,
    pub scan_preset_count: usize,
    pub band_plan_count: usize,
    pub dtmf_preset_count: usize,
    pub unknown_region_count: usize,
    pub unknown_regions_with_live_data: usize,
    pub vfo_memories: Vec<VfoMemory>,
    pub channels: Vec<Channel>,
    pub settings: Option<SettingsBlock>,
    pub scan_presets: Vec<ScanPreset>,
    pub band_plans: Vec<BandPlan>,
    pub dtmf_presets: Vec<DTMFPreset>,
    pub group_labels: Vec<String>,
    pub regions: Vec<CodeplugRegion>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VfoMemory {
    pub slot: String,
    pub channel: Channel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeplugRegionKind {
    Known,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CodeplugRegion {
    pub name: String,
    pub kind: CodeplugRegionKind,
    pub start_offset: usize,
    pub end_offset_exclusive: usize,
    pub length: usize,
    pub non_ff_bytes: usize,
    pub non_zero_bytes: usize,
    pub first_non_ff_offset: Option<usize>,
    pub preview_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegionSpec {
    name: &'static str,
    start_offset: usize,
    end_offset_exclusive: usize,
}

fn build_codeplug_regions(data: &[u8]) -> Vec<CodeplugRegion> {
    let mut regions = Vec::new();
    let mut known_ranges = known_codeplug_regions();
    known_ranges.sort_by_key(|region| region.start_offset);

    let mut cursor = 0usize;
    for region in known_ranges {
        if cursor < region.start_offset {
            regions.push(summarize_region(
                data,
                "Unknown",
                CodeplugRegionKind::Unknown,
                cursor,
                region.start_offset,
            ));
        }

        regions.push(summarize_region(
            data,
            region.name,
            CodeplugRegionKind::Known,
            region.start_offset,
            region.end_offset_exclusive,
        ));
        cursor = region.end_offset_exclusive;
    }

    if cursor < data.len() {
        regions.push(summarize_region(
            data,
            "Unknown",
            CodeplugRegionKind::Unknown,
            cursor,
            data.len(),
        ));
    }

    regions
}

fn known_codeplug_regions() -> Vec<RegionSpec> {
    vec![
        RegionSpec {
            name: "VFO Memories",
            start_offset: 0,
            end_offset_exclusive: CHANNEL_START_BLOCK * BLOCK_SIZE,
        },
        RegionSpec {
            name: "Channels",
            start_offset: CHANNEL_DATA_OFFSET,
            end_offset_exclusive: CHANNEL_DATA_OFFSET + CHANNEL_DATA_SIZE,
        },
        RegionSpec {
            name: "Settings",
            start_offset: SETTINGS_OFFSET,
            end_offset_exclusive: SETTINGS_OFFSET + SETTINGS_SIZE,
        },
        RegionSpec {
            name: "Band Plans",
            start_offset: BAND_PLAN_OFFSET,
            end_offset_exclusive: BAND_PLAN_OFFSET + BAND_PLAN_SIZE,
        },
        RegionSpec {
            name: "Scan Presets",
            start_offset: SCAN_PRESET_OFFSET,
            end_offset_exclusive: SCAN_PRESET_OFFSET + SCAN_PRESET_SIZE,
        },
        RegionSpec {
            name: "Group Labels",
            start_offset: GROUP_LABELS_OFFSET,
            end_offset_exclusive: GROUP_LABELS_OFFSET + (GROUP_LABEL_COUNT * GROUP_LABEL_SIZE),
        },
        RegionSpec {
            name: "DTMF Presets",
            start_offset: DTMF_OFFSET,
            end_offset_exclusive: DTMF_OFFSET + DTMF_SIZE,
        },
    ]
}

fn summarize_region(
    data: &[u8],
    name: &str,
    kind: CodeplugRegionKind,
    start_offset: usize,
    end_offset_exclusive: usize,
) -> CodeplugRegion {
    let bytes = &data[start_offset..end_offset_exclusive];
    let non_ff_bytes = bytes.iter().filter(|&&value| value != 0xFF).count();
    let non_zero_bytes = bytes.iter().filter(|&&value| value != 0x00).count();
    let first_non_ff_offset = bytes
        .iter()
        .position(|&value| value != 0xFF)
        .map(|index| start_offset + index);
    let preview_hex = bytes
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");

    CodeplugRegion {
        name: name.to_string(),
        kind,
        start_offset,
        end_offset_exclusive,
        length: end_offset_exclusive - start_offset,
        non_ff_bytes,
        non_zero_bytes,
        first_non_ff_offset,
        preview_hex,
    }
}

fn open_protocol(port_name: &str) -> Result<RadioProtocol> {
    RadioProtocol::new(port_name).map_err(|error| explain_open_error(port_name, error))
}

fn open_handshaken_protocol(port_name: &str) -> Result<RadioProtocol> {
    let mut proto = open_protocol(port_name)?;
    if !proto.handshake().context("Handshake attempt failed")? {
        bail!("Handshake failed for {}", port_name);
    }
    ensure_nicsure_firmware(&mut proto, port_name)?;
    Ok(proto)
}

fn detect_firmware_variant(proto: &mut RadioProtocol) -> Result<FirmwareVariant> {
    let mut ignore_progress = |_event: ProgressEvent| {};
    let data = read_block_range(
        proto,
        SETTINGS_BLOCK_START,
        SETTINGS_BLOCK_COUNT,
        &mut ignore_progress,
    )?;
    Ok(classify_firmware_variant_from_settings(&data))
}

fn ensure_nicsure_firmware(proto: &mut RadioProtocol, port_name: &str) -> Result<()> {
    match detect_firmware_variant(proto)? {
        FirmwareVariant::NicSure => Ok(()),
        FirmwareVariant::Stock => bail!(unsupported_firmware_message(port_name)),
    }
}

fn unsupported_firmware_message(port_name: &str) -> String {
    format!(
        "{} appears to be running the original stock firmware or another unsupported image. {}",
        port_name, NICSURE_REQUIRED_HINT
    )
}

fn classify_firmware_variant_from_settings(data: &[u8]) -> FirmwareVariant {
    let endian = RadioProtocol::infer_settings_endianness(data);
    let settings = RadioProtocol::parse_settings_block(data, endian);

    if settings.magic == SETTINGS_MAGIC {
        FirmwareVariant::NicSure
    } else {
        FirmwareVariant::Stock
    }
}

fn explain_open_error(port_name: &str, error: anyhow::Error) -> anyhow::Error {
    let message = error.to_string();
    if message.contains("Device or resource busy")
        || message.contains("Unable to acquire exclusive lock")
        || message.contains("exclusive lock")
    {
        anyhow!(
            "Failed to open {}: {}. Close other NicTUI sessions or serial tools that may already be using this port.",
            port_name,
            message
        )
    } else {
        anyhow!("Failed to open {}: {}", port_name, message)
    }
}

fn read_block_range<F>(
    proto: &mut RadioProtocol,
    start_block: usize,
    block_count: usize,
    report: &mut F,
) -> Result<Vec<u8>>
where
    F: FnMut(ProgressEvent),
{
    let mut data = Vec::with_capacity(block_count * BLOCK_SIZE);
    for offset in 0..block_count {
        let block = start_block + offset;
        let bytes = proto
            .read_block(block as u8)
            .with_context(|| format!("Failed to read block {}", block))?;
        data.extend_from_slice(&bytes);
        report(ProgressEvent::Progress(
            (offset + 1) as f64 / block_count as f64,
        ));
    }
    Ok(data)
}

fn write_block_range<F>(
    proto: &mut RadioProtocol,
    start_block: usize,
    data: &[u8],
    report: &mut F,
    label: &str,
) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    if !data.len().is_multiple_of(BLOCK_SIZE) {
        bail!(
            "Packed {} payload must align to {} bytes",
            label,
            BLOCK_SIZE
        );
    }

    let block_count = data.len() / BLOCK_SIZE;
    for offset in 0..block_count {
        let block = start_block + offset;
        let start = offset * BLOCK_SIZE;
        let acknowledged = proto
            .write_block(block as u8, &data[start..start + BLOCK_SIZE])
            .with_context(|| format!("Failed to write {} block {}", label, block))?;
        if !acknowledged {
            bail!("Radio rejected {} block {}", label, block);
        }
        report(ProgressEvent::Progress(
            (offset + 1) as f64 / block_count as f64,
        ));
    }

    Ok(())
}

pub(crate) fn validate_channels(channels: &[Channel]) -> Result<()> {
    let mut seen = HashSet::new();
    if channels.len() > CHANNEL_RECORD_COUNT {
        bail!(
            "Found {} channels, but the radio only supports {}.",
            channels.len(),
            CHANNEL_RECORD_COUNT
        );
    }

    for channel in channels {
        if !(1..=198).contains(&channel.channel_num) {
            bail!(
                "Channel {} is out of range. Valid channel numbers are 1-198.",
                channel.channel_num
            );
        }
        if !seen.insert(channel.channel_num) {
            bail!(
                "Channel {} appears more than once. Channel numbers must be unique.",
                channel.channel_num
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_scan_presets(presets: &[ScanPreset]) -> Result<()> {
    validate_indexed_items(
        presets.iter().map(|preset| preset.index),
        SCAN_PRESET_RECORD_COUNT,
        "scan preset",
    )
}

pub(crate) fn validate_band_plans_payload(band_plans: &[BandPlan]) -> Result<()> {
    validate_indexed_items(
        band_plans.iter().map(|plan| plan.index),
        BAND_PLAN_RECORD_COUNT,
        "band plan",
    )
}

pub(crate) fn validate_dtmf_presets_payload(presets: &[DTMFPreset]) -> Result<()> {
    validate_indexed_items(
        presets.iter().map(|preset| preset.index),
        DTMF_PRESET_COUNT,
        "DTMF preset",
    )
}

fn validate_indexed_items<I>(indices: I, max_len: usize, label: &str) -> Result<()>
where
    I: IntoIterator<Item = u8>,
{
    let mut seen = HashSet::new();
    for index in indices {
        if index as usize >= max_len {
            bail!(
                "{} index {} is out of range. Valid indices are 0-{}.",
                label,
                index,
                max_len.saturating_sub(1)
            );
        }
        if !seen.insert(index) {
            bail!(
                "{} index {} appears more than once. Indices must be unique.",
                label,
                index
            );
        }
    }
    Ok(())
}

fn resolve_port_from_candidates(candidates: &[PortCandidate]) -> Result<String> {
    match candidates {
        [] => bail!("No serial ports found. Connect the radio and try again."),
        [single] => return Ok(single.port_name.clone()),
        _ => {}
    }

    let responsive_ports: Vec<&PortCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.is_radio())
        .collect();
    if responsive_ports.len() == 1 {
        return Ok(responsive_ports[0].port_name.clone());
    }

    let likely_ports: Vec<&PortCandidate> = candidates
        .iter()
        .filter(|candidate| matches!(candidate.kind, PortKind::Candidate | PortKind::Radio))
        .collect();
    if likely_ports.len() == 1 {
        return Ok(likely_ports[0].port_name.clone());
    }

    let non_system_ports: Vec<&PortCandidate> = candidates
        .iter()
        .filter(|candidate| !matches!(candidate.kind, PortKind::System))
        .collect();
    if non_system_ports.len() == 1 {
        return Ok(non_system_ports[0].port_name.clone());
    }

    bail!(
        "Multiple serial ports detected. Pass --port explicitly.\nAvailable ports: {}",
        candidates
            .iter()
            .map(|candidate| candidate.port_name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn port_candidate_from_info(port: serialport::SerialPortInfo) -> PortCandidate {
    let port_name = port.port_name;
    let mut score = 0;
    let mut product = None;
    let mut manufacturer = None;
    let mut usb_vid = None;
    let mut usb_pid = None;

    match port.port_type {
        SerialPortType::UsbPort(info) => {
            usb_vid = Some(info.vid);
            usb_pid = Some(info.pid);
            product = info.product;
            manufacturer = info.manufacturer;
            score += 50;

            if matches!((usb_vid, usb_pid), (Some(0x1A86), Some(0x7523))) {
                score += 80;
            }
        }
        SerialPortType::BluetoothPort => score -= 500,
        SerialPortType::PciPort => score -= 100,
        SerialPortType::Unknown => {}
    }

    let normalized = port_name.to_ascii_lowercase();
    if is_auxiliary_port(&port_name) {
        score -= 1_000;
    } else if is_preferred_port(&port_name) {
        score += 20;
    }

    let product_text = product.as_deref().unwrap_or_default().to_ascii_lowercase();
    let manufacturer_text = manufacturer
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    for needle in ["usb serial", "ch340", "wch", "cp210", "uart", "serial"] {
        if product_text.contains(needle) {
            score += 12;
        }
        if manufacturer_text.contains(needle) {
            score += 12;
        }
    }

    let kind = if is_auxiliary_port(&normalized) {
        PortKind::System
    } else if score > 0 {
        PortKind::Candidate
    } else {
        PortKind::Unknown
    };

    PortCandidate {
        port_name,
        kind,
        score,
        product,
        manufacturer,
        usb_vid,
        usb_pid,
        ble_device_id: None,
        ble_rssi: None,
        handshake_ok: false,
        firmware_variant: None,
    }
}

fn should_probe_candidate(candidate: &PortCandidate) -> bool {
    !matches!(candidate.kind, PortKind::System) && candidate.score > -50
}

fn dedupe_port_candidates(mut candidates: Vec<PortCandidate>) -> Vec<PortCandidate> {
    candidates.sort_by(|left, right| {
        canonical_port_group(&left.port_name)
            .cmp(&canonical_port_group(&right.port_name))
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| {
                preferred_alias_rank(&left.port_name).cmp(&preferred_alias_rank(&right.port_name))
            })
            .then_with(|| left.port_name.cmp(&right.port_name))
    });

    let mut deduped = Vec::new();
    let mut last_group = None::<String>;
    for candidate in candidates {
        let group = canonical_port_group(&candidate.port_name);
        if last_group.as_deref() == Some(group.as_str()) {
            continue;
        }
        last_group = Some(group);
        deduped.push(candidate);
    }

    deduped
}

fn port_sort_key(port: &str) -> (u8, String) {
    let rank = if is_preferred_port(port) {
        0
    } else if is_auxiliary_port(port) {
        2
    } else {
        1
    };

    (rank, port.to_ascii_lowercase())
}

fn canonical_port_group(port: &str) -> String {
    if let Some(suffix) = port.strip_prefix("/dev/cu.") {
        return suffix.to_ascii_lowercase();
    }
    if let Some(suffix) = port.strip_prefix("/dev/tty.") {
        return suffix.to_ascii_lowercase();
    }

    port.to_ascii_lowercase()
}

fn preferred_alias_rank(port: &str) -> u8 {
    if port.starts_with("/dev/cu.") {
        0
    } else if port.starts_with("/dev/tty.") {
        1
    } else {
        2
    }
}

fn is_preferred_port(port: &str) -> bool {
    let normalized = port.to_ascii_lowercase();
    normalized.starts_with("com")
        || normalized.contains("usbserial")
        || normalized.contains("usbmodem")
        || normalized.contains("wchusbserial")
        || normalized.contains("slab_usbtouart")
        || normalized.contains("ttyusb")
        || normalized.contains("ttyacm")
        || normalized.contains("/cu.usb")
        || normalized.contains("/tty.usb")
}

fn is_auxiliary_port(port: &str) -> bool {
    let normalized = port.to_ascii_lowercase();
    normalized.contains("bluetooth") || normalized.contains("debug-console")
}

#[cfg(test)]
mod tests {
    use super::{
        CodeplugRegionKind, FirmwareVariant, PortCandidate, PortKind, build_codeplug_regions,
        classify_firmware_variant_from_settings, dedupe_port_candidates, known_codeplug_regions,
        port_sort_key, resolve_port_from_candidates,
    };
    use crate::protocol::EEPROM_SIZE;
    use crate::protocol::radio::SETTINGS_MAGIC;
    use std::time::Duration;

    #[test]
    fn sorts_usb_radio_ports_ahead_of_auxiliary_ports() {
        let mut ports = [
            "/dev/cu.Bluetooth-Incoming-Port".to_string(),
            "/dev/cu.usbserial-210".to_string(),
            "/dev/cu.debug-console".to_string(),
        ];

        ports.sort_by_key(|port| port_sort_key(port));

        assert_eq!(ports[0], "/dev/cu.usbserial-210");
        assert_eq!(ports[1], "/dev/cu.Bluetooth-Incoming-Port");
        assert_eq!(ports[2], "/dev/cu.debug-console");
    }

    #[test]
    fn resolves_single_preferred_port_when_multiple_ports_exist() {
        let ports = vec![
            test_candidate("/dev/cu.Bluetooth-Incoming-Port", PortKind::System, 0),
            test_candidate("/dev/cu.usbserial-210", PortKind::Candidate, 100),
            test_candidate("/dev/cu.debug-console", PortKind::System, 0),
        ];

        let resolved = resolve_port_from_candidates(&ports).unwrap();
        assert_eq!(resolved, "/dev/cu.usbserial-210");
    }

    #[test]
    fn errors_when_multiple_real_candidates_exist() {
        let ports = vec![
            test_candidate("/dev/cu.usbserial-210", PortKind::Candidate, 100),
            test_candidate("/dev/cu.usbmodem123", PortKind::Candidate, 90),
        ];

        let error = resolve_port_from_candidates(&ports)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Multiple serial ports detected"));
    }

    #[test]
    fn prefers_single_responsive_radio_port() {
        let mut radio = test_candidate("/dev/cu.usbserial-2110", PortKind::Candidate, 100);
        radio.kind = PortKind::Radio;
        radio.handshake_ok = true;
        let ports = vec![
            test_candidate("/dev/cu.usbmodem123", PortKind::Candidate, 90),
            radio,
        ];

        let resolved = resolve_port_from_candidates(&ports).unwrap();
        assert_eq!(resolved, "/dev/cu.usbserial-2110");
    }

    #[test]
    fn dedupes_cu_and_tty_aliases_for_same_device() {
        let ports = vec![
            test_candidate("/dev/tty.usbserial-2110", PortKind::Candidate, 100),
            test_candidate("/dev/cu.usbserial-2110", PortKind::Candidate, 100),
            test_candidate("/dev/cu.debug-console", PortKind::System, -1000),
        ];

        let deduped = dedupe_port_candidates(ports);
        let names = deduped
            .iter()
            .map(|candidate| candidate.port_name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"/dev/cu.usbserial-2110"));
        assert!(!names.contains(&"/dev/tty.usbserial-2110"));
    }

    #[test]
    fn remote_command_schedule_resets_when_session_rearms() {
        let schedule = super::RemoteCommandSchedule::new(Duration::from_millis(250));

        assert!(!schedule.command_due(Duration::from_millis(249)));
        assert!(schedule.command_due(Duration::from_millis(250)));

        schedule.mark_sent(Duration::from_millis(250), Duration::from_millis(350));
        assert!(!schedule.command_due(Duration::from_millis(599)));
        assert!(schedule.command_due(Duration::from_millis(600)));

        schedule.reset(Duration::from_millis(250));
        assert!(!schedule.command_due(Duration::from_millis(249)));
        assert!(schedule.command_due(Duration::from_millis(250)));
    }

    #[test]
    fn classifies_nicsure_firmware_from_settings_magic() {
        let mut data = vec![0u8; 128];
        data[0] = (SETTINGS_MAGIC >> 8) as u8;
        data[1] = SETTINGS_MAGIC as u8;

        assert_eq!(
            classify_firmware_variant_from_settings(&data),
            FirmwareVariant::NicSure
        );
    }

    #[test]
    fn classifies_stock_firmware_when_settings_magic_is_missing() {
        let data = vec![0u8; 128];

        assert_eq!(
            classify_firmware_variant_from_settings(&data),
            FirmwareVariant::Stock
        );
    }

    #[test]
    fn known_regions_do_not_overlap_and_stay_in_bounds() {
        let mut regions = known_codeplug_regions();
        regions.sort_by_key(|region| region.start_offset);

        let mut cursor = 0usize;
        for region in regions {
            assert!(region.start_offset >= cursor);
            assert!(region.end_offset_exclusive <= EEPROM_SIZE);
            assert!(region.end_offset_exclusive > region.start_offset);
            cursor = region.end_offset_exclusive;
        }
    }

    #[test]
    fn build_codeplug_regions_includes_unknown_gaps_with_live_data_counts() {
        let mut data = vec![0xFFu8; EEPROM_SIZE];
        data[0x1AE0] = 0x34;
        data[0x1E10] = 0x56;

        let regions = build_codeplug_regions(&data);
        let unknown_regions: Vec<_> = regions
            .iter()
            .filter(|region| matches!(region.kind, CodeplugRegionKind::Unknown))
            .collect();

        assert!(!unknown_regions.is_empty());
        assert!(unknown_regions.iter().any(|region| {
            region.start_offset == 0x1ACA
                && region.end_offset_exclusive == 0x1B00
                && region.non_ff_bytes == 1
                && region.first_non_ff_offset == Some(0x1AE0)
        }));
        assert!(unknown_regions.iter().any(|region| {
            region.start_offset == 0x1DF4
                && region.end_offset_exclusive == EEPROM_SIZE
                && region.non_ff_bytes == 1
                && region.first_non_ff_offset == Some(0x1E10)
        }));
    }

    #[test]
    fn inspect_codeplug_parses_vfo_memories_from_leading_records() {
        let raw = [
            0x00, 0xC9, 0x6A, 0x80, 0x00, 0xC9, 0x6A, 0x80, 0x00, 0x00, 0x00, 0x00, 0x82, 0x00,
            0x00, 0x04, 0xFF, 0xFF, 0xFF, 0xFF, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
            0x20, 0x20, 0x20, 0x00, 0x02, 0xB4, 0x19, 0xBC, 0x02, 0xB4, 0x19, 0xBC, 0x00, 0x00,
            0x00, 0x00, 0x82, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x20, 0x20, 0x20, 0x20,
            0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00,
        ];
        let mut data = vec![0xFFu8; EEPROM_SIZE];
        data[..raw.len()].copy_from_slice(&raw);

        let inspection = super::inspect_codeplug(&data).unwrap();

        assert_eq!(inspection.vfo_memory_count, 2);
        assert_eq!(inspection.vfo_memories[0].slot, "A");
        assert_eq!(inspection.vfo_memories[0].channel.rx_freq, "132.00000");
        assert_eq!(inspection.vfo_memories[1].slot, "B");
        assert_eq!(inspection.vfo_memories[1].channel.rx_freq, "453.57500");
    }

    fn test_candidate(port_name: &str, kind: PortKind, score: i32) -> PortCandidate {
        PortCandidate {
            port_name: port_name.to_string(),
            kind,
            score,
            product: None,
            manufacturer: None,
            usb_vid: None,
            usb_pid: None,
            ble_device_id: None,
            ble_rssi: None,
            handshake_ok: false,
            firmware_variant: None,
        }
    }
}
