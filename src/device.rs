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
use crate::protocol::radio::{
    DTMF_PRESET_RECORD_COUNT, DTMF_PRESET_RECORD_SIZE, ReadTiming, SETTINGS_MAGIC,
};
use crate::protocol::{
    BAND_PLAN_RECORD_COUNT, BAND_PLAN_RECORD_SIZE, BIN_FLASH_BAUD_RATE, BLOCK_SIZE, BandPlan,
    Channel, DTMFPreset, EEPROM_SIZE, Endianness, GROUP_LABEL_COUNT, GROUP_LABEL_SIZE,
    GROUP_LABEL_TEXT_SIZE, GROUP_LABELS_BLOCK_COUNT, GROUP_LABELS_BLOCK_OFFSET,
    GROUP_LABELS_BLOCK_START, GROUP_LABELS_OFFSET, RadioProtocol, RemotePacket,
    SCAN_PRESET_RECORD_COUNT, SCAN_PRESET_RECORD_SIZE, ScanPreset, SettingsBlock, TOTAL_BLOCKS,
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
    (SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE).div_ceil(BLOCK_SIZE);
const SCAN_PRESET_OFFSET: usize = 0x1B00;
const SCAN_PRESET_SIZE: usize = SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE;
const BAND_PLAN_START: usize = 0x1A00 / BLOCK_SIZE;
const BAND_PLAN_BLOCK_COUNT: usize = 7;
const BAND_PLAN_OFFSET: usize = 0x1A02;
const BAND_PLAN_SIZE: usize = BAND_PLAN_RECORD_COUNT * BAND_PLAN_RECORD_SIZE;
const DTMF_START: usize = 0x1CF0 / BLOCK_SIZE;
const DTMF_BLOCK_OFFSET: usize = 0x1CF0 % BLOCK_SIZE;
const DTMF_RECORD_SIZE: usize = DTMF_PRESET_RECORD_SIZE;
const DTMF_PRESET_COUNT: usize = DTMF_PRESET_RECORD_COUNT;
const DTMF_OFFSET: usize = 0x1CF0;
const DTMF_SIZE: usize = DTMF_PRESET_COUNT * DTMF_RECORD_SIZE;
const DTMF_BLOCK_COUNT: usize = (DTMF_BLOCK_OFFSET + DTMF_SIZE).div_ceil(BLOCK_SIZE);
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

    fn on_phase(&self, phase: RemoteSessionPhase, start_delay: Duration) {
        // Armed also follows commands without telemetry; only Opening starts
        // a session with a new elapsed-time base, including recovery attempts.
        if phase == RemoteSessionPhase::Opening {
            self.next_command_at.set(start_delay);
        }
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

pub fn read_codeplug<F>(port_name: &str, report: F) -> Result<(Vec<u8>, Endianness)>
where
    F: FnMut(ProgressEvent),
{
    read_codeplug_with_timing(port_name, normal_read_timing(port_name), report)
}

fn normal_read_timing(port_name: &str) -> ReadTiming {
    if crate::ble::parse_ble_device_uri(port_name).is_some() {
        ReadTiming::Conservative
    } else {
        ReadTiming::Responsive
    }
}

struct ReadSession {
    protocol: RadioProtocol,
    resume_required: bool,
}

impl ReadSession {
    fn read_blocks<F>(
        &mut self,
        start_block: usize,
        block_count: usize,
        report: &mut F,
    ) -> Result<Vec<u8>>
    where
        F: FnMut(ProgressEvent),
    {
        let mut data = Vec::with_capacity(block_count * BLOCK_SIZE);
        for offset in 0..block_count {
            check_read_interruption()?;
            let block = start_block + offset;
            let bytes = self
                .protocol
                .read_block(block as u8)
                .with_context(|| format!("Failed to read block {block}"))?;
            data.extend_from_slice(&bytes);
            report(ProgressEvent::Progress(
                (offset + 1) as f64 / block_count as f64,
            ));
        }
        check_read_interruption()?;
        Ok(data)
    }

    fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        let resume_result = if self.resume_required {
            self.protocol
                .enable_radio()
                .context("Failed to resume radio after read")
                .and_then(|ack| {
                    if ack {
                        self.resume_required = false;
                        Ok(())
                    } else {
                        bail!("Radio did not acknowledge read resume");
                    }
                })
        } else {
            Ok(())
        };
        // Keep cleanup armed after a failed ACK for one final best-effort
        // attempt on drop, without reporting the read as successful.
        match (result, resume_result) {
            (Ok(data), Ok(())) => Ok(data),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(resume_error)) => {
                Err(error.context(format!("Radio resume also failed: {resume_error:#}")))
            }
        }
    }
}

impl Drop for ReadSession {
    fn drop(&mut self) {
        if self.resume_required {
            let _ = self.protocol.enable_radio();
        }
    }
}

fn check_read_interruption() -> Result<()> {
    if crate::cancellation::is_requested() {
        bail!("Interrupted: signal requested during radio read");
    }
    Ok(())
}

/// Read a complete snapshot, optionally using response-paced USB transactions.
pub fn read_codeplug_with_timing<F>(
    port_name: &str,
    timing: ReadTiming,
    mut report: F,
) -> Result<(Vec<u8>, Endianness)>
where
    F: FnMut(ProgressEvent),
{
    if timing == ReadTiming::Responsive && crate::ble::parse_ble_device_uri(port_name).is_some() {
        bail!("Response-paced reads currently require USB serial; use conservative timing for BLE");
    }
    report(ProgressEvent::Status(format!(
        "Opening {} and reading full radio memory",
        port_name
    )));

    let eeprom = read_memory_range(port_name, timing, 0, TOTAL_BLOCKS, &mut report)?;
    let endian = infer_endianness_from_codeplug(&eeprom);
    report(ProgressEvent::Status(format!(
        "Read {} bytes from radio",
        eeprom.len()
    )));

    Ok((eeprom, endian))
}

fn read_normal_memory_range<F>(
    port_name: &str,
    start_block: usize,
    block_count: usize,
    report: &mut F,
) -> Result<Vec<u8>>
where
    F: FnMut(ProgressEvent),
{
    read_memory_range(
        port_name,
        normal_read_timing(port_name),
        start_block,
        block_count,
        report,
    )
}

fn read_memory_range<F>(
    port_name: &str,
    timing: ReadTiming,
    start_block: usize,
    block_count: usize,
    report: &mut F,
) -> Result<Vec<u8>>
where
    F: FnMut(ProgressEvent),
{
    with_read_session(port_name, timing, |session| {
        session.read_blocks(start_block, block_count, report)
    })
}

fn with_read_session<T>(
    port_name: &str,
    timing: ReadTiming,
    read: impl FnOnce(&mut ReadSession) -> Result<T>,
) -> Result<T> {
    check_read_interruption()?;
    let pause_radio = timing == ReadTiming::Responsive;
    // Identification is shared with writes and is deliberately not cancellable.
    let protocol = open_handshaken_protocol(port_name)?;
    check_read_interruption()?;
    let mut session = ReadSession {
        // Identify supported firmware conservatively before pausing the radio.
        protocol,
        // Arm before sending pause: a missing ACK may still mean it paused.
        resume_required: pause_radio,
    };
    let result = (|| {
        if pause_radio {
            if !session
                .protocol
                .disable_radio()
                .context("Failed to pause radio for read")?
            {
                bail!("Radio did not acknowledge read pause");
            }
            session.protocol.set_read_timing(timing);
        }
        let data = read(&mut session)?;
        check_read_interruption()?;
        Ok(data)
    })();
    let data = session.finish(result)?;
    // A signal arriving during the resume ACK must not produce a successful backup.
    check_read_interruption()?;
    Ok(data)
}

fn read_memory_with_group_labels<F>(
    port_name: &str,
    start_block: usize,
    block_count: usize,
    report: &mut F,
) -> Result<(Vec<u8>, Result<Vec<String>>)>
where
    F: FnMut(ProgressEvent),
{
    let timing = normal_read_timing(port_name);
    let primary_blocks = start_block..start_block + block_count;
    let label_blocks =
        GROUP_LABELS_BLOCK_START..GROUP_LABELS_BLOCK_START + GROUP_LABELS_BLOCK_COUNT;
    // Only a paused USB session guarantees that overlapping bytes are stable.
    let reuse_overlap = timing == ReadTiming::Responsive;
    let overlap = label_blocks
        .clone()
        .filter(|block| reuse_overlap && primary_blocks.contains(block))
        .count();
    let total = block_count + GROUP_LABELS_BLOCK_COUNT - overlap;
    let mut completed = 0;
    let mut complete_block = || {
        completed += 1;
        report(ProgressEvent::Progress(completed as f64 / total as f64));
    };

    with_read_session(port_name, timing, |session| {
        let primary = session.read_blocks(start_block, block_count, &mut |_| complete_block())?;
        let labels = (|| {
            let mut data = Vec::with_capacity(GROUP_LABELS_BLOCK_COUNT * BLOCK_SIZE);
            for block in label_blocks {
                check_read_interruption()?;
                if reuse_overlap && primary_blocks.contains(&block) {
                    let offset = (block - start_block) * BLOCK_SIZE;
                    data.extend_from_slice(&primary[offset..offset + BLOCK_SIZE]);
                } else {
                    let bytes = session
                        .protocol
                        .read_block(block as u8)
                        .with_context(|| format!("Failed to read group names block {block}"))?;
                    data.extend_from_slice(&bytes);
                    complete_block();
                }
            }
            Ok(parse_group_label_memory(&data))
        })();
        Ok((primary, labels))
    })
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
    validate_codeplug_payload(codeplug_data)?;

    report(ProgressEvent::Status(format!(
        "Opening {} and writing full radio memory",
        port_name
    )));

    let mut proto = open_handshaken_protocol(port_name)?;
    let current_settings = proto.read_block(SETTINGS_BLOCK_START as u8)?;
    let current_band_plan = proto.read_block(BAND_PLAN_START as u8)?;
    if codeplug_data[SETTINGS_OFFSET..SETTINGS_OFFSET + 2] != current_settings[..2]
        || codeplug_data[0x1A00..0x1A02] != current_band_plan[..2]
    {
        bail!(
            "Codeplug version or byte order does not match the target radio; no blocks were written"
        );
    }
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

pub(crate) fn validate_codeplug_payload(data: &[u8]) -> Result<()> {
    if data.len() != EEPROM_SIZE {
        bail!(
            "Invalid codeplug size: expected {EEPROM_SIZE} bytes, got {} bytes",
            data.len()
        );
    }
    let settings = &data[SETTINGS_OFFSET..SETTINGS_OFFSET + 2];
    if settings != SETTINGS_MAGIC.to_be_bytes() && settings != SETTINGS_MAGIC.to_le_bytes() {
        bail!("Codeplug has no supported NicSure settings marker");
    }
    let band_plan = &data[0x1A00..0x1A02];
    if band_plan != 0xA46Du16.to_be_bytes() && band_plan != 0xA46Du16.to_le_bytes() {
        bail!("Codeplug has no supported NicSure band-plan version marker");
    }
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

    let data = read_normal_memory_range(
        port_name,
        CHANNEL_START_BLOCK,
        CHANNEL_RECORD_COUNT,
        &mut report,
    )?;
    let (channels, channel_endian) = parse_channel_memory(&data);

    report(ProgressEvent::Status(format!(
        "Loaded {} channels",
        channels.len()
    )));

    Ok((channels, channel_endian))
}

/// Read channels and group names in one session. Label errors are nonfatal;
/// handshake, primary-read, and radio-resume errors fail the entire operation.
pub fn read_channels_with_group_labels<F>(
    port_name: &str,
    mut report: F,
) -> Result<(Vec<Channel>, Endianness, Result<Vec<String>>)>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading channels and group names",
        port_name
    )));
    let (data, labels) = read_memory_with_group_labels(
        port_name,
        CHANNEL_START_BLOCK,
        CHANNEL_RECORD_COUNT,
        &mut report,
    )?;
    let (channels, endian) = parse_channel_memory(&data);
    report(ProgressEvent::Status(format!(
        "Loaded {} channels",
        channels.len()
    )));
    Ok((channels, endian, labels))
}

fn parse_channel_memory(data: &[u8]) -> (Vec<Channel>, Endianness) {
    let mut eeprom = vec![0u8; (CHANNEL_START_BLOCK + CHANNEL_RECORD_COUNT) * BLOCK_SIZE];
    eeprom[CHANNEL_DATA_OFFSET..].copy_from_slice(data);

    let channel_endian = infer_channel_endianness_from_codeplug(&eeprom);
    let channels = extract_channels_from_codeplug(&eeprom, channel_endian);

    (channels, channel_endian)
}

pub fn write_channels<F>(
    port_name: &str,
    channels: &[Channel],
    deleted_channels: &[u16],
    _default_endian: Endianness,
    reboot: bool,
    mut report: F,
) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_channels(channels)?;
    let mut seen: HashSet<u16> = channels.iter().map(|channel| channel.channel_num).collect();
    for &channel in deleted_channels {
        if !(1..=198).contains(&channel) || !seen.insert(channel) {
            bail!("Invalid, duplicate, or conflicting channel deletion: {channel}");
        }
    }

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
    let channel_endian = proto
        .detect_channel_endianness()
        .context("Failed to determine channel byte order; no channel blocks were written")?;
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

    let data = read_normal_memory_range(
        port_name,
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
    let mut packed = current;
    let encoded = RadioProtocol::pack_settings_block(settings, endian);
    // Fields beyond the modeled prefix belong to newer firmware versions.
    packed[..0x67].copy_from_slice(&encoded[..0x67]);
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

    let data = read_normal_memory_range(
        port_name,
        SCAN_PRESET_START,
        SCAN_PRESET_BLOCK_COUNT,
        &mut report,
    )?;
    let (presets, endian) = parse_scan_preset_memory(&data);
    report(ProgressEvent::Status(format!(
        "Read {} scan presets",
        presets.len()
    )));
    Ok((presets, endian))
}

/// Read scan presets and group names in one session, reusing shared blocks only
/// while paused. Label errors are nonfatal, but radio-resume errors are fatal.
pub fn read_scan_presets_with_group_labels<F>(
    port_name: &str,
    mut report: F,
) -> Result<(Vec<ScanPreset>, Endianness, Result<Vec<String>>)>
where
    F: FnMut(ProgressEvent),
{
    report(ProgressEvent::Status(format!(
        "Opening {} and reading scan presets and group names",
        port_name
    )));
    let (data, labels) = read_memory_with_group_labels(
        port_name,
        SCAN_PRESET_START,
        SCAN_PRESET_BLOCK_COUNT,
        &mut report,
    )?;
    let (presets, endian) = parse_scan_preset_memory(&data);
    report(ProgressEvent::Status(format!(
        "Read {} scan presets",
        presets.len()
    )));
    Ok((presets, endian, labels))
}

fn parse_scan_preset_memory(data: &[u8]) -> (Vec<ScanPreset>, Endianness) {
    let endian = RadioProtocol::infer_scan_preset_endianness(data);

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

    (presets, endian)
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
    let endian = RadioProtocol::infer_scan_preset_endianness_checked(&existing[..SCAN_PRESET_SIZE])
        .context("Scan byte order is uncertain; no scan preset blocks were written")?;
    let mut packed = existing;
    // The final scan block also contains the beginning of the group-name table.
    packed[..SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE].fill(0xFF);

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
    let endian = RadioProtocol::infer_scan_preset_endianness_checked(&packed[..SCAN_PRESET_SIZE])
        .context("Scan byte order is uncertain; no scan preset blocks were written")?;
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

    let data = read_normal_memory_range(
        port_name,
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

    let data = read_normal_memory_range(port_name, DTMF_START, DTMF_BLOCK_COUNT, &mut report)?;

    let mut presets = Vec::new();
    for index in 0..DTMF_PRESET_COUNT {
        let start = DTMF_BLOCK_OFFSET + index * DTMF_RECORD_SIZE;
        let end = start + DTMF_RECORD_SIZE;
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

    let data = read_normal_memory_range(
        port_name,
        GROUP_LABELS_BLOCK_START,
        GROUP_LABELS_BLOCK_COUNT,
        &mut report,
    )?;
    let labels = parse_group_label_memory(&data);

    report(ProgressEvent::Status(format!(
        "Loaded {} group names",
        labels
            .iter()
            .filter(|label| !label.trim().is_empty())
            .count()
    )));
    Ok(labels)
}

fn parse_group_label_memory(data: &[u8]) -> Vec<String> {
    let start = GROUP_LABELS_BLOCK_OFFSET;
    let end = start + GROUP_LABEL_COUNT * GROUP_LABEL_SIZE;
    RadioProtocol::parse_group_labels(&data[start..end])
}

pub fn write_group_labels<F>(port_name: &str, labels: &[String], mut report: F) -> Result<()>
where
    F: FnMut(ProgressEvent),
{
    validate_group_labels_payload(labels)?;
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
    let mut packed = read_block_range(&mut proto, DTMF_START, DTMF_BLOCK_COUNT, &mut report)?;
    packed[DTMF_BLOCK_OFFSET..DTMF_BLOCK_OFFSET + DTMF_SIZE].fill(0xFF);

    for preset in presets {
        let offset = DTMF_BLOCK_OFFSET + preset.index as usize * DTMF_RECORD_SIZE;
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
    let offset = DTMF_BLOCK_OFFSET + preset.index as usize * DTMF_RECORD_SIZE;
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
    if crate::ble::parse_ble_device_uri(port_name).is_some() {
        bail!("Firmware flashing requires a USB serial connection");
    }
    const INIT_SEQUENCE: [u8; 36] = [
        0xA0, 0xEE, 0x74, 0x71, 0x07, 0x74, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
        0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    ];

    validate_firmware_data(firmware_data)?;
    let block_count = firmware_data.len().div_ceil(BLOCK_SIZE);
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
    loop {
        if start_time.elapsed() > timeout {
            bail!("Timeout waiting for radio bootloader");
        }

        match proto
            .read_byte()
            .context("Failed while waiting for bootloader")?
        {
            Some(0xA5) => {
                proto
                    .send_bytes(&INIT_SEQUENCE)
                    .context("Failed to send bootloader init sequence")?;
                break;
            }
            Some(_) => {}
            None => {}
        }
    }

    // Accept the init ACK, or allow bootloaders that go quiet without one.
    // Drain trailing A5 bytes so they cannot be mistaken for a data-block ACK.
    let settle_started = Instant::now();
    let mut last_activity = settle_started;
    loop {
        if settle_started.elapsed() >= Duration::from_secs(2) {
            bail!("Timeout waiting for bootloader initialization to settle");
        }
        match proto.read_byte()? {
            Some(0xA3) => break,
            Some(0xA5) => last_activity = Instant::now(),
            None if last_activity.elapsed() >= Duration::from_millis(400) => break,
            None => {}
            Some(byte) => bail!("Unexpected bootloader initialization response: {byte:02X}"),
        }
    }
    report(ProgressEvent::Status(
        "Bootloader handshake detected. Starting transfer.".to_string(),
    ));

    for (block, data) in firmware_data.chunks(BLOCK_SIZE).enumerate() {
        let mut packet = [0u8; 36];
        packet[0] = if block + 1 == block_count { 0xA2 } else { 0xA1 };
        packet[1..3].copy_from_slice(&(block as u16).to_be_bytes());
        packet[4..4 + data.len()].copy_from_slice(data);
        packet[3] = packet[4..]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        // NicSure's firmware_flash_protocol.txt requires A3 for every data
        // packet, including the final A2. Lost ACKs may only retry this block.
        for attempt in 1..=3 {
            proto
                .send_bytes(&packet)
                .with_context(|| format!("Failed to send firmware block {block}"))?;
            let sent_at = Instant::now();
            let mut acknowledged = false;
            while sent_at.elapsed() < Duration::from_millis(500) {
                match proto.read_byte().with_context(|| {
                    format!("Failed to receive bootloader ACK for block {block}")
                })? {
                    Some(0xA3) => {
                        acknowledged = true;
                        break;
                    }
                    Some(byte) => {
                        bail!("Bootloader rejected block {block}: expected A3, got {byte:02X}");
                    }
                    None => {}
                }
            }
            if acknowledged {
                break;
            }
            if attempt == 3 {
                bail!("Missing bootloader ACK for block {block} after 3 attempts");
            }
        }
        report(ProgressEvent::Progress(
            (block + 1) as f64 / block_count as f64,
        ));
    }
    report(ProgressEvent::Status(
        "Firmware transfer acknowledged. Power-cycle the radio to boot the new image.".to_string(),
    ));
    Ok(())
}

pub(crate) fn validate_firmware_data(data: &[u8]) -> Result<()> {
    if data.is_empty() {
        bail!("Firmware image is empty");
    }
    if data.len() > 0xF800 {
        bail!("Firmware image exceeds the radio limit of 63488 bytes");
    }
    Ok(())
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
            if let RemoteCaptureEvent::Phase(phase) = &event {
                schedule.on_phase(*phase, options.command_start_delay);
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
                RemoteCaptureEvent::Received(_) => {}
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
        0x11 => "B",
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
        validate_label(&channel.name, 12, "channel name")?;
        validate_channel_frequency(&channel.rx_freq, false)?;
        validate_channel_frequency(&channel.tx_freq, true)?;
        validate_tone(&channel.rx_tone)?;
        validate_tone(&channel.tx_tone)?;
        if !matches!(channel.bandwidth.as_str(), "Wide" | "Narrow")
            || !matches!(channel.modulation.as_str(), "Auto" | "FM" | "AM" | "USB")
            || channel.position > 1
            || channel.ptt_id > 3
            || channel.groups.iter().any(|group| *group > 15)
        {
            bail!(
                "Channel {} contains an invalid mode, flag, or group slot",
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
    )?;
    for preset in presets {
        validate_label(&preset.label, 8, "scan preset label")?;
        let enabled = preset.start_freq != 0;
        if (enabled
            && (!(1_800_000..=130_000_000).contains(&preset.start_freq)
                || preset.range == 0
                || preset.step == 0))
            || preset.range > 60_000
            || preset.step > 50_000
            || preset.resume > 250
            || preset.persist > 200
            || preset.modulation > 2
            || preset.ultrascan > 20
        {
            bail!(
                "Scan preset {} contains an invalid frequency, range, step, timing, or mode",
                preset.index
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_band_plans_payload(band_plans: &[BandPlan]) -> Result<()> {
    validate_indexed_items(
        band_plans.iter().map(|plan| plan.index),
        BAND_PLAN_RECORD_COUNT,
        "band plan",
    )?;
    for plan in band_plans {
        if plan.start_freq > 130_000_000
            || plan.end_freq > 130_000_000
            || ((plan.start_freq != 0 || plan.end_freq != 0) && plan.start_freq >= plan.end_freq)
            || plan.modulation > 7
            || plan.bandwidth > 5
        {
            bail!(
                "Band plan {} contains invalid frequency bounds or mode",
                plan.index
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_dtmf_presets_payload(presets: &[DTMFPreset]) -> Result<()> {
    validate_indexed_items(
        presets.iter().map(|preset| preset.index),
        DTMF_PRESET_COUNT,
        "DTMF preset",
    )?;
    for preset in presets {
        validate_label(&preset.label, 7, "DTMF label")?;
        if preset.digits.len() > 9 || preset.digits.iter().any(|digit| *digit > 15) {
            bail!(
                "DTMF preset {} accepts at most nine digits, each from 0 to 15",
                preset.index
            );
        }
    }
    Ok(())
}

fn validate_label(value: &str, max_bytes: usize, label: &str) -> Result<()> {
    if value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        bail!("{label} must contain at most {max_bytes} printable ASCII bytes");
    }
    Ok(())
}

pub(crate) fn validate_group_labels_payload(labels: &[String]) -> Result<()> {
    if labels.len() > GROUP_LABEL_COUNT {
        bail!("Radio supports at most {GROUP_LABEL_COUNT} group labels");
    }
    for label in labels {
        validate_label(label, GROUP_LABEL_TEXT_SIZE, "group label")?;
    }
    Ok(())
}

fn validate_channel_frequency(value: &str, allow_zero: bool) -> Result<()> {
    let parsed = value
        .parse::<f64>()
        .context("Channel frequency must be numeric MHz")?;
    if !parsed.is_finite() || !((18.0..=1300.0).contains(&parsed) || allow_zero && parsed == 0.0) {
        bail!(
            "Channel frequency must be 18-1300 MHz{}",
            if allow_zero {
                " or 0 for TX disabled"
            } else {
                ""
            }
        );
    }
    let scaled = parsed * 100_000.0;
    if (scaled - scaled.round()).abs() > 0.00001 {
        bail!("Channel frequency must be representable in 10 Hz steps");
    }
    Ok(())
}

fn validate_tone(value: &str) -> Result<()> {
    if value.is_empty() || value.eq_ignore_ascii_case("off") {
        return Ok(());
    }
    let upper = value.to_ascii_uppercase();
    if let Some(dcs) = upper.strip_prefix('D') {
        let digits = dcs.strip_suffix(['I', 'N']).unwrap_or(dcs);
        if digits.is_empty() || digits.len() > 3 || u16::from_str_radix(digits, 8).is_err() {
            bail!("Invalid DCS tone {value:?}: use octal D000-D777 with optional n/i suffix");
        }
        return Ok(());
    }
    let tone = value.parse::<f64>().context("Invalid CTCSS tone")?;
    if !tone.is_finite()
        || !(0.0..=300.0).contains(&tone)
        || (tone * 10.0 - (tone * 10.0).round()).abs() > 0.00001
    {
        bail!("CTCSS tone must be 0-300 Hz in 0.1 Hz steps");
    }
    Ok(())
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
    fn normal_reads_select_responsive_serial_but_keep_ble_conservative() {
        use crate::protocol::radio::ReadTiming;

        for port in [
            "/dev/ttyUSB0",
            "/dev/cu.usbserial-test",
            "COM3",
            "/dev/pts/1",
        ] {
            assert_eq!(super::normal_read_timing(port), ReadTiming::Responsive);
        }
        for port in ["ble://read-test", "ble:read-test"] {
            assert_eq!(super::normal_read_timing(port), ReadTiming::Conservative);
        }
        assert_eq!(ReadTiming::default(), ReadTiming::Conservative);
    }

    #[test]
    fn semantic_validation_rejects_lossy_or_invalid_payloads() {
        let channel = crate::protocol::Channel {
            channel_num: 1,
            name: "TEST".into(),
            rx_freq: "145.35000".into(),
            tx_freq: "0".into(),
            rx_tone: "D023n".into(),
            tx_tone: "Off".into(),
            bandwidth: "Wide".into(),
            modulation: "FM".into(),
            ..Default::default()
        };
        assert!(super::validate_channels(std::slice::from_ref(&channel)).is_ok());
        for value in ["NaN", "inf", "-145", "not a frequency", "145.000001"] {
            let mut invalid = channel.clone();
            invalid.rx_freq = value.into();
            assert!(super::validate_channels(&[invalid]).is_err(), "{value}");
        }
        for tone in ["D089n", "D1000n", "garbage", "NaN", "301"] {
            let mut invalid = channel.clone();
            invalid.tx_tone = tone.into();
            assert!(super::validate_channels(&[invalid]).is_err(), "{tone}");
        }
        let mut preset = crate::protocol::ScanPreset {
            start_freq: 14_500_000,
            range: 100,
            step: 1250,
            ..Default::default()
        };
        assert!(super::validate_scan_presets(&[preset.clone()]).is_ok());
        preset.step = 0;
        assert!(super::validate_scan_presets(&[preset]).is_err());
        let invalid = crate::protocol::DTMFPreset {
            index: 0,
            digits: vec![255; 11],
            label: String::new(),
        };
        assert!(super::validate_dtmf_presets_payload(&[invalid]).is_err());
    }

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
    fn remote_command_schedule_resets_only_for_new_sessions() {
        use crate::remote::RemoteSessionPhase;

        let start_delay = Duration::from_millis(200);
        let schedule = super::RemoteCommandSchedule::new(start_delay);

        schedule.on_phase(RemoteSessionPhase::Opening, start_delay);
        schedule.on_phase(RemoteSessionPhase::Armed, start_delay);
        assert!(!schedule.command_due(Duration::from_millis(199)));
        assert!(schedule.command_due(start_delay));

        schedule.mark_sent(start_delay, Duration::from_millis(200));
        for phase in [
            RemoteSessionPhase::Probing,
            RemoteSessionPhase::Armed,
            RemoteSessionPhase::Live,
        ] {
            schedule.on_phase(phase, start_delay);
            assert!(!schedule.command_due(Duration::from_millis(399)));
            assert!(schedule.command_due(Duration::from_millis(400)));
        }

        schedule.on_phase(RemoteSessionPhase::Recovering, start_delay);
        assert!(!schedule.command_due(start_delay));
        schedule.on_phase(RemoteSessionPhase::Opening, start_delay);
        schedule.on_phase(RemoteSessionPhase::Armed, start_delay);
        assert!(!schedule.command_due(Duration::from_millis(199)));
        assert!(schedule.command_due(start_delay));
    }

    #[test]
    fn remote_command_schedule_rejects_stale_elapsed_after_send() {
        use crate::remote::RemoteSessionPhase;

        let start_delay = Duration::from_millis(200);
        let schedule = super::RemoteCommandSchedule::new(start_delay);
        let elapsed = Duration::from_millis(230);
        assert!(schedule.command_due(elapsed));
        schedule.mark_sent(elapsed, Duration::from_millis(200));
        schedule.on_phase(RemoteSessionPhase::Probing, start_delay);
        schedule.on_phase(RemoteSessionPhase::Armed, start_delay);

        // The session's send loop can reuse the pre-command elapsed value.
        assert!(!schedule.command_due(elapsed));
        assert!(!schedule.command_due(Duration::from_millis(429)));
        assert!(schedule.command_due(Duration::from_millis(430)));
        schedule.mark_sent(Duration::from_millis(600), Duration::from_millis(200));
        assert!(!schedule.command_due(Duration::from_millis(600)));
        assert!(!schedule.command_due(Duration::from_millis(799)));
        assert!(schedule.command_due(Duration::from_millis(800)));
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
            region.start_offset == 0x1DE0
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
