use anyhow::{Result, anyhow};
use byteorder::{BigEndian, ByteOrder, LittleEndian};
#[cfg(target_os = "macos")]
use std::fs;
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::path::Path;
use std::time::{Duration, Instant};

use crate::ble::{BleLink, parse_ble_device_uri};

use super::types::*;

pub const SETTINGS_MAGIC: u16 = 0xD82F;
pub const BAND_PLAN_RECORD_COUNT: usize = 20;
pub const BAND_PLAN_RECORD_SIZE: usize = 10;
pub const SCAN_PRESET_RECORD_COUNT: usize = 20;
pub const SCAN_PRESET_RECORD_SIZE: usize = 20;
pub const DTMF_PRESET_RECORD_COUNT: usize = 20;
pub const DTMF_PRESET_RECORD_SIZE: usize = 12;
pub const DTMF_PRESET_LABEL_SIZE: usize = 7;
pub const GROUP_LABEL_RECORD_COUNT: usize = GROUP_LABEL_COUNT;
pub const GROUP_LABEL_RECORD_SIZE: usize = GROUP_LABEL_SIZE;
const LIVE_TRANSACTION_MAGIC: [u8; 7] = [0x50, 0x56, 0x4F, 0x4A, 0x48, 0x5C, 0x14];
const LIVE_ACK: u8 = 0x06;
const LIVE_ID_REQ: u8 = 0x02;
const LIVE_READ_BLOCK: u8 = 0x52;
const LIVE_WRITE_BLOCK: u8 = 0x57;
const LIVE_END: u8 = 0x45;
const LIVE_ID_LEN: usize = 8;
const REMOTE_BYTE_TIMEOUT: Duration = Duration::from_millis(500);
const REMOTE_TEXT_MAX_BYTES: usize = 256;
const REMOTE_TEXT_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum RemotePacket {
    DisplayText {
        font_size: u8,
        x: u8,
        y: u8,
        fg_color: u16,
        bg_color: u16,
        text: String,
    },
    DrawRectangle {
        x: u8,
        y: u8,
        width: u8,
        height: u8,
        color: u16,
    },
    DrawSymbol {
        symbol_id: u8,
        x: u8,
        y: u8,
        fg_color: u16,
        bg_color: u16,
    },
    SignalStrength {
        strength: u8,
        mode: u8,
        battery: u8, // Legacy API field; not present in the wire packet.
    },
    NoiseLevel {
        level: u8,
        mode: u8,
    },
    SignalBarPos {
        y: u8,
        aux: u8,
    },
    SmallStatus {
        id: u8,
        value1: u8,
        value2: u8,
    },
    UnknownFrame {
        opcode: u8,
        payload: Vec<u8>,
    },
}

impl RemotePacket {
    pub fn summary(&self) -> String {
        match self {
            RemotePacket::DisplayText { x, y, text, .. } => {
                format!("TXT ({x:>3},{y:>3}) {}", truncate_remote_text(text, 20))
            }
            RemotePacket::DrawRectangle {
                x,
                y,
                width,
                height,
                ..
            } => format!("BOX ({x:>3},{y:>3}) {width:>3}x{height:<3}"),
            RemotePacket::DrawSymbol {
                symbol_id, x, y, ..
            } => {
                format!("SYM {symbol_id:02X} @ ({x:>3},{y:>3})")
            }
            RemotePacket::SignalStrength { strength, mode, .. } => {
                format!("RSSI {strength:>3} mode {mode:>2}")
            }
            RemotePacket::NoiseLevel { level, mode } => {
                format!("NOISE {level:>3} mode {mode:>2}")
            }
            RemotePacket::SignalBarPos { y, aux } => {
                format!("SBAR y {y:>3} aux {aux:>3}")
            }
            RemotePacket::SmallStatus { id, value1, value2 } => {
                format!("STS {id:02X}  {value1:02X} {value2:02X}")
            }
            RemotePacket::UnknownFrame { opcode, payload } => {
                if payload.is_empty() {
                    format!("UNK {opcode:02X} len=0")
                } else {
                    format!(
                        "UNK {opcode:02X} len={} {}",
                        payload.len(),
                        hex_bytes(payload)
                    )
                }
            }
        }
    }

    pub fn family_key(&self) -> String {
        match self {
            RemotePacket::DisplayText { x, y, text, .. } => {
                if *y >= 60 && looks_like_voltage_text(text) {
                    "battery_text".to_string()
                } else {
                    format!("text:{x}:{y}")
                }
            }
            RemotePacket::DrawRectangle {
                x,
                y,
                width,
                height,
                ..
            } => format!("box:{x}:{y}:{width}:{height}"),
            RemotePacket::DrawSymbol {
                symbol_id, x, y, ..
            } => {
                format!("sym:{symbol_id:02X}:{x}:{y}")
            }
            RemotePacket::SignalStrength { .. } => "signal_strength".to_string(),
            RemotePacket::NoiseLevel { .. } => "noise_level".to_string(),
            RemotePacket::SignalBarPos { .. } => "signal_bar".to_string(),
            RemotePacket::SmallStatus { id, .. } => format!("status:{id:02X}"),
            RemotePacket::UnknownFrame { opcode, .. } => format!("unknown:{opcode:02X}"),
        }
    }

    pub fn is_idle_telemetry(&self) -> bool {
        match self {
            RemotePacket::DisplayText { y, text, .. } => *y >= 60 && looks_like_voltage_text(text),
            RemotePacket::SmallStatus { id, value1, value2 } => {
                *id == 0x70 && *value1 == 0x00 && *value2 == 0x00
            }
            _ => false,
        }
    }

    pub fn detail_key(&self) -> String {
        match self {
            RemotePacket::DisplayText {
                font_size,
                x,
                y,
                fg_color,
                bg_color,
                text,
            } => format!("TXT:{font_size}:{x}:{y}:{fg_color:04X}:{bg_color:04X}:{text}"),
            RemotePacket::DrawRectangle {
                x,
                y,
                width,
                height,
                color,
            } => format!("BOX:{x}:{y}:{width}:{height}:{color:04X}"),
            RemotePacket::DrawSymbol {
                symbol_id,
                x,
                y,
                fg_color,
                bg_color,
            } => format!("SYM:{symbol_id:02X}:{x}:{y}:{fg_color:04X}:{bg_color:04X}"),
            RemotePacket::SignalStrength { strength, mode, .. } => {
                format!("RSSI:{strength}:{mode}")
            }
            RemotePacket::NoiseLevel { level, mode } => format!("NOISE:{level}:{mode}"),
            RemotePacket::SignalBarPos { y, aux } => format!("SBAR:{y}:{aux}"),
            RemotePacket::SmallStatus { id, value1, value2 } => {
                format!("STS:{id:02X}:{value1:02X}:{value2:02X}")
            }
            RemotePacket::UnknownFrame { opcode, payload } => {
                format!("UNK:{opcode:02X}:{}", hex_bytes(payload))
            }
        }
    }
}

fn truncate_remote_text(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        text.to_string()
    } else {
        let mut shortened = text
            .chars()
            .take(max_len.saturating_sub(3))
            .collect::<String>();
        shortened.push_str("...");
        shortened
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn additive_checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, &byte| acc.wrapping_add(byte))
}

fn looks_like_voltage_text(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.ends_with('V')
        && trimmed
            .strip_suffix('V')
            .is_some_and(|value| value.parse::<f32>().is_ok())
}

fn validate_live_address(address: u16) -> Result<()> {
    if address as usize >= EEPROM_SIZE {
        return Err(anyhow!(
            "Live-mode address 0x{address:04X} is outside the {}-byte EEPROM window",
            EEPROM_SIZE
        ));
    }
    if !(address as usize).is_multiple_of(BLOCK_SIZE) {
        return Err(anyhow!(
            "Live-mode address 0x{address:04X} must be aligned to {} bytes",
            BLOCK_SIZE
        ));
    }
    Ok(())
}

fn parse_live_read_response(address: u16, response: &[u8]) -> Result<Vec<u8>> {
    if response.len() != 4 + BLOCK_SIZE + 1 {
        return Err(anyhow!(
            "Live-mode read response length mismatch: got {}, expected {}",
            response.len(),
            4 + BLOCK_SIZE + 1
        ));
    }
    let addr_hi = (address >> 8) as u8;
    let addr_lo = address as u8;
    if response[0] != LIVE_WRITE_BLOCK
        || response[1] != addr_hi
        || response[2] != addr_lo
        || response[3] != BLOCK_SIZE as u8
    {
        return Err(anyhow!(
            "Live-mode read header mismatch for 0x{address:04X}: {:02X?}",
            &response[..4]
        ));
    }

    let data = &response[4..4 + BLOCK_SIZE];
    let checksum = response[4 + BLOCK_SIZE];
    let expected = additive_checksum(data);
    if checksum != expected {
        return Err(anyhow!(
            "Live-mode read checksum mismatch for 0x{address:04X}: got {:02X}, expected {:02X}",
            checksum,
            expected
        ));
    }
    Ok(data.to_vec())
}

enum PortHandle {
    Serial(Box<dyn serialport::SerialPort>),
    Ble(Box<BleLink>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReadTiming {
    #[default]
    Conservative,
    /// Opt-in serial reads with no per-block sleeps or clears. Each request still
    /// waits for a complete, checksum-validated response before the next request.
    Responsive,
}

pub struct RadioProtocol {
    port: PortHandle,
    pub log_callback: Option<Box<dyn Fn(String) + Send + Sync>>,
    read_timing: ReadTiming,
}

impl RadioProtocol {
    pub fn new(port_name: &str) -> Result<Self> {
        Self::new_with_baud(port_name, BAUD_RATE)
    }

    pub fn new_with_baud(port_name: &str, baud_rate: u32) -> Result<Self> {
        let port = if let Some(device_id) = parse_ble_device_uri(port_name) {
            let mut port = BleLink::connect(device_id)?;
            port.set_timeout(Duration::from_millis(50));
            PortHandle::Ble(Box::new(port))
        } else {
            let builder = serialport::new(port_name, effective_baud_rate(port_name, baud_rate))
                .timeout(Duration::from_millis(50));
            #[cfg(unix)]
            let port: Box<dyn serialport::SerialPort> = {
                let port = builder.open_native()?;
                #[cfg(target_os = "macos")]
                let port = {
                    let mut port = port;
                    let _ = port.set_exclusive(false);
                    port
                };
                Box::new(port)
            };
            #[cfg(not(unix))]
            let port = builder.open()?;
            let _ = port.clear(serialport::ClearBuffer::All);
            PortHandle::Serial(port)
        };
        Ok(Self {
            port,
            log_callback: None,
            read_timing: ReadTiming::Conservative,
        })
    }

    /// Select EEPROM read pacing; BLE always uses conservative timing.
    /// Responsive reads retain validation, timeouts and retry recovery. A failed
    /// response restores conservative timing until explicitly selected again.
    /// The caller manages radio disable/enable around transfers. Write timing and
    /// live-mode transactions are unaffected.
    pub fn set_read_timing(&mut self, timing: ReadTiming) {
        self.read_timing = if matches!(&self.port, PortHandle::Serial(_)) {
            timing
        } else {
            ReadTiming::Conservative
        };
    }

    fn log(&self, msg: String) {
        if let Some(ref cb) = self.log_callback {
            cb(msg);
        }
    }

    fn send(&mut self, data: &[u8]) -> Result<()> {
        self.log(format!("TX: {:02X?}", data));
        self.port_write_all(data)?;
        self.port_flush()?;
        Ok(())
    }

    pub fn send_bytes(&mut self, data: &[u8]) -> Result<()> {
        self.send(data)
    }

    pub fn read_byte(&mut self) -> Result<Option<u8>> {
        self.read_byte_with_logging(true)
    }

    fn read_byte_with_logging(&mut self, should_log: bool) -> Result<Option<u8>> {
        let mut buf = [0u8; 1];
        match self.port_read(&mut buf) {
            Ok(1) => {
                if should_log {
                    self.log(format!("RX: [{:02X}]", buf[0]));
                }
                Ok(Some(buf[0]))
            }
            Ok(_) => Ok(None),
            Err(ref e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn recv(&mut self, length: usize, timeout: Duration) -> Result<Vec<u8>> {
        self.recv_with_logging(length, timeout, true)
    }

    fn recv_with_logging(
        &mut self,
        length: usize,
        timeout: Duration,
        should_log: bool,
    ) -> Result<Vec<u8>> {
        self.recv_with_timeouts(length, timeout, None, should_log)
    }

    fn recv_with_timeouts(
        &mut self,
        length: usize,
        timeout: Duration,
        inactivity_timeout: Option<Duration>,
        should_log: bool,
    ) -> Result<Vec<u8>> {
        let buffer = Self::receive_frame(length, timeout, inactivity_timeout, |buffer| {
            self.port_read(buffer)
        })?;
        if should_log {
            self.log(format!("RX: {:02X?}", buffer));
        }
        Ok(buffer)
    }

    fn receive_frame(
        length: usize,
        timeout: Duration,
        inactivity_timeout: Option<Duration>,
        mut read: impl FnMut(&mut [u8]) -> std::io::Result<usize>,
    ) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; length];
        let start = Instant::now();
        let mut last_progress = start;
        let mut read_bytes = 0;

        while read_bytes < length
            && start.elapsed() < timeout
            && inactivity_timeout.is_none_or(|idle| last_progress.elapsed() < idle)
        {
            match read(&mut buffer[read_bytes..]) {
                Ok(n) => {
                    read_bytes += n;
                    if n > 0 {
                        last_progress = Instant::now();
                    }
                }
                Err(ref e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }

        if read_bytes < length {
            return Err(anyhow!(
                "Timeout waiting for response (got {}/{} bytes)",
                read_bytes,
                length
            ));
        }

        Ok(buffer)
    }

    fn port_write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match &mut self.port {
            PortHandle::Serial(port) => port.write_all(data),
            PortHandle::Ble(port) => port.write_all(data),
        }
    }

    fn port_flush(&mut self) -> std::io::Result<()> {
        match &mut self.port {
            PortHandle::Serial(port) => port.flush(),
            PortHandle::Ble(port) => port.flush(),
        }
    }

    fn port_read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.port {
            PortHandle::Serial(port) => port.read(buffer),
            PortHandle::Ble(port) => port.read(buffer),
        }
    }

    fn port_clear(&mut self, clear: serialport::ClearBuffer) -> std::io::Result<()> {
        match &mut self.port {
            PortHandle::Serial(port) => port
                .clear(clear)
                .map_err(|error| std::io::Error::other(error.to_string())),
            PortHandle::Ble(port) => port.clear(clear),
        }
    }

    pub fn ping(&mut self) -> Result<bool> {
        self.send(&[PKT_PING1])?;
        let resp = self.recv(1, Duration::from_millis(500))?;
        Ok(resp[0] == PKT_PING1)
    }

    pub fn handshake(&mut self) -> Result<bool> {
        for _ in 0..3 {
            if self.ping().unwrap_or(false) {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(false)
    }

    pub fn disable_radio(&mut self) -> Result<bool> {
        self.send(&[PKT_DISABLE])?;
        let resp = self.recv(1, Duration::from_millis(500))?;
        Ok(resp[0] == PKT_DISABLE)
    }

    pub fn enable_radio(&mut self) -> Result<bool> {
        self.send(&[PKT_ENABLE])?;
        let resp = self.recv(1, Duration::from_millis(500))?;
        Ok(resp[0] == PKT_ENABLE)
    }

    pub fn remote_on(&mut self) -> Result<bool> {
        self.send(&[PKT_REMOTE_ON])?;
        let resp = self.recv(1, Duration::from_millis(500))?;
        Ok(resp[0] == PKT_REMOTE_ON)
    }

    pub fn remote_off(&mut self) -> Result<bool> {
        self.send(&[PKT_REMOTE_OFF])?;
        let resp = self.recv(1, Duration::from_millis(500))?;
        Ok(resp[0] == PKT_REMOTE_OFF)
    }

    /// Send exactly one wire byte, without logical-key translation or release.
    pub fn send_key(&mut self, key_code: u8) -> Result<()> {
        self.send(&[key_code])?;
        Ok(())
    }

    pub fn press_remote_key(&mut self, key_code: u8) -> Result<()> {
        crate::remote::RemoteControlCommand::raw_key("key", key_code).execute(self)
    }

    pub fn reboot(&mut self) -> Result<()> {
        self.send(&[PKT_REBOOT])?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    }

    pub fn detect_endianness(&mut self) -> Result<Endianness> {
        let blk = self.read_block(240)?;
        if blk[0] == 0x57 {
            Ok(Endianness::Little)
        } else {
            Ok(Endianness::Big)
        }
    }

    pub fn detect_channel_endianness(&mut self) -> Result<Endianness> {
        let mut samples = Vec::with_capacity(8);
        for blk_num in 2..10 {
            samples.push(self.read_block(blk_num)?);
        }
        let (candidate, _) = Self::channel_endianness_from_samples(&samples)?;
        if let Some(candidate) = candidate {
            return Ok(candidate);
        }
        // An inconclusive first window does not establish an empty radio. VFOs
        // and later memories can provide evidence independent of settings order.
        for block in [0, 1].into_iter().chain(10..200) {
            samples.push(self.read_block(block)?);
        }
        let (candidate, all_empty) = Self::channel_endianness_from_samples(&samples)?;
        if let Some(candidate) = candidate {
            return Ok(candidate);
        }
        if all_empty {
            let settings = self.read_block((0x1900 / BLOCK_SIZE) as u8)?;
            if let Some(endian) = Self::recognized_settings_endianness(&settings) {
                return Ok(endian);
            }
        }
        Err(anyhow!(
            "Channel byte order is ambiguous: no consistent RX/TX evidence or recognized empty-radio settings marker"
        ))
    }

    pub(crate) fn channel_endianness_from_samples(
        samples: &[Vec<u8>],
    ) -> Result<(Option<Endianness>, bool)> {
        let mut candidate = None;
        let mut all_empty = true;
        for raw in samples {
            if raw.len() < 8 {
                return Err(anyhow!("Channel byte-order sample is truncated"));
            }
            if raw[..4].iter().all(|byte| *byte == 0) || raw[..4].iter().all(|byte| *byte == 0xFF) {
                continue;
            }
            all_empty = false;
            let valid = |endian| {
                let (rx, tx) = match endian {
                    Endianness::Little => (
                        LittleEndian::read_u32(&raw[..4]),
                        LittleEndian::read_u32(&raw[4..8]),
                    ),
                    Endianness::Big => (
                        BigEndian::read_u32(&raw[..4]),
                        BigEndian::read_u32(&raw[4..8]),
                    ),
                };
                (1_800_000..=130_000_000).contains(&rx)
                    && (matches!(tx, 0 | u32::MAX) || (1_800_000..=130_000_000).contains(&tx))
            };
            let next = match (valid(Endianness::Little), valid(Endianness::Big)) {
                (true, false) => Endianness::Little,
                (false, true) => Endianness::Big,
                _ => continue,
            };
            if candidate.is_some_and(|previous| previous != next) {
                return Err(anyhow!(
                    "Channel byte-order samples contain conflicting RX/TX evidence"
                ));
            }
            candidate = Some(next);
        }
        Ok((candidate, all_empty))
    }

    pub(crate) fn recognized_settings_endianness(raw: &[u8]) -> Option<Endianness> {
        let magic = raw.get(..2)?;
        if BigEndian::read_u16(magic) == SETTINGS_MAGIC {
            Some(Endianness::Big)
        } else if LittleEndian::read_u16(magic) == SETTINGS_MAGIC {
            Some(Endianness::Little)
        } else {
            None
        }
    }

    pub fn infer_settings_endianness(raw: &[u8]) -> Endianness {
        if raw.len() < 2 {
            return Endianness::Big;
        }

        let big = BigEndian::read_u16(&raw[0..2]);
        if big == SETTINGS_MAGIC {
            return Endianness::Big;
        }

        let little = LittleEndian::read_u16(&raw[0..2]);
        if little == SETTINGS_MAGIC {
            return Endianness::Little;
        }

        Endianness::Big
    }

    pub fn infer_bandplan_endianness(raw: &[u8]) -> Endianness {
        let score = |endian| {
            let mut valid = 0usize;
            for chunk in raw
                .as_chunks::<BAND_PLAN_RECORD_SIZE>()
                .0
                .iter()
                .take(BAND_PLAN_RECORD_COUNT)
            {
                let start = match endian {
                    Endianness::Little => LittleEndian::read_u32(&chunk[0..4]),
                    Endianness::Big => BigEndian::read_u32(&chunk[0..4]),
                };
                let end = match endian {
                    Endianness::Little => LittleEndian::read_u32(&chunk[4..8]),
                    Endianness::Big => BigEndian::read_u32(&chunk[4..8]),
                };
                if start == 0 && end == 0 {
                    continue;
                }
                if (1_000_000..=130_000_000).contains(&start)
                    && (1_000_000..=130_000_000).contains(&end)
                    && start < end
                {
                    valid += 1;
                }
            }
            valid
        };

        if score(Endianness::Big) >= score(Endianness::Little) {
            Endianness::Big
        } else {
            Endianness::Little
        }
    }

    /// Compatibility heuristic for display callers. Writes must use the checked
    /// variant rather than defaulting ambiguous or invalid tables to big endian.
    pub fn infer_scan_preset_endianness(raw: &[u8]) -> Endianness {
        Self::infer_scan_preset_endianness_checked(raw).unwrap_or(Endianness::Big)
    }

    /// Infer scan byte order only when all populated records support it.
    pub fn infer_scan_preset_endianness_checked(raw: &[u8]) -> Result<Endianness> {
        let table_len = SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE;
        if raw.len() < SCAN_PRESET_RECORD_SIZE
            || (raw.len() < table_len && !raw.len().is_multiple_of(SCAN_PRESET_RECORD_SIZE))
        {
            return Err(anyhow!("Scan preset byte-order table is truncated"));
        }

        let mut little_possible = true;
        let mut big_possible = true;
        let mut populated = false;
        for (index, chunk) in raw
            .as_chunks::<SCAN_PRESET_RECORD_SIZE>()
            .0
            .iter()
            .take(SCAN_PRESET_RECORD_COUNT)
            .enumerate()
        {
            if chunk[..4].iter().all(|byte| *byte == 0) || chunk.iter().all(|byte| *byte == 0xFF) {
                continue;
            }
            populated = true;
            let valid = |endian| {
                let start = match endian {
                    Endianness::Little => LittleEndian::read_u32(&chunk[0..4]),
                    Endianness::Big => BigEndian::read_u32(&chunk[0..4]),
                };
                let range = match endian {
                    Endianness::Little => LittleEndian::read_u16(&chunk[4..6]),
                    Endianness::Big => BigEndian::read_u16(&chunk[4..6]),
                };
                let step = match endian {
                    Endianness::Little => LittleEndian::read_u16(&chunk[6..8]),
                    Endianness::Big => BigEndian::read_u16(&chunk[6..8]),
                };
                // Presets.RefreshToData clamps these fields and SetName uses
                // Encoding.ASCII, including punctuation. Zero range/step can be
                // serialized by its truncation/clamping, so they are not evidence
                // against either byte order.
                (1..=130_000_000).contains(&start)
                    && range <= 60_000
                    && step <= 50_000
                    && chunk[8] <= 250
                    && chunk[9] <= 200
                    && chunk[10] & 3 <= 2
                    && chunk[10] >> 2 <= 20
                    && chunk[11..19].iter().all(u8::is_ascii)
            };
            let little_valid = valid(Endianness::Little);
            let big_valid = valid(Endianness::Big);
            if !little_valid && !big_valid {
                return Err(anyhow!(
                    "Scan preset {index} has no byte order consistent with the programmer schema"
                ));
            }
            little_possible &= little_valid;
            big_possible &= big_valid;
            if !little_possible && !big_possible {
                return Err(anyhow!(
                    "Scan preset byte-order records contain conflicting evidence"
                ));
            }
        }

        match (little_possible, big_possible) {
            (true, false) => Ok(Endianness::Little),
            (false, true) => Ok(Endianness::Big),
            _ => Err(anyhow!(
                "Scan preset byte order is ambiguous: {}",
                if populated {
                    "records are valid in both byte orders"
                } else {
                    "no populated records"
                }
            )),
        }
    }

    pub fn read_block(&mut self, block_num: u8) -> Result<Vec<u8>> {
        for attempt in 0..6 {
            if self.read_timing == ReadTiming::Conservative {
                let _ = self.port_clear(serialport::ClearBuffer::Input);
                std::thread::sleep(Duration::from_millis(15));
            }
            self.send(&[PKT_READ_EEPROM, block_num])?;

            match self.recv(1 + BLOCK_SIZE + 1, Duration::from_millis(1000)) {
                Ok(resp) if resp[0] == PKT_READ_EEPROM => {
                    let data = &resp[1..1 + BLOCK_SIZE];
                    let checksum = resp[1 + BLOCK_SIZE];
                    let calculated_sum: u8 = data.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
                    if calculated_sum == checksum {
                        if self.read_timing == ReadTiming::Conservative {
                            std::thread::sleep(Duration::from_millis(8));
                        }
                        return Ok(data.to_vec());
                    }

                    self.log(format!(
                        "Read block {} attempt {} checksum mismatch: got {:02X}, expected {:02X}",
                        block_num,
                        attempt + 1,
                        checksum,
                        calculated_sum
                    ));
                }
                Ok(resp) => {
                    self.log(format!(
                        "Read block {} attempt {} returned unexpected header {:02X}",
                        block_num,
                        attempt + 1,
                        resp[0]
                    ));
                }
                Err(error) => {
                    self.log(format!(
                        "Read block {} attempt {} failed: {}",
                        block_num,
                        attempt + 1,
                        error
                    ));
                }
            }

            if self.read_timing == ReadTiming::Responsive {
                self.read_timing = ReadTiming::Conservative;
                self.log("Responsive read failed; restoring conservative read timing".to_string());
            }
            let _ = self.port_clear(serialport::ClearBuffer::All);
            std::thread::sleep(Duration::from_millis(120 + attempt as u64 * 30));
        }
        Err(anyhow!("Failed to read block {} after retries", block_num))
    }

    pub fn live_mode_begin(&mut self) -> Result<[u8; LIVE_ID_LEN]> {
        self.send(&LIVE_TRANSACTION_MAGIC)?;
        let mut retried_after_remote_on = false;
        let mut ack = self.recv(1, Duration::from_millis(500))?;
        if ack[0] == PKT_REMOTE_ON {
            self.log(
                "Live mode begin received 4A REMOTE_ON; sending 45 recovery and retrying"
                    .to_string(),
            );
            retried_after_remote_on = true;
            self.send(&[LIVE_END])?;
            let _ = self.recv(1, Duration::from_millis(250));
            self.send(&LIVE_TRANSACTION_MAGIC)?;
            ack = self.recv(1, Duration::from_millis(500))?;
        }
        if ack[0] != LIVE_ACK {
            if ack[0] == PKT_REMOTE_ON && retried_after_remote_on {
                return Err(anyhow!(
                    "Live mode start ack mismatch: got 4A after recovery retry. This NicSure build appears to route the documented PVOJH opener into the remote-control parser (the opener contains byte 4A), so the public live-mode handshake is not available on this device/firmware."
                ));
            }
            return Err(anyhow!(
                "Live mode start ack mismatch: got {:02X}, expected {:02X}",
                ack[0],
                LIVE_ACK
            ));
        }

        self.send(&[LIVE_ID_REQ])?;
        let id = self.recv(LIVE_ID_LEN, Duration::from_millis(500))?;
        let id_bytes: [u8; LIVE_ID_LEN] = id
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Live mode ID response length mismatch"))?;

        self.send(&[LIVE_ACK])?;
        let final_ack = self.recv(1, Duration::from_millis(500))?;
        if final_ack[0] != LIVE_ACK {
            return Err(anyhow!(
                "Live mode ready ack mismatch: got {:02X}, expected {:02X}",
                final_ack[0],
                LIVE_ACK
            ));
        }

        Ok(id_bytes)
    }

    pub fn live_mode_end(&mut self) -> Result<[u8; LIVE_ID_LEN]> {
        self.send(&[LIVE_END])?;
        let ack = self.recv(1, Duration::from_millis(500))?;
        if ack[0] != LIVE_ACK {
            return Err(anyhow!(
                "Live mode end ack mismatch: got {:02X}, expected {:02X}",
                ack[0],
                LIVE_ACK
            ));
        }

        self.send(&[LIVE_ID_REQ])?;
        let id = self.recv(LIVE_ID_LEN, Duration::from_millis(500))?;
        let id_bytes: [u8; LIVE_ID_LEN] = id
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Live mode trailing ID response length mismatch"))?;
        Ok(id_bytes)
    }

    pub fn live_read_block(&mut self, address: u16) -> Result<Vec<u8>> {
        validate_live_address(address)?;
        let addr_hi = (address >> 8) as u8;
        let addr_lo = address as u8;
        self.send(&[LIVE_READ_BLOCK, addr_hi, addr_lo, BLOCK_SIZE as u8])?;
        let response = self.recv(4 + BLOCK_SIZE + 1, Duration::from_millis(1000))?;
        parse_live_read_response(address, &response)
    }

    pub fn live_write_block(&mut self, address: u16, data: &[u8]) -> Result<bool> {
        validate_live_address(address)?;
        if data.len() != BLOCK_SIZE {
            return Err(anyhow!(
                "Live-mode writes must be exactly {} bytes",
                BLOCK_SIZE
            ));
        }

        let addr_hi = (address >> 8) as u8;
        let addr_lo = address as u8;
        let mut packet = vec![LIVE_WRITE_BLOCK, addr_hi, addr_lo, BLOCK_SIZE as u8];
        packet.extend_from_slice(data);
        packet.push(additive_checksum(data));
        self.send(&packet)?;
        let ack = self.recv(1, Duration::from_millis(1000))?;
        Ok(ack[0] == LIVE_ACK)
    }

    pub fn write_block(&mut self, block_num: u8, data: &[u8]) -> Result<bool> {
        if data.len() != BLOCK_SIZE {
            return Err(anyhow!("Data must be exactly 32 bytes"));
        }
        let checksum: u8 = data.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
        let mut pkt = vec![PKT_WRITE_EEPROM, block_num];
        pkt.extend_from_slice(data);
        pkt.push(checksum);

        for attempt in 0..5 {
            let _ = self.port_clear(serialport::ClearBuffer::Input);
            std::thread::sleep(Duration::from_millis(15));
            self.send(&pkt)?;

            match self.recv(1, Duration::from_millis(1000)) {
                Ok(resp) if resp[0] == PKT_WRITE_EEPROM => {
                    std::thread::sleep(Duration::from_millis(12));
                    return Ok(true);
                }
                Ok(resp) => {
                    self.log(format!(
                        "Write block {} attempt {} returned unexpected ack {:02X}",
                        block_num,
                        attempt + 1,
                        resp[0]
                    ));
                }
                Err(error) => {
                    self.log(format!(
                        "Write block {} attempt {} failed: {}",
                        block_num,
                        attempt + 1,
                        error
                    ));
                }
            }

            let _ = self.port_clear(serialport::ClearBuffer::All);
            std::thread::sleep(Duration::from_millis(120 + attempt as u64 * 30));
        }
        Ok(false)
    }

    pub fn parse_channel(raw: &[u8], channel_num: u16, endian: Endianness) -> Option<Channel> {
        if raw.len() < BLOCK_SIZE {
            return None;
        }

        let rx_freq_raw = match endian {
            Endianness::Little => LittleEndian::read_u32(&raw[0..4]),
            Endianness::Big => BigEndian::read_u32(&raw[0..4]),
        };
        if rx_freq_raw == 0 || rx_freq_raw == 0xFFFFFFFF {
            return None;
        }

        // Validate frequency is in reasonable range (1 MHz to 1000 MHz for amateur radio)
        // Stored as integer: value * 100000 = Hz
        if !(100000..=1000000000).contains(&rx_freq_raw) {
            return None;
        }

        let tx_freq_raw = match endian {
            Endianness::Little => LittleEndian::read_u32(&raw[4..8]),
            Endianness::Big => BigEndian::read_u32(&raw[4..8]),
        };

        // Validate TX frequency is in reasonable range
        if tx_freq_raw != 0
            && tx_freq_raw != 0xFFFFFFFF
            && !(100000..=1000000000).contains(&tx_freq_raw)
        {
            return None;
        }

        let rx_tone_raw = match endian {
            Endianness::Little => LittleEndian::read_u16(&raw[8..10]),
            Endianness::Big => BigEndian::read_u16(&raw[8..10]),
        };
        let tx_tone_raw = match endian {
            Endianness::Little => LittleEndian::read_u16(&raw[10..12]),
            Endianness::Big => BigEndian::read_u16(&raw[10..12]),
        };
        let power = raw[12];
        let groups_raw = match endian {
            Endianness::Little => LittleEndian::read_u16(&raw[13..15]),
            Endianness::Big => BigEndian::read_u16(&raw[13..15]),
        };
        let bits = raw[15];

        // Some radios synthesize a placeholder record when a memory slot is cleared instead of
        // leaving the bytes erased. Treat that template as empty so readback matches user intent.
        if rx_freq_raw == 14_400_000
            && tx_freq_raw == 14_400_000
            && rx_tone_raw == 0xFFFF
            && tx_tone_raw == 0xFFFF
            && power == 0xFF
            && groups_raw == 0xFFFF
            && bits == 0xFF
        {
            return None;
        }

        let groups = [
            (groups_raw & 0x000F) as u8,
            ((groups_raw & 0x00F0) >> 4) as u8,
            ((groups_raw & 0x0F00) >> 8) as u8,
            ((groups_raw & 0xF000) >> 12) as u8,
        ];

        let name_bytes = &raw[20..32];
        let name = String::from_utf8_lossy(name_bytes)
            .trim_matches(char::from(0))
            .trim()
            .to_string();

        Some(Channel {
            channel_num,
            name,
            rx_freq: format!("{:.5}", rx_freq_raw as f64 / 100000.0),
            tx_freq: format!("{:.5}", tx_freq_raw as f64 / 100000.0),
            rx_tone: parse_tone(rx_tone_raw),
            tx_tone: parse_tone(tx_tone_raw),
            power,
            bandwidth: if bits & 0x01 != 0 {
                "Narrow".to_string()
            } else {
                "Wide".to_string()
            },
            modulation: match (bits >> 1) & 0x03 {
                0 => "Auto".to_string(),
                1 => "FM".to_string(),
                2 => "AM".to_string(),
                3 => "USB".to_string(),
                _ => "FM".to_string(),
            },
            position: (bits >> 3) & 0x01,
            ptt_id: (bits >> 4) & 0x03,
            reverse: bits & 0x40 != 0,
            busy_lock: bits & 0x80 != 0,
            groups,
        })
    }

    pub fn pack_channel(ch: &Channel, endian: Endianness) -> Vec<u8> {
        let mut raw = vec![0xFFu8; 32];
        let rx_freq_raw = (ch.rx_freq.parse::<f64>().unwrap_or(0.0) * 100000.0).round() as u32;
        let tx_freq_raw = (ch.tx_freq.parse::<f64>().unwrap_or(0.0) * 100000.0).round() as u32;

        let rx_tone_raw = pack_tone(&ch.rx_tone);
        let tx_tone_raw = pack_tone(&ch.tx_tone);

        match endian {
            Endianness::Little => {
                LittleEndian::write_u32(&mut raw[0..4], rx_freq_raw);
                LittleEndian::write_u32(&mut raw[4..8], tx_freq_raw);
                LittleEndian::write_u16(&mut raw[8..10], rx_tone_raw);
                LittleEndian::write_u16(&mut raw[10..12], tx_tone_raw);
            }
            Endianness::Big => {
                BigEndian::write_u32(&mut raw[0..4], rx_freq_raw);
                BigEndian::write_u32(&mut raw[4..8], tx_freq_raw);
                BigEndian::write_u16(&mut raw[8..10], rx_tone_raw);
                BigEndian::write_u16(&mut raw[10..12], tx_tone_raw);
            }
        }
        raw[12] = ch.power;

        let groups_raw = (ch.groups[0] as u16 & 0x0F)
            | ((ch.groups[1] as u16 & 0x0F) << 4)
            | ((ch.groups[2] as u16 & 0x0F) << 8)
            | ((ch.groups[3] as u16 & 0x0F) << 12);
        match endian {
            Endianness::Little => LittleEndian::write_u16(&mut raw[13..15], groups_raw),
            Endianness::Big => BigEndian::write_u16(&mut raw[13..15], groups_raw),
        }

        let mut bits = 0u8;
        if ch.bandwidth == "Narrow" {
            bits |= 0x01;
        }
        let mod_val = match ch.modulation.as_str() {
            "Auto" => 0,
            "FM" => 1,
            "AM" => 2,
            "USB" => 3,
            _ => 1,
        };
        bits |= (mod_val << 1) & 0x06;
        bits |= (ch.position & 0x01) << 3;
        bits |= (ch.ptt_id & 0x03) << 4;
        if ch.reverse {
            bits |= 0x40;
        }
        if ch.busy_lock {
            bits |= 0x80;
        }
        raw[15] = bits;
        raw[16..20].fill(0xFF);

        let name_bytes = ch.name.as_bytes();
        let len = name_bytes.len().min(12);
        raw[20..32].fill(0);
        if len < 12 {
            raw[20..31].fill(b' ');
            raw[31] = 0;
        }
        raw[20..20 + len].copy_from_slice(&name_bytes[..len]);

        raw
    }

    pub fn parse_settings_block(raw: &[u8], endian: Endianness) -> SettingsBlock {
        let mut vfo_state = [VfoState::default(), VfoState::default()];
        for (i, state) in vfo_state.iter_mut().enumerate() {
            let offset = 0x20 + (i * 19);
            // Check bounds before accessing to prevent panic with 64-byte codeplugs
            if offset + 18 < raw.len() {
                state.group = raw[offset];
                state.last_group = raw[offset + 1];
                state
                    .group_mode_channels
                    .copy_from_slice(&raw[offset + 2..offset + 18]);
                state.mode = raw[offset + 18];
            }
        }

        SettingsBlock {
            magic: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x00..0x02).unwrap_or(&[0; 2]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x00..0x02).unwrap_or(&[0; 2])),
            },
            squelch: raw.get(0x02).copied().unwrap_or(0),
            dual_watch: raw.get(0x03).copied().unwrap_or(0),
            auto_floor: raw.get(0x04).copied().unwrap_or(0),
            active_vfo: raw.get(0x05).copied().unwrap_or(0),
            step: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x06..0x08).unwrap_or(&[0; 2]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x06..0x08).unwrap_or(&[0; 2])),
            },
            rx_split: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x08..0x0A).unwrap_or(&[0; 2]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x08..0x0A).unwrap_or(&[0; 2])),
            },
            tx_split: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x0A..0x0C).unwrap_or(&[0; 2]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x0A..0x0C).unwrap_or(&[0; 2])),
            },
            ptt_mode: raw.get(0x0C).copied().unwrap_or(0),
            tx_mod_meter: raw.get(0x0D).copied().unwrap_or(0),
            mic_gain: raw.get(0x0E).copied().unwrap_or(0),
            tx_deviation: raw.get(0x0F).copied().unwrap_or(0),
            xtal671_defunct: raw.get(0x10).map(|&b| b as i8).unwrap_or(0),
            batt_style: raw.get(0x11).copied().unwrap_or(0),
            scan_range: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x12..0x14).unwrap_or(&[0; 2]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x12..0x14).unwrap_or(&[0; 2])),
            },
            scan_persist: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x14..0x16).unwrap_or(&[0; 2]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x14..0x16).unwrap_or(&[0; 2])),
            },
            scan_resume: raw.get(0x16).copied().unwrap_or(0),
            ultra_scan: raw.get(0x17).copied().unwrap_or(0),
            tone_monitor: raw.get(0x18).copied().unwrap_or(0),
            lcd_brightness: raw.get(0x19).copied().unwrap_or(0),
            lcd_timeout: raw.get(0x1A).copied().unwrap_or(0),
            breathe: raw.get(0x1B).copied().unwrap_or(0),
            dtmf_dev: raw.get(0x1C).copied().unwrap_or(0),
            gamma: raw.get(0x1D).copied().unwrap_or(0),
            repeater_tone: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x1E..0x20).unwrap_or(&[0; 2]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x1E..0x20).unwrap_or(&[0; 2])),
            },
            vfo_state,
            key_lock: raw.get(0x46).copied().unwrap_or(0),
            bluetooth: raw.get(0x47).copied().unwrap_or(0),
            power_save: raw.get(0x48).copied().unwrap_or(0),
            key_tones: raw.get(0x49).copied().unwrap_or(0),
            ste: raw.get(0x4A).copied().unwrap_or(0),
            rf_gain: raw.get(0x4B).copied().unwrap_or(0),
            s_bar_style: raw.get(0x4C).copied().unwrap_or(0),
            sq_noise_lev: raw.get(0x4D).copied().unwrap_or(0),
            last_fmt_freq: match endian {
                Endianness::Little => {
                    LittleEndian::read_u32(raw.get(0x4E..0x52).unwrap_or(&[0; 4]))
                }
                Endianness::Big => BigEndian::read_u32(raw.get(0x4E..0x52).unwrap_or(&[0; 4])),
            },
            vox: raw.get(0x52).copied().unwrap_or(0),
            vox_tail: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x53..0x55).unwrap_or(&[0, 0]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x53..0x55).unwrap_or(&[0, 0])),
            },
            tx_timeout: raw.get(0x55).copied().unwrap_or(0),
            dimmer: raw.get(0x56).copied().unwrap_or(0),
            dtmf_speed: raw.get(0x57).copied().unwrap_or(0),
            noise_gate: raw.get(0x58).copied().unwrap_or(0),
            scan_update: raw.get(0x59).copied().unwrap_or(0),
            asl: raw.get(0x5A).copied().unwrap_or(0),
            disable_fmt: raw.get(0x5B).copied().unwrap_or(0),
            pin: match endian {
                Endianness::Little => {
                    LittleEndian::read_u16(raw.get(0x5C..0x5E).unwrap_or(&[0, 0]))
                }
                Endianness::Big => BigEndian::read_u16(raw.get(0x5C..0x5E).unwrap_or(&[0, 0])),
            },
            pin_action: raw.get(0x5E).copied().unwrap_or(0),
            lcd_inverted: raw.get(0x5F).copied().unwrap_or(0),
            af_filters: raw.get(0x60).copied().unwrap_or(0),
            if_freq: raw.get(0x61).copied().unwrap_or(0),
            s_bar_always_on: raw.get(0x62).copied().unwrap_or(0),
            locked_vfo: raw.get(0x63).copied().unwrap_or(0),
            vfo_lock_active: raw.get(0x64).copied().unwrap_or(0),
            dual_watch_delay: raw.get(0x65).copied().unwrap_or(0),
            sub_tone_deviation: raw.get(0x66).copied().unwrap_or(0),
        }
    }

    pub fn pack_settings_block(s: &SettingsBlock, endian: Endianness) -> Vec<u8> {
        let mut raw = vec![0u8; 128]; // Settings block is 4 blocks of 32 bytes = 128 bytes

        match endian {
            Endianness::Little => LittleEndian::write_u16(&mut raw[0x00..0x02], s.magic),
            Endianness::Big => BigEndian::write_u16(&mut raw[0x00..0x02], s.magic),
        }

        raw[0x02] = s.squelch;
        raw[0x03] = s.dual_watch;
        raw[0x04] = s.auto_floor;
        raw[0x05] = s.active_vfo;

        match endian {
            Endianness::Little => {
                LittleEndian::write_u16(&mut raw[0x06..0x08], s.step);
                LittleEndian::write_u16(&mut raw[0x08..0x0A], s.rx_split);
                LittleEndian::write_u16(&mut raw[0x0A..0x0C], s.tx_split);
            }
            Endianness::Big => {
                BigEndian::write_u16(&mut raw[0x06..0x08], s.step);
                BigEndian::write_u16(&mut raw[0x08..0x0A], s.rx_split);
                BigEndian::write_u16(&mut raw[0x0A..0x0C], s.tx_split);
            }
        }

        raw[0x0C] = s.ptt_mode;
        raw[0x0D] = s.tx_mod_meter;
        raw[0x0E] = s.mic_gain;
        raw[0x0F] = s.tx_deviation;
        raw[0x10] = s.xtal671_defunct as u8;
        raw[0x11] = s.batt_style;

        match endian {
            Endianness::Little => {
                LittleEndian::write_u16(&mut raw[0x12..0x14], s.scan_range);
                LittleEndian::write_u16(&mut raw[0x14..0x16], s.scan_persist);
            }
            Endianness::Big => {
                BigEndian::write_u16(&mut raw[0x12..0x14], s.scan_range);
                BigEndian::write_u16(&mut raw[0x14..0x16], s.scan_persist);
            }
        }

        raw[0x16] = s.scan_resume;
        raw[0x17] = s.ultra_scan;
        raw[0x18] = s.tone_monitor;
        raw[0x19] = s.lcd_brightness;
        raw[0x1A] = s.lcd_timeout;
        raw[0x1B] = s.breathe;
        raw[0x1C] = s.dtmf_dev;
        raw[0x1D] = s.gamma;

        match endian {
            Endianness::Little => LittleEndian::write_u16(&mut raw[0x1E..0x20], s.repeater_tone),
            Endianness::Big => BigEndian::write_u16(&mut raw[0x1E..0x20], s.repeater_tone),
        }

        for i in 0..2 {
            let offset = 0x20 + (i * 19);
            raw[offset] = s.vfo_state[i].group;
            raw[offset + 1] = s.vfo_state[i].last_group;
            raw[offset + 2..offset + 18].copy_from_slice(&s.vfo_state[i].group_mode_channels);
            raw[offset + 18] = s.vfo_state[i].mode;
        }

        raw[0x46] = s.key_lock;
        raw[0x47] = s.bluetooth;
        raw[0x48] = s.power_save;
        raw[0x49] = s.key_tones;
        raw[0x4A] = s.ste;
        raw[0x4B] = s.rf_gain;
        raw[0x4C] = s.s_bar_style;
        raw[0x4D] = s.sq_noise_lev;

        match endian {
            Endianness::Little => LittleEndian::write_u32(&mut raw[0x4E..0x52], s.last_fmt_freq),
            Endianness::Big => BigEndian::write_u32(&mut raw[0x4E..0x52], s.last_fmt_freq),
        }

        raw[0x52] = s.vox;
        match endian {
            Endianness::Little => LittleEndian::write_u16(&mut raw[0x53..0x55], s.vox_tail),
            Endianness::Big => BigEndian::write_u16(&mut raw[0x53..0x55], s.vox_tail),
        }

        raw[0x55] = s.tx_timeout;
        raw[0x56] = s.dimmer;
        raw[0x57] = s.dtmf_speed;
        raw[0x58] = s.noise_gate;
        raw[0x59] = s.scan_update;
        raw[0x5A] = s.asl;
        raw[0x5B] = s.disable_fmt;

        match endian {
            Endianness::Little => LittleEndian::write_u16(&mut raw[0x5C..0x5E], s.pin),
            Endianness::Big => BigEndian::write_u16(&mut raw[0x5C..0x5E], s.pin),
        }

        raw[0x5E] = s.pin_action;
        raw[0x5F] = s.lcd_inverted;
        raw[0x60] = s.af_filters;
        raw[0x61] = s.if_freq;
        raw[0x62] = s.s_bar_always_on;
        raw[0x63] = s.locked_vfo;
        raw[0x64] = s.vfo_lock_active;
        raw[0x65] = s.dual_watch_delay;
        raw[0x66] = s.sub_tone_deviation;

        raw
    }

    pub fn parse_remote_packet(&mut self) -> Result<Option<RemotePacket>> {
        self.parse_remote_packet_with_options(24, Duration::from_millis(8))
    }

    pub fn parse_remote_packet_with_options(
        &mut self,
        _unknown_capture_limit: usize,
        _unknown_capture_gap: Duration,
    ) -> Result<Option<RemotePacket>> {
        let Some(id) = self.read_byte_with_logging(false)? else {
            return Ok(None);
        };

        // NOP packets are just ignored
        if id == 0x00 {
            return Ok(None);
        }

        let packet = Self::parse_remote_packet_inner(id, |length, timeout| {
            // The programmer resets its 500ms timeout on every received byte.
            // EEPROM and bootloader requests retain their fixed deadlines.
            self.recv_with_timeouts(length, REMOTE_TEXT_FRAME_TIMEOUT, Some(timeout), false)
        })?;
        if let Some(packet) = packet {
            return Ok(Some(packet));
        }

        // Unknown opcodes have no defined length. Consuming subsequent bytes
        // here can swallow the next valid display update.
        let packet = RemotePacket::UnknownFrame {
            opcode: id,
            payload: Vec::new(),
        };
        self.log(format!("REMOTE {}", packet.summary()));
        Ok(Some(packet))
    }

    fn parse_remote_packet_inner<F>(id: u8, mut recv: F) -> Result<Option<RemotePacket>>
    where
        F: FnMut(usize, Duration) -> Result<Vec<u8>>,
    {
        match id {
            0x64 => {
                // Display Text
                let deadline = Instant::now() + REMOTE_TEXT_FRAME_TIMEOUT;
                let header = recv(7, REMOTE_BYTE_TIMEOUT)?;
                let font_size = header[0];
                let x = header[1];
                let y = header[2];
                let fg_color = LittleEndian::read_u16(&header[3..5]);
                let bg_color = LittleEndian::read_u16(&header[5..7]);

                // Read null-terminated string
                let mut text_bytes = Vec::new();
                loop {
                    let remaining = deadline
                        .checked_duration_since(Instant::now())
                        .ok_or_else(|| anyhow!("Remote text frame exceeded its 5s deadline"))?;
                    let b = recv(1, remaining.min(REMOTE_BYTE_TIMEOUT))?[0];
                    if Instant::now() >= deadline {
                        return Err(anyhow!("Remote text frame exceeded its 5s deadline"));
                    }
                    if b == 0 {
                        break;
                    }
                    if text_bytes.len() >= REMOTE_TEXT_MAX_BYTES {
                        return Err(anyhow!(
                            "Remote text frame exceeds {} text bytes",
                            REMOTE_TEXT_MAX_BYTES
                        ));
                    }
                    text_bytes.push(if b == b'@' { b' ' } else { b });
                }
                Ok(Some(RemotePacket::DisplayText {
                    font_size,
                    x,
                    y,
                    fg_color,
                    bg_color,
                    text: text_bytes.into_iter().map(char::from).collect(),
                }))
            }
            0x65 => {
                // Draw Rectangle
                let data = recv(6, REMOTE_BYTE_TIMEOUT)?;
                let x = data[0];
                let y = data[1];
                let width = data[2];
                let height = data[3];
                let color = LittleEndian::read_u16(&data[4..6]);
                Ok(Some(RemotePacket::DrawRectangle {
                    x,
                    y,
                    width,
                    height,
                    color,
                }))
            }
            0x66 => {
                // Draw Symbol
                let data = recv(7, REMOTE_BYTE_TIMEOUT)?;
                let symbol_id = data[0];
                let x = data[1];
                let y = data[2];
                let fg_color = LittleEndian::read_u16(&data[3..5]);
                let bg_color = LittleEndian::read_u16(&data[5..7]);
                Ok(Some(RemotePacket::DrawSymbol {
                    symbol_id,
                    x,
                    y,
                    fg_color,
                    bg_color,
                }))
            }
            0x67 => {
                // The programmer consumes exactly two bytes here.
                let data = recv(2, REMOTE_BYTE_TIMEOUT)?;

                Ok(Some(RemotePacket::SignalStrength {
                    strength: data[0],
                    mode: data[1],
                    battery: 0,
                }))
            }
            0x68 => {
                let data = recv(2, REMOTE_BYTE_TIMEOUT)?;
                Ok(Some(RemotePacket::NoiseLevel {
                    level: data[0],
                    mode: data[1],
                }))
            }
            0x69 => {
                let data = recv(1, REMOTE_BYTE_TIMEOUT)?;
                Ok(Some(RemotePacket::SignalBarPos { y: data[0], aux: 0 }))
            }
            0x70..=0x7F => Ok(Some(RemotePacket::SmallStatus {
                id,
                value1: 0,
                value2: 0,
            })),
            _ => Ok(None),
        }
    }

    pub fn parse_bandplan(raw: &[u8], index: u8, endian: Endianness) -> BandPlan {
        let start_freq = match endian {
            Endianness::Little => LittleEndian::read_u32(&raw[0..4]),
            Endianness::Big => BigEndian::read_u32(&raw[0..4]),
        };
        let end_freq = match endian {
            Endianness::Little => LittleEndian::read_u32(&raw[4..8]),
            Endianness::Big => BigEndian::read_u32(&raw[4..8]),
        };
        let max_power = raw[8];
        let bits = raw[9];
        BandPlan {
            index,
            start_freq,
            end_freq,
            max_power,
            tx_allowed: bits & 1 != 0,
            wrap: bits & 2 != 0,
            bandwidth: (bits >> 5) & 7,
            modulation: (bits >> 2) & 7,
            raw_flags: bits,
        }
    }

    pub fn pack_bandplan(bp: &BandPlan, endian: Endianness) -> Vec<u8> {
        let mut raw = vec![0u8; 10];
        match endian {
            Endianness::Little => {
                LittleEndian::write_u32(&mut raw[0..4], bp.start_freq);
                LittleEndian::write_u32(&mut raw[4..8], bp.end_freq);
            }
            Endianness::Big => {
                BigEndian::write_u32(&mut raw[0..4], bp.start_freq);
                BigEndian::write_u32(&mut raw[4..8], bp.end_freq);
            }
        }
        raw[8] = bp.max_power;
        raw[9] = u8::from(bp.tx_allowed)
            | (u8::from(bp.wrap) << 1)
            | ((bp.modulation & 7) << 2)
            | ((bp.bandwidth & 7) << 5);
        raw
    }

    pub fn parse_scan_preset(raw: &[u8], index: u8, endian: Endianness) -> ScanPreset {
        let start_freq = match endian {
            Endianness::Little => LittleEndian::read_u32(&raw[0..4]),
            Endianness::Big => BigEndian::read_u32(&raw[0..4]),
        };
        let range = match endian {
            Endianness::Little => LittleEndian::read_u16(&raw[4..6]),
            Endianness::Big => BigEndian::read_u16(&raw[4..6]),
        };
        let step = match endian {
            Endianness::Little => LittleEndian::read_u16(&raw[6..8]),
            Endianness::Big => BigEndian::read_u16(&raw[6..8]),
        };
        let raw_mode = raw[10];
        let label = String::from_utf8_lossy(&raw[11..19])
            .trim_matches(char::from(0))
            .trim()
            .to_string();

        ScanPreset {
            index,
            start_freq,
            range,
            step,
            resume: raw[8],
            persist: raw[9],
            modulation: raw_mode & 3,
            ultrascan: raw_mode >> 2,
            label,
            raw_mode,
            raw_tail: raw[19],
        }
    }

    pub fn pack_scan_preset(sp: &ScanPreset, endian: Endianness) -> Vec<u8> {
        let mut raw = vec![0u8; SCAN_PRESET_RECORD_SIZE];
        match endian {
            Endianness::Little => {
                LittleEndian::write_u32(&mut raw[0..4], sp.start_freq);
                LittleEndian::write_u16(&mut raw[4..6], sp.range);
                LittleEndian::write_u16(&mut raw[6..8], sp.step);
            }
            Endianness::Big => {
                BigEndian::write_u32(&mut raw[0..4], sp.start_freq);
                BigEndian::write_u16(&mut raw[4..6], sp.range);
                BigEndian::write_u16(&mut raw[6..8], sp.step);
            }
        }
        raw[8] = sp.resume;
        raw[9] = sp.persist;
        raw[10] = ((sp.ultrascan & 0x3F) << 2) | (sp.modulation & 3);
        raw[11..19].fill(b' ');
        let label_bytes = sp.label.as_bytes();
        let label_len = label_bytes.len().min(8);
        raw[11..11 + label_len].copy_from_slice(&label_bytes[..label_len]);
        raw[19] = sp.raw_tail;
        raw
    }

    pub fn parse_dtmf_preset(raw: &[u8], index: u8) -> DTMFPreset {
        let length = match raw[0] & 0x0F {
            length @ 0..=9 => length as usize,
            _ => 0,
        };
        let mut digits = Vec::new();
        if length > 0 {
            digits.push((raw[0] >> 4) & 0x0F);
        }
        if length > 1 {
            digits.push(raw[1] & 0x0F);
        }
        if length > 2 {
            digits.push((raw[1] >> 4) & 0x0F);
        }
        if length > 3 {
            digits.push(raw[2] & 0x0F);
        }
        if length > 4 {
            digits.push((raw[2] >> 4) & 0x0F);
        }
        if length > 5 {
            digits.push(raw[3] & 0x0F);
        }
        if length > 6 {
            digits.push((raw[3] >> 4) & 0x0F);
        }
        if length > 7 {
            digits.push(raw[4] & 0x0F);
        }
        if length > 8 {
            digits.push((raw[4] >> 4) & 0x0F);
        }

        let label = String::from_utf8_lossy(&raw[5..DTMF_PRESET_RECORD_SIZE])
            .trim_matches(char::from(0))
            .trim()
            .to_string();

        DTMFPreset {
            index,
            digits,
            label,
        }
    }

    pub fn pack_dtmf_preset(dp: &DTMFPreset) -> Vec<u8> {
        let mut raw = vec![0u8; DTMF_PRESET_RECORD_SIZE];
        let len = dp.digits.len().min(9);
        raw[0] = (len as u8) & 0x0F;
        if len > 0 {
            raw[0] |= (dp.digits[0] & 0x0F) << 4;
        }
        if len > 1 {
            raw[1] |= dp.digits[1] & 0x0F;
        }
        if len > 2 {
            raw[1] |= (dp.digits[2] & 0x0F) << 4;
        }
        if len > 3 {
            raw[2] |= dp.digits[3] & 0x0F;
        }
        if len > 4 {
            raw[2] |= (dp.digits[4] & 0x0F) << 4;
        }
        if len > 5 {
            raw[3] |= dp.digits[5] & 0x0F;
        }
        if len > 6 {
            raw[3] |= (dp.digits[6] & 0x0F) << 4;
        }
        if len > 7 {
            raw[4] |= dp.digits[7] & 0x0F;
        }
        if len > 8 {
            raw[4] |= (dp.digits[8] & 0x0F) << 4;
        }

        let label_bytes = dp.label.as_bytes();
        let label_len = label_bytes.len().min(DTMF_PRESET_LABEL_SIZE);
        raw[5..5 + label_len].copy_from_slice(&label_bytes[..label_len]);
        raw
    }

    pub fn parse_group_labels(raw: &[u8]) -> Vec<String> {
        let mut labels = Vec::with_capacity(GROUP_LABEL_RECORD_COUNT);
        for index in 0..GROUP_LABEL_RECORD_COUNT {
            let start = index * GROUP_LABEL_RECORD_SIZE;
            let end = start + GROUP_LABEL_TEXT_SIZE;
            let label = raw
                .get(start..end)
                .map(|bytes| {
                    let bytes: Vec<u8> = bytes
                        .iter()
                        .map(|byte| if *byte == 0xFF { 0 } else { *byte })
                        .collect();
                    String::from_utf8_lossy(&bytes)
                        .replace('\0', " ")
                        .trim()
                        .to_string()
                })
                .unwrap_or_default();
            labels.push(label);
        }
        labels
    }

    pub fn pack_group_labels(labels: &[String]) -> Vec<u8> {
        let normalized = normalize_group_labels(labels);
        let mut raw = vec![0u8; GROUP_LABEL_RECORD_COUNT * GROUP_LABEL_RECORD_SIZE];

        for (index, label) in normalized.iter().enumerate() {
            let start = index * GROUP_LABEL_RECORD_SIZE;
            let end = start + GROUP_LABEL_RECORD_SIZE;
            let bytes = label.as_bytes();
            let len = bytes.len().min(GROUP_LABEL_TEXT_SIZE);
            raw[start..end].fill(0);
            raw[start..start + len].copy_from_slice(&bytes[..len]);
        }

        raw
    }
}

impl SettingsBlock {
    pub fn get_value(&self, index: usize) -> u32 {
        match index {
            0 => self.squelch as u32,
            1 => self.dual_watch as u32,
            2 => self.auto_floor as u32,
            3 => self.active_vfo as u32,
            4 => self.step as u32,
            5 => self.rx_split as u32,
            6 => self.tx_split as u32,
            7 => self.ptt_mode as u32,
            8 => self.tx_mod_meter as u32,
            9 => self.mic_gain as u32,
            10 => self.tx_deviation as u32,
            11 => self.batt_style as u32,
            12 => self.scan_range as u32,
            13 => self.scan_persist as u32,
            14 => self.scan_resume as u32,
            15 => self.ultra_scan as u32,
            16 => self.tone_monitor as u32,
            17 => self.lcd_brightness as u32,
            18 => self.lcd_timeout as u32,
            19 => self.breathe as u32,
            20 => self.dtmf_dev as u32,
            21 => self.gamma as u32,
            22 => self.repeater_tone as u32,
            23 => self.vfo_state[0].group as u32,
            24 => self.vfo_state[0].last_group as u32,
            25 => self.vfo_state[0].mode as u32,
            26 => self.vfo_state[1].group as u32,
            27 => self.vfo_state[1].last_group as u32,
            28 => self.vfo_state[1].mode as u32,
            29 => self.key_lock as u32,
            30 => self.bluetooth as u32,
            31 => self.power_save as u32,
            32 => self.key_tones as u32,
            33 => self.ste as u32,
            34 => self.rf_gain as u32,
            35 => self.s_bar_style as u32,
            36 => self.sq_noise_lev as u32,
            37 => self.last_fmt_freq,
            38 => self.vox as u32,
            39 => self.vox_tail as u32,
            40 => self.tx_timeout as u32,
            41 => self.dimmer as u32,
            42 => self.dtmf_speed as u32,
            43 => self.noise_gate as u32,
            44 => self.scan_update as u32,
            45 => self.asl as u32,
            46 => self.disable_fmt as u32,
            47 => self.pin as u32,
            48 => self.pin_action as u32,
            49 => self.lcd_inverted as u32,
            50 => self.af_filters as u32,
            51 => self.if_freq as u32,
            52 => self.s_bar_always_on as u32,
            53 => self.locked_vfo as u32,
            54 => self.vfo_lock_active as u32,
            55 => self.dual_watch_delay as u32,
            56 => self.sub_tone_deviation as u32,
            _ => 0,
        }
    }

    pub fn set_value(&mut self, index: usize, value: u32) {
        match index {
            0 => self.squelch = value as u8,
            1 => self.dual_watch = value as u8,
            2 => self.auto_floor = value as u8,
            3 => self.active_vfo = value as u8,
            4 => self.step = value as u16,
            5 => self.rx_split = value as u16,
            6 => self.tx_split = value as u16,
            7 => self.ptt_mode = value as u8,
            8 => self.tx_mod_meter = value as u8,
            9 => self.mic_gain = value as u8,
            10 => self.tx_deviation = value as u8,
            11 => self.batt_style = value as u8,
            12 => self.scan_range = value as u16,
            13 => self.scan_persist = value as u16,
            14 => self.scan_resume = value as u8,
            15 => self.ultra_scan = value as u8,
            16 => self.tone_monitor = value as u8,
            17 => self.lcd_brightness = value as u8,
            18 => self.lcd_timeout = value as u8,
            19 => self.breathe = value as u8,
            20 => self.dtmf_dev = value as u8,
            21 => self.gamma = value as u8,
            22 => self.repeater_tone = value as u16,
            23 => self.vfo_state[0].group = value as u8,
            24 => self.vfo_state[0].last_group = value as u8,
            25 => self.vfo_state[0].mode = value as u8,
            26 => self.vfo_state[1].group = value as u8,
            27 => self.vfo_state[1].last_group = value as u8,
            28 => self.vfo_state[1].mode = value as u8,
            29 => self.key_lock = value as u8,
            30 => self.bluetooth = value as u8,
            31 => self.power_save = value as u8,
            32 => self.key_tones = value as u8,
            33 => self.ste = value as u8,
            34 => self.rf_gain = value as u8,
            35 => self.s_bar_style = value as u8,
            36 => self.sq_noise_lev = value as u8,
            37 => self.last_fmt_freq = value,
            38 => self.vox = value as u8,
            39 => self.vox_tail = value as u16,
            40 => self.tx_timeout = value as u8,
            41 => self.dimmer = value as u8,
            42 => self.dtmf_speed = value as u8,
            43 => self.noise_gate = value as u8,
            44 => self.scan_update = value as u8,
            45 => self.asl = value as u8,
            46 => self.disable_fmt = value as u8,
            47 => self.pin = value as u16,
            48 => self.pin_action = value as u8,
            49 => self.lcd_inverted = value as u8,
            50 => self.af_filters = value as u8,
            51 => self.if_freq = value as u8,
            52 => self.s_bar_always_on = value as u8,
            53 => self.locked_vfo = value as u8,
            54 => self.vfo_lock_active = value as u8,
            55 => self.dual_watch_delay = value as u8,
            56 => self.sub_tone_deviation = value as u8,
            _ => {}
        }
    }

    pub fn get_display_value(&self, index: usize) -> String {
        super::setting_value::display_value(index, self.get_value(index))
    }
}

#[cfg(target_os = "macos")]
fn effective_baud_rate(port_name: &str, baud_rate: u32) -> u32 {
    if should_use_pty_safe_baud(port_name) {
        return 0;
    }

    baud_rate
}

#[cfg(not(target_os = "macos"))]
fn effective_baud_rate(_port_name: &str, baud_rate: u32) -> u32 {
    baud_rate
}

#[cfg(target_os = "macos")]
fn should_use_pty_safe_baud(port_name: &str) -> bool {
    if parse_ble_device_uri(port_name).is_some() {
        return false;
    }

    let path = Path::new(port_name);
    if port_name.starts_with("/dev/ttys") || port_name.starts_with("/dev/pts/") {
        return true;
    }

    fs::read_link(path)
        .ok()
        .and_then(|target| target.to_str().map(str::to_owned))
        .is_some_and(|target| target.starts_with("/dev/ttys") || target.starts_with("/dev/pts/"))
}

fn parse_tone(raw: u16) -> String {
    if raw == 0 {
        return "Off".to_string();
    }
    if raw & 0x8000 != 0 {
        let dcs = raw & 0x3FFF;
        let inverted = raw & 0x4000 != 0;
        return format!("D{:03o}{}", dcs, if inverted { "i" } else { "n" });
    }
    format!("{:.1}", raw as f64 / 10.0)
}

fn pack_tone(tone: &str) -> u16 {
    if tone.to_lowercase() == "off" {
        return 0;
    }
    if let Some(rest) = tone.strip_prefix(['D', 'd']) {
        let inverted = rest.ends_with(['i', 'I']);
        let dcs_str = rest.trim_end_matches(['i', 'n', 'I', 'N']);
        let dcs = u16::from_str_radix(dcs_str, 8).unwrap_or(0);
        let mut val = 0x8000 | (dcs & 0x3FFF);
        if inverted {
            val |= 0x4000;
        }
        return val;
    }
    (tone.parse::<f64>().unwrap_or(0.0) * 10.0) as u16
}

#[cfg(all(test, unix))]
mod read_timing_tests {
    use super::*;
    use serialport::SerialPort;
    use std::sync::mpsc::{self, Sender};
    use std::thread::{self, JoinHandle};

    #[derive(Clone, Copy)]
    enum Reply {
        Valid,
        BadHeader,
        BadChecksum,
        Truncated,
        Silent,
        WriteAck,
    }

    struct Fixture {
        protocol: RadioProtocol,
        shutdown: Sender<()>,
        worker: Option<JoinHandle<Vec<Vec<u8>>>>,
    }

    fn block_data(block: u8) -> Vec<u8> {
        (0..BLOCK_SIZE)
            .map(|offset| block.wrapping_mul(17).wrapping_add(offset as u8))
            .collect()
    }

    #[test]
    fn press_remote_key_uses_shared_mapping_release_and_pauses() {
        let cases = [
            (0x0B, 0x8A), // Menu must be 8A FF, not the logical ID followed by 00.
            (0x01, 0x80),
            (0x02, 0x81),
            (0x03, 0x82),
            (0x04, 0x83),
            (0x05, 0x84),
            (0x06, 0x85),
            (0x07, 0x86),
            (0x08, 0x87),
            (0x09, 0x88),
            (0x0A, 0x89),
            (0x0C, 0x8B),
            (0x0D, 0x8C),
            (0x0E, 0x8D),
            (0x0F, 0x8E),
            (0x10, 0x8F),
            (0x11, 0x91),
            (0x12, 0x92),
            (0x13, 0x90),
            (0x1A, 0x91),
            (0x8D, 0x8D),
        ];
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        let name = slave.name().unwrap();
        drop(slave);
        let mut protocol = RadioProtocol::new(&name).unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let (shutdown, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut raw_byte = [0];
            radio.read_exact(&mut raw_byte).unwrap();
            assert_eq!(raw_byte, [0x0B]); // send_key deliberately remains raw.
            for (_, wire) in cases {
                let mut press = [0];
                radio.read_exact(&mut press).unwrap();
                assert_eq!(press, [wire]);
                let mut release = [0];
                radio.read_exact(&mut release).unwrap();
                assert_eq!(release, [0xFF]);
            }
            let _ = stopped.recv_timeout(Duration::from_secs(3));
        });
        protocol.send_key(0x0B).unwrap();
        let started = Instant::now();
        for (logical, _) in cases {
            protocol.press_remote_key(logical).unwrap();
        }
        assert!(started.elapsed() >= Duration::from_millis(220 * cases.len() as u64));
        shutdown.send(()).unwrap();
        worker.join().unwrap();
    }

    fn detect_from_virtual_samples(samples: Vec<(u8, Vec<u8>)>) -> Result<Endianness> {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        let name = slave.name().unwrap();
        drop(slave);
        let mut protocol = RadioProtocol::new(&name).unwrap();
        protocol.set_read_timing(ReadTiming::Responsive);
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let (shutdown, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            for (block, data) in samples {
                let mut request = [0; 2];
                radio.read_exact(&mut request).unwrap();
                assert_eq!(request, [PKT_READ_EEPROM, block]);
                let mut response = vec![PKT_READ_EEPROM];
                response.extend_from_slice(&data);
                response.push(additive_checksum(&data));
                radio.write_all(&response).unwrap();
            }
            let _ = stopped.recv_timeout(Duration::from_secs(3));
        });
        let result = protocol.detect_channel_endianness();
        shutdown.send(()).unwrap();
        worker.join().unwrap();
        result
    }

    fn sample_frequency(endian: Endianness) -> Vec<u8> {
        let mut data = vec![0; BLOCK_SIZE];
        let bytes = match endian {
            Endianness::Big => 14_535_000u32.to_be_bytes(),
            Endianness::Little => 14_535_000u32.to_le_bytes(),
        };
        data[..4].copy_from_slice(&bytes);
        data[4..8].copy_from_slice(&bytes);
        data
    }

    #[test]
    fn channel_detection_finds_late_records_before_using_settings_order() {
        for endian in [Endianness::Big, Endianness::Little] {
            let samples = (2..10)
                .chain([0, 1])
                .chain(10..200)
                .map(|block| {
                    (
                        block,
                        if block == 199 {
                            sample_frequency(endian)
                        } else {
                            vec![0xFF; BLOCK_SIZE]
                        },
                    )
                })
                .collect();
            // A settings-block request is not part of the fixture: the final
            // channel, not settings metadata, must decide channel byte order.
            assert_eq!(detect_from_virtual_samples(samples).unwrap(), endian);
        }
    }

    #[test]
    fn channel_detection_checks_late_conflicts_but_keeps_the_normal_read_budget() {
        let normal = (2..10)
            .map(|block| (block, sample_frequency(Endianness::Big)))
            .collect();
        assert_eq!(
            detect_from_virtual_samples(normal).unwrap(),
            Endianness::Big
        );
        let conflicting = (2..10)
            .chain([0, 1])
            .chain(10..200)
            .map(|block| {
                let data = match block {
                    100 => sample_frequency(Endianness::Big),
                    199 => sample_frequency(Endianness::Little),
                    _ => vec![0xFF; BLOCK_SIZE],
                };
                (block, data)
            })
            .collect();
        assert!(
            detect_from_virtual_samples(conflicting)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
    }

    impl Fixture {
        fn new(
            timing: ReadTiming,
            replies: Vec<Reply>,
            chunk_size: usize,
            delay: Duration,
        ) -> Self {
            let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
            let name = slave.name().unwrap();
            drop(slave);
            let mut protocol = RadioProtocol::new(&name).unwrap();
            assert_eq!(protocol.read_timing, ReadTiming::Conservative);
            protocol.set_read_timing(timing);
            radio.set_timeout(Duration::from_secs(3)).unwrap();
            let (shutdown, stopped) = mpsc::channel();
            let worker = thread::spawn(move || {
                let mut requests = Vec::new();
                for reply in replies {
                    let mut command = [0; 2];
                    radio.read_exact(&mut command).unwrap();
                    let mut request = command.to_vec();
                    if matches!(reply, Reply::WriteAck) {
                        assert_eq!(command[0], PKT_WRITE_EEPROM);
                        let mut body = [0; BLOCK_SIZE + 1];
                        radio.read_exact(&mut body).unwrap();
                        assert_eq!(body[BLOCK_SIZE], additive_checksum(&body[..BLOCK_SIZE]));
                        request.extend_from_slice(&body);
                        radio.write_all(&[PKT_WRITE_EEPROM]).unwrap();
                    } else {
                        assert_eq!(command[0], PKT_READ_EEPROM);
                        let data = block_data(command[1]);
                        let mut response = vec![PKT_READ_EEPROM];
                        response.extend_from_slice(&data);
                        response.push(additive_checksum(&data));
                        match reply {
                            Reply::BadHeader => response[0] ^= 1,
                            Reply::BadChecksum => response[BLOCK_SIZE + 1] ^= 1,
                            Reply::Truncated => response.truncate(7),
                            Reply::Silent => response.clear(),
                            _ => {}
                        }
                        for chunk in response.chunks(chunk_size) {
                            thread::sleep(delay);
                            radio.write_all(chunk).unwrap();
                        }
                    }
                    requests.push(request);
                }
                // Keep the master open until the final response has been consumed.
                let _ = stopped.recv_timeout(Duration::from_secs(5));
                requests
            });
            Self {
                protocol,
                shutdown,
                worker: Some(worker),
            }
        }

        fn finish(&mut self) -> Vec<Vec<u8>> {
            self.shutdown.send(()).unwrap();
            self.worker.take().unwrap().join().unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.shutdown.send(());
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[test]
    fn default_conservative_reads_keep_success_delays() {
        assert_eq!(ReadTiming::default(), ReadTiming::Conservative);
        let mut fixture = Fixture::new(
            ReadTiming::Conservative,
            vec![Reply::Valid; 2],
            34,
            Duration::ZERO,
        );
        let start = Instant::now();
        for block in [0, 255] {
            assert_eq!(
                fixture.protocol.read_block(block).unwrap(),
                block_data(block)
            );
        }
        assert!(start.elapsed() >= Duration::from_millis(46));
        assert_eq!(
            fixture.finish(),
            [[PKT_READ_EEPROM, 0], [PKT_READ_EEPROM, 255]]
        );
    }

    #[test]
    fn responsive_reads_check_fragmented_responses_for_all_blocks() {
        let mut fixture = Fixture::new(
            ReadTiming::Responsive,
            vec![Reply::Valid; TOTAL_BLOCKS],
            3,
            Duration::ZERO,
        );
        for block in 0..TOTAL_BLOCKS {
            assert_eq!(
                fixture.protocol.read_block(block as u8).unwrap(),
                block_data(block as u8)
            );
        }
        assert_eq!(fixture.protocol.read_timing, ReadTiming::Responsive);
        let requests = fixture.finish();
        assert_eq!(requests.len(), TOTAL_BLOCKS);
        for (block, request) in requests.iter().enumerate() {
            assert_eq!(request, &[PKT_READ_EEPROM, block as u8]);
        }
    }

    #[test]
    fn responsive_reads_tolerate_gaps_longer_than_port_timeout() {
        let mut fixture = Fixture::new(
            ReadTiming::Responsive,
            vec![Reply::Valid],
            11,
            Duration::from_millis(65),
        );
        assert_eq!(fixture.protocol.read_block(240).unwrap(), block_data(240));
        assert_eq!(fixture.protocol.read_timing, ReadTiming::Responsive);
        assert_eq!(fixture.finish(), [[PKT_READ_EEPROM, 240]]);
    }

    #[test]
    fn failed_responses_retry_same_block_and_latch_conservative_timing() {
        for failure in [
            Reply::BadChecksum,
            Reply::BadHeader,
            Reply::Truncated,
            Reply::Silent,
        ] {
            let mut fixture = Fixture::new(
                ReadTiming::Responsive,
                vec![failure, Reply::Valid, Reply::Valid],
                7,
                Duration::from_millis(1),
            );
            let start = Instant::now();
            assert_eq!(fixture.protocol.read_block(42).unwrap(), block_data(42));
            // Preserve both the 1 s receive budget and conservative retry pacing.
            let minimum_ms = if matches!(failure, Reply::Truncated | Reply::Silent) {
                1143
            } else {
                143
            };
            assert!(start.elapsed() >= Duration::from_millis(minimum_ms));
            assert_eq!(fixture.protocol.read_timing, ReadTiming::Conservative);
            assert_eq!(fixture.protocol.read_block(43).unwrap(), block_data(43));
            assert_eq!(fixture.protocol.read_timing, ReadTiming::Conservative);
            assert_eq!(
                fixture.finish(),
                [
                    [PKT_READ_EEPROM, 42],
                    [PKT_READ_EEPROM, 42],
                    [PKT_READ_EEPROM, 43]
                ]
            );
        }
    }

    #[test]
    fn responsive_reads_preserve_six_attempt_limit() {
        let mut fixture = Fixture::new(
            ReadTiming::Responsive,
            vec![Reply::BadChecksum; 6],
            34,
            Duration::ZERO,
        );
        let error = fixture.protocol.read_block(7).unwrap_err().to_string();
        assert_eq!(error, "Failed to read block 7 after retries");
        assert_eq!(fixture.protocol.read_timing, ReadTiming::Conservative);
        assert_eq!(fixture.finish(), vec![vec![PKT_READ_EEPROM, 7]; 6]);
    }

    #[test]
    fn read_timing_can_be_reset_and_does_not_accelerate_writes() {
        let mut fixture = Fixture::new(
            ReadTiming::Responsive,
            vec![Reply::WriteAck, Reply::Valid],
            34,
            Duration::ZERO,
        );
        let data = block_data(12);
        let start = Instant::now();
        assert!(fixture.protocol.write_block(12, &data).unwrap());
        assert!(start.elapsed() >= Duration::from_millis(27));
        assert_eq!(fixture.protocol.read_timing, ReadTiming::Responsive);
        fixture.protocol.set_read_timing(ReadTiming::Conservative);
        assert_eq!(fixture.protocol.read_block(12).unwrap(), data);
        let mut write = vec![PKT_WRITE_EEPROM, 12];
        write.extend_from_slice(&data);
        write.push(additive_checksum(&data));
        assert_eq!(fixture.finish(), [write, vec![PKT_READ_EEPROM, 12]]);
    }

    /// Run with: cargo test --lib benchmark_read_timing -- --ignored --nocapture
    #[test]
    #[ignore = "wall-clock PTY benchmark; not a hardware throughput measurement"]
    fn benchmark_read_timing() {
        for (profile, delay) in [
            ("immediate", Duration::ZERO),
            // Two request bytes + 34 response bytes, 8N1 at 38400 baud.
            ("38400-baud-equivalent", Duration::from_micros(9_375)),
        ] {
            for timing in [ReadTiming::Conservative, ReadTiming::Responsive] {
                let mut fixture = Fixture::new(timing, vec![Reply::Valid; TOTAL_BLOCKS], 34, delay);
                let start = Instant::now();
                for block in 0..TOTAL_BLOCKS {
                    assert_eq!(
                        fixture.protocol.read_block(block as u8).unwrap(),
                        block_data(block as u8)
                    );
                }
                let elapsed = start.elapsed();
                assert_eq!(fixture.finish().len(), TOTAL_BLOCKS);
                println!(
                    "{profile} {timing:?}: {TOTAL_BLOCKS} blocks, {:.3} s, {:.3} ms/block",
                    elapsed.as_secs_f64(),
                    elapsed.as_secs_f64() * 1000.0 / TOTAL_BLOCKS as f64
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn interrupted_receive_preserves_partial_frame_until_completion() {
        let mut calls = 0;
        let frame = RadioProtocol::receive_frame(3, Duration::from_secs(1), None, |buffer| {
            calls += 1;
            match calls {
                1 | 3 => Err(std::io::ErrorKind::Interrupted.into()),
                2 => {
                    buffer[0] = 0x30;
                    Ok(1)
                }
                4 => {
                    buffer.copy_from_slice(&[0x12, 0x34]);
                    Ok(2)
                }
                _ => panic!("unexpected extra read"),
            }
        })
        .unwrap();
        assert_eq!(frame, [0x30, 0x12, 0x34]);
        assert_eq!(calls, 4);
    }

    #[test]
    fn interrupted_receive_does_not_reset_overall_or_idle_deadlines() {
        for (timeout, idle) in [
            (Duration::from_millis(10), None),
            (Duration::from_secs(2), Some(Duration::from_millis(10))),
        ] {
            let started = Instant::now();
            let mut first = true;
            let error = RadioProtocol::receive_frame(2, timeout, idle, |buffer| {
                if first {
                    first = false;
                    buffer[0] = 0x46;
                    return Ok(1);
                }
                std::thread::sleep(Duration::from_millis(1));
                Err(std::io::ErrorKind::Interrupted.into())
            })
            .unwrap_err();
            assert!(error.to_string().contains("got 1/2 bytes"));
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "EINTR extended the deadline"
            );
        }
    }

    fn recv_from_queue(
        queue: &mut VecDeque<u8>,
        length: usize,
        _timeout: Duration,
    ) -> Result<Vec<u8>> {
        if queue.len() < length {
            return Err(anyhow!(
                "Timeout waiting for response (got {}/{} bytes)",
                queue.len(),
                length
            ));
        }

        Ok((0..length)
            .map(|_| queue.pop_front().expect("queue length checked"))
            .collect())
    }

    #[test]
    fn remote_text_without_a_terminator_fails_at_the_byte_budget() {
        let mut reads = 0;
        let error = RadioProtocol::parse_remote_packet_inner(0x64, |length, _| {
            reads += 1;
            assert!(reads <= REMOTE_TEXT_MAX_BYTES + 2);
            Ok(vec![b'X'; length])
        })
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 256 text bytes"));
        assert_eq!(reads, REMOTE_TEXT_MAX_BYTES + 2);
    }

    #[test]
    fn remote_text_accepts_the_byte_budget_only_with_a_terminator() {
        let mut queue = VecDeque::from(vec![0; 7]);
        queue.extend(vec![b'X'; REMOTE_TEXT_MAX_BYTES]);
        queue.extend([0, 0x67]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x64, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap()
        .unwrap();
        assert!(
            matches!(packet, RemotePacket::DisplayText { text, .. } if text.len() == REMOTE_TEXT_MAX_BYTES)
        );
        assert_eq!(queue, VecDeque::from([0x67]));
    }

    #[test]
    fn remote_text_deadline_includes_header_time() {
        let mut reads = 0;
        let error = RadioProtocol::parse_remote_packet_inner(0x64, |length, _| {
            reads += 1;
            assert_eq!(reads, 1);
            std::thread::sleep(REMOTE_TEXT_FRAME_TIMEOUT + Duration::from_millis(10));
            Ok(vec![0; length])
        })
        .unwrap_err();
        assert!(error.to_string().contains("5s deadline"));
    }

    fn scan_byte_order_fixture() -> Vec<u8> {
        let mut table = vec![0; SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE];
        table[..20].copy_from_slice(&[
            0x58, 0xC9, 0xDD, 0x00, 0x6C, 0x07, 0xE2, 0x04, 0, 0, 0, b'2', b'M', b'-', b'S', b'C',
            b'A', b'N', b' ', 0,
        ]);
        table
    }

    #[test]
    fn scan_byte_order_accepts_exact_hyphenated_programmer_fixture() {
        let table = scan_byte_order_fixture();
        assert_eq!(
            RadioProtocol::infer_scan_preset_endianness(&table),
            Endianness::Little
        );
        let endian = RadioProtocol::infer_scan_preset_endianness_checked(&table).unwrap();
        assert_eq!(endian, Endianness::Little);
        let preset = RadioProtocol::parse_scan_preset(&table[..20], 0, endian);
        assert_eq!(preset.start_freq, 14_535_000);
        assert_eq!(preset.range, 1_900);
        assert_eq!(preset.step, 1_250);
        assert_eq!(preset.label, "2M-SCAN");
        assert_eq!(
            RadioProtocol::pack_scan_preset(&preset, endian),
            table[..20]
        );
    }

    #[test]
    fn scan_byte_order_uses_programmer_schema_bounds_in_both_orders() {
        let mut table = scan_byte_order_fixture();
        table[11..19].copy_from_slice(b"!-/+()?#");
        table[8] = 250;
        table[9] = 200;
        table[10] = (20 << 2) | 2;
        for (range, step) in [(60_000, 50_000), (1, 1), (0, 0)] {
            let mut preset = RadioProtocol::parse_scan_preset(&table[..20], 0, Endianness::Little);
            preset.range = range;
            preset.step = step;
            for endian in [Endianness::Little, Endianness::Big] {
                table[..20].copy_from_slice(&RadioProtocol::pack_scan_preset(&preset, endian));
                assert_eq!(
                    RadioProtocol::infer_scan_preset_endianness_checked(&table).unwrap(),
                    endian
                );
            }
            // Restore the little-endian fixture for the next parse.
            table[..20].copy_from_slice(&RadioProtocol::pack_scan_preset(
                &preset,
                Endianness::Little,
            ));
        }
        // A block-aligned read includes 16 bytes of the following group table.
        table.extend([0xFF; 16]);
        assert_eq!(
            RadioProtocol::infer_scan_preset_endianness_checked(&table).unwrap(),
            Endianness::Little
        );
        table.truncate(SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE);
        let preset = RadioProtocol::parse_scan_preset(&table[..20], 0, Endianness::Little);
        table.extend(RadioProtocol::pack_scan_preset(&preset, Endianness::Big));
        table.extend([0xFF; SCAN_PRESET_RECORD_SIZE - 1]);
        assert_eq!(
            RadioProtocol::infer_scan_preset_endianness_checked(&table).unwrap(),
            Endianness::Little
        );
    }

    #[test]
    fn checked_scan_byte_order_rejects_empty_ambiguous_and_conflicting_tables() {
        for table in [
            vec![],
            vec![0; 19],
            vec![0; 21],
            vec![0; 400],
            vec![0xFF; 400],
        ] {
            assert!(RadioProtocol::infer_scan_preset_endianness_checked(&table).is_err());
        }
        let mut ambiguous = vec![0; 20];
        ambiguous[..8].copy_from_slice(&[0, 1, 2, 0, 1, 2, 1, 2]);
        assert!(
            RadioProtocol::infer_scan_preset_endianness_checked(&ambiguous)
                .unwrap_err()
                .to_string()
                .contains("both byte orders")
        );
        let mut table = scan_byte_order_fixture();
        let preset = RadioProtocol::parse_scan_preset(&table[..20], 0, Endianness::Little);
        let opposite = RadioProtocol::pack_scan_preset(&preset, Endianness::Big);
        // A majority must not overrule a conflicting populated record.
        let first = table[..20].to_vec();
        table[20..40].copy_from_slice(&first);
        table[40..60].copy_from_slice(&opposite);
        assert!(
            RadioProtocol::infer_scan_preset_endianness_checked(&table)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
    }

    #[test]
    fn checked_scan_byte_order_rejects_out_of_schema_fields() {
        for (offset, bytes) in [
            (0, 130_000_001u32.to_le_bytes().to_vec()),
            (4, 60_001u16.to_le_bytes().to_vec()),
            (6, 50_001u16.to_le_bytes().to_vec()),
            (8, vec![251]),
            (9, vec![201]),
            (10, vec![3]),
            (10, vec![21 << 2]),
            (11, vec![0x80]),
        ] {
            let mut table = scan_byte_order_fixture();
            table[offset..offset + bytes.len()].copy_from_slice(&bytes);
            assert!(
                RadioProtocol::infer_scan_preset_endianness_checked(&table).is_err(),
                "accepted invalid field at offset {offset}"
            );
        }
    }

    #[test]
    fn parses_small_status_packets_without_payload() {
        let mut queue = VecDeque::from([0x12, 0x34]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x72, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::SmallStatus { id, value1, value2 }) => {
                assert_eq!(id, 0x72);
                assert_eq!(value1, 0x00);
                assert_eq!(value2, 0x00);
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert_eq!(queue, VecDeque::from([0x12, 0x34]));
    }

    #[test]
    fn parses_signal_bar_packet_without_overreading() {
        let mut queue = VecDeque::from([0x55, 0xAA]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x69, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::SignalBarPos { y, aux }) => {
                assert_eq!(y, 0x55);
                assert_eq!(aux, 0x00);
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert_eq!(queue, VecDeque::from([0xAA]));
    }

    #[test]
    fn parses_noise_packet_without_padding() {
        let mut queue = VecDeque::from([0x09, 0x02]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x68, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::NoiseLevel { level, mode }) => {
                assert_eq!(level, 0x09);
                assert_eq!(mode, 0x02);
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert!(queue.is_empty());
    }

    #[test]
    fn remote_text_matches_programmer_byte_mapping() {
        let mut queue = VecDeque::from([0, 4, 8, 0x1F, 0, 0, 0, b'@', 0xE9, 0]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x64, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();
        match packet {
            Some(RemotePacket::DisplayText { text, fg_color, .. }) => {
                assert_eq!(text, " \u{e9}");
                assert_eq!(fg_color, 0x001F);
            }
            other => panic!("unexpected packet: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn unknown_remote_opcode_keeps_next_drawing_command_intact() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        let mut protocol = RadioProtocol {
            port: PortHandle::Serial(Box::new(slave)),
            log_callback: None,
            read_timing: ReadTiming::Conservative,
        };
        radio
            .write_all(&[0xE8, 0x64, 0, 4, 8, 0x1F, 0, 0, 0, b'A', 0])
            .unwrap();
        assert!(matches!(protocol.parse_remote_packet().unwrap(),
            Some(RemotePacket::UnknownFrame { opcode: 0xE8, payload }) if payload.is_empty()));
        assert!(matches!(protocol.parse_remote_packet().unwrap(),
            Some(RemotePacket::DisplayText { text, .. }) if text == "A"));
    }

    #[test]
    fn parses_display_text_without_consuming_padding_bytes() {
        let mut queue =
            VecDeque::from([0x01, 0x02, 0x03, 0x34, 0x12, 0x78, 0x56, b'A', 0x00, 0xEE]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x64, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::DisplayText { text, .. }) => assert_eq!(text, "A"),
            other => panic!("unexpected packet: {other:?}"),
        }

        assert_eq!(queue, VecDeque::from([0xEE]));
    }

    #[test]
    fn parses_rectangle_without_consuming_extra_padding_bytes() {
        let mut queue = VecDeque::from([0x01, 0x02, 0x03, 0x04, 0x34, 0x12, 0xEE]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x65, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::DrawRectangle {
                x,
                y,
                width,
                height,
                color,
            }) => {
                assert_eq!(
                    (x, y, width, height, color),
                    (0x01, 0x02, 0x03, 0x04, 0x1234)
                );
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert_eq!(queue, VecDeque::from([0xEE]));
    }

    #[test]
    fn parses_symbol_without_consuming_extra_padding_bytes() {
        let mut queue = VecDeque::from([0x05, 0x06, 0x07, 0x34, 0x12, 0x78, 0x56, 0xEE]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x66, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::DrawSymbol {
                symbol_id,
                x,
                y,
                fg_color,
                bg_color,
            }) => {
                assert_eq!(
                    (symbol_id, x, y, fg_color, bg_color),
                    (0x05, 0x06, 0x07, 0x1234, 0x5678)
                );
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert_eq!(queue, VecDeque::from([0xEE]));
    }

    #[test]
    fn parses_signal_strength_without_consuming_battery_padding_bytes() {
        let mut queue = VecDeque::from([0x09, 0x02, 0xEE]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x67, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::SignalStrength {
                strength,
                mode,
                battery,
            }) => {
                assert_eq!((strength, mode, battery), (0x09, 0x02, 0x00));
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert_eq!(queue, VecDeque::from([0xEE]));
    }

    #[test]
    fn parses_documented_live_read_response() {
        let response = [
            0x57, 0x1B, 0x40, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let block = parse_live_read_response(0x1B40, &response).unwrap();
        assert_eq!(block.len(), BLOCK_SIZE);
        assert!(block.iter().all(|byte| *byte == 0x00));
    }

    #[test]
    fn rejects_misaligned_live_mode_address() {
        let error = validate_live_address(0x0CA1).unwrap_err().to_string();
        assert!(error.contains("aligned"));
    }

    #[test]
    fn infers_big_endian_settings_from_live_magic() {
        let raw = [
            0xD8, 0x2F, 0x04, 0x01, 0x00, 0x00, 0x13, 0x88, 0x0A, 0xF0, 0x0A, 0xF0,
        ];
        assert_eq!(
            RadioProtocol::infer_settings_endianness(&raw),
            Endianness::Big
        );
    }

    #[test]
    fn parses_live_like_channel_meta_from_byte_fifteen() {
        let raw = [
            0x00, 0xDD, 0xB9, 0xB8, 0x00, 0xDC, 0xCF, 0x58, 0x00, 0x00, 0x03, 0xB4, 0xFF, 0x10,
            0x00, 0x0A, 0xFF, 0xFF, 0xFF, 0xFF, 0x57, 0x36, 0x53, 0x4C, 0x47, 0x20, 0x20, 0x20,
            0x20, 0x20, 0x20, 0x00,
        ];

        let channel = RadioProtocol::parse_channel(&raw, 1, Endianness::Big)
            .expect("live-like channel should parse");

        assert_eq!(channel.rx_freq, "145.31000");
        assert_eq!(channel.tx_freq, "144.71000");
        assert_eq!(channel.bandwidth, "Wide");
        assert_eq!(channel.modulation, "FM");
        assert!(!channel.reverse);
        assert!(!channel.busy_lock);
        assert_eq!(channel.groups, [0, 0, 0, 1]);
        assert_eq!(channel.position, 1);
    }

    #[test]
    fn packs_live_like_ham_channel_exactly() {
        let raw = [
            0x00, 0xDD, 0xB9, 0xB8, 0x00, 0xDC, 0xCF, 0x58, 0x00, 0x00, 0x03, 0xB4, 0xFF, 0x10,
            0x00, 0x0A, 0xFF, 0xFF, 0xFF, 0xFF, 0x57, 0x36, 0x53, 0x4C, 0x47, 0x20, 0x20, 0x20,
            0x20, 0x20, 0x20, 0x00,
        ];
        let channel = RadioProtocol::parse_channel(&raw, 1, Endianness::Big)
            .expect("live-like channel should parse");

        assert_eq!(RadioProtocol::pack_channel(&channel, Endianness::Big), raw);
    }

    #[test]
    fn channel_frequency_packing_preserves_ten_hz_precision_in_both_byte_orders() {
        for endian in [Endianness::Big, Endianness::Little] {
            for frequency in ["145.35000", "145.34999", "146.94000", "446.00625"] {
                let channel = Channel {
                    channel_num: 1,
                    rx_freq: frequency.to_string(),
                    tx_freq: frequency.to_string(),
                    rx_tone: "Off".to_string(),
                    tx_tone: "Off".to_string(),
                    ..Channel::default()
                };
                let raw = RadioProtocol::pack_channel(&channel, endian);
                let expected = frequency.replace('.', "").parse::<u32>().unwrap();
                let expected_bytes = match endian {
                    Endianness::Big => expected.to_be_bytes(),
                    Endianness::Little => expected.to_le_bytes(),
                };
                assert_eq!(&raw[..4], &expected_bytes, "RX {frequency} {endian:?}");
                assert_eq!(&raw[4..8], &expected_bytes, "TX {frequency} {endian:?}");
                let decoded = RadioProtocol::parse_channel(&raw, 1, endian).unwrap();
                assert_eq!(decoded.rx_freq, frequency);
                assert_eq!(decoded.tx_freq, frequency);
                assert_eq!(RadioProtocol::pack_channel(&decoded, endian), raw);
            }
        }
    }

    #[test]
    fn channel_frequency_packing_does_not_truncate_adjacent_ten_hz_steps() {
        for expected in 14_530_000u32..14_540_000 {
            let frequency = format!("{}.{:05}", expected / 100_000, expected % 100_000);
            let channel = Channel {
                channel_num: 1,
                rx_freq: frequency.clone(),
                tx_freq: frequency.clone(),
                ..Channel::default()
            };
            for endian in [Endianness::Big, Endianness::Little] {
                let raw = RadioProtocol::pack_channel(&channel, endian);
                let actual = match endian {
                    Endianness::Big => BigEndian::read_u32(&raw[..4]),
                    Endianness::Little => LittleEndian::read_u32(&raw[..4]),
                };
                assert_eq!(actual, expected, "{frequency} {endian:?}");
            }
        }
    }

    #[test]
    fn packs_live_like_narrow_fm_channel_exactly() {
        let raw = [
            0x00, 0xED, 0x5F, 0x94, 0x00, 0xF2, 0xA9, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
            0x00, 0x0B, 0xFF, 0xFF, 0xFF, 0xFF, 0x53, 0x43, 0x20, 0x53, 0x68, 0x65, 0x72, 0x69,
            0x66, 0x66, 0x20, 0x42,
        ];
        let channel = RadioProtocol::parse_channel(&raw, 77, Endianness::Big)
            .expect("live-like channel should parse");

        assert_eq!(RadioProtocol::pack_channel(&channel, Endianness::Big), raw);
    }

    #[test]
    fn packs_live_like_am_channel_exactly() {
        let raw = [
            0x00, 0xBB, 0xB8, 0xA4, 0x00, 0xBB, 0xB8, 0xA4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30,
            0x00, 0x0C, 0xFF, 0xFF, 0xFF, 0xFF, 0x41, 0x69, 0x72, 0x2D, 0x41, 0x69, 0x72, 0x20,
            0x48, 0x65, 0x6C, 0x69,
        ];
        let channel = RadioProtocol::parse_channel(&raw, 142, Endianness::Big)
            .expect("live-like channel should parse");

        assert_eq!(RadioProtocol::pack_channel(&channel, Endianness::Big), raw);
    }

    #[test]
    fn ignores_cleared_placeholder_channel_template() {
        let raw = [
            0x00, 0xDB, 0xBA, 0x00, 0x00, 0xDB, 0xBA, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];

        assert!(RadioProtocol::parse_channel(&raw, 8, Endianness::Big).is_none());
    }

    #[test]
    fn parse_channel_rejects_short_raw_buffers() {
        assert!(RadioProtocol::parse_channel(&[0x00, 0xDD, 0xB9], 1, Endianness::Big).is_none());
    }

    #[test]
    fn parses_live_like_bandplan_record_big_endian() {
        let raw = [0x00, 0xA4, 0xCB, 0x80, 0x00, 0xD1, 0x0B, 0xA0, 0x00, 0x4A];
        let plan = RadioProtocol::parse_bandplan(&raw, 0, Endianness::Big);

        assert_eq!(plan.start_freq, 10_800_000);
        assert_eq!(plan.end_freq, 13_700_000);
        assert_eq!(plan.modulation, 2);
        assert_eq!(plan.bandwidth, 2);
        assert!(plan.wrap);
        assert!(!plan.tx_allowed);
        assert_eq!(plan.raw_flags, 0x4A);
    }

    #[test]
    fn bandplan_byte_order_ignores_partial_records_and_records_past_table() {
        let big = [0x00, 0xA4, 0xCB, 0x80, 0x00, 0xD1, 0x0B, 0xA0, 0x00, 0x4A];
        let mut little = big;
        little[..4].reverse();
        little[4..8].reverse();
        for (record, endian) in [(big, Endianness::Big), (little, Endianness::Little)] {
            for remainder in 0..BAND_PLAN_RECORD_SIZE {
                let mut raw = record.to_vec();
                raw.extend(vec![0xFF; remainder]);
                assert_eq!(RadioProtocol::infer_bandplan_endianness(&raw), endian);
            }
            let mut raw = record.to_vec();
            raw.resize(BAND_PLAN_RECORD_COUNT * BAND_PLAN_RECORD_SIZE, 0);
            raw.extend(big);
            raw.extend(little);
            assert_eq!(RadioProtocol::infer_bandplan_endianness(&raw), endian);
        }
        for length in 0..BAND_PLAN_RECORD_SIZE {
            assert_eq!(
                RadioProtocol::infer_bandplan_endianness(&big[..length]),
                Endianness::Big
            );
        }
    }

    #[test]
    fn parses_live_like_scan_preset_record() {
        let raw = [
            0x00, 0xB2, 0x87, 0x20, 0x07, 0x6C, 0x13, 0x88, 0x02, 0x00, 0x11, 0x61, 0x69, 0x72,
            0x20, 0x20, 0x20, 0x20, 0x00, 0x00,
        ];
        let preset = RadioProtocol::parse_scan_preset(&raw, 0, Endianness::Big);

        assert_eq!(preset.start_freq, 11_700_000);
        assert_eq!(preset.range, 1900);
        assert_eq!(preset.step, 5000);
        assert_eq!(preset.modulation, 1);
        assert_eq!(preset.ultrascan, 4);
        assert_eq!(preset.label, "air");
        assert_eq!(preset.raw_mode, 0x11);
    }

    #[test]
    fn bandplan_flags_match_original_bitfields_and_edits_override_raw_copy() {
        for flags in 0..=u8::MAX {
            let mut raw = [0u8; BAND_PLAN_RECORD_SIZE];
            raw[9] = flags;
            let mut plan = RadioProtocol::parse_bandplan(&raw, 0, Endianness::Big);
            assert_eq!(plan.tx_allowed, flags & 1 != 0);
            assert_eq!(plan.wrap, flags & 2 != 0);
            assert_eq!(plan.modulation, (flags >> 2) & 7);
            assert_eq!(plan.bandwidth, flags >> 5);
            assert_eq!(RadioProtocol::pack_bandplan(&plan, Endianness::Big), raw);
            plan.tx_allowed = !plan.tx_allowed;
            assert_eq!(
                RadioProtocol::pack_bandplan(&plan, Endianness::Big)[9],
                flags ^ 1
            );
        }
    }

    #[test]
    fn scan_mode_and_ultrascan_share_byte_ten_not_name_terminator() {
        assert_eq!(SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE, 400);
        for mode in 0..=u8::MAX {
            let mut raw = [0u8; SCAN_PRESET_RECORD_SIZE];
            raw[10] = mode;
            raw[11..19].copy_from_slice(b"VHF     ");
            let mut preset = RadioProtocol::parse_scan_preset(&raw, 0, Endianness::Big);
            assert_eq!(preset.modulation, mode & 3);
            assert_eq!(preset.ultrascan, mode >> 2);
            assert_eq!(
                RadioProtocol::pack_scan_preset(&preset, Endianness::Big),
                raw
            );
            preset.ultrascan = 0;
            let packed = RadioProtocol::pack_scan_preset(&preset, Endianness::Big);
            assert_eq!(packed[10], mode & 3);
            assert_eq!(packed[19], 0);
        }
    }

    #[test]
    fn empty_scan_labels_remain_empty_in_every_slot() {
        for index in 0..SCAN_PRESET_RECORD_COUNT {
            let preset = RadioProtocol::parse_scan_preset(
                &[0; SCAN_PRESET_RECORD_SIZE],
                index as u8,
                Endianness::Big,
            );
            assert!(preset.label.is_empty());
        }
    }

    #[test]
    fn dtmf_records_match_original_twelve_byte_layout() {
        let raw = [
            0x19, 0x32, 0x54, 0x76, 0x98, b'R', b'E', b'P', b'E', b'A', b'T', b'R',
        ];
        let preset = RadioProtocol::parse_dtmf_preset(&raw, 19);
        assert_eq!(preset.digits, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(preset.label, "REPEATR");
        assert_eq!(RadioProtocol::pack_dtmf_preset(&preset), raw);
        let erased = RadioProtocol::parse_dtmf_preset(&[0xFF; DTMF_PRESET_RECORD_SIZE], 0);
        assert!(erased.digits.is_empty());
    }

    #[test]
    fn channel_dcs_codes_use_octal_digits_in_both_byte_orders() {
        for endian in [Endianness::Big, Endianness::Little] {
            let channel = Channel {
                channel_num: 1,
                rx_freq: "145.35000".to_string(),
                tx_freq: "145.35000".to_string(),
                rx_tone: "D023N".to_string(),
                tx_tone: "D754I".to_string(),
                modulation: "Auto".to_string(),
                ..Channel::default()
            };
            let raw = RadioProtocol::pack_channel(&channel, endian);
            let word = |bytes: &[u8]| match endian {
                Endianness::Big => BigEndian::read_u16(bytes),
                Endianness::Little => LittleEndian::read_u16(bytes),
            };
            assert_eq!(word(&raw[8..10]), 0x8000 | 0o23);
            assert_eq!(word(&raw[10..12]), 0xC000 | 0o754);
            let decoded = RadioProtocol::parse_channel(&raw, 1, endian).unwrap();
            assert_eq!(decoded.rx_tone, "D023n");
            assert_eq!(decoded.tx_tone, "D754i");
            assert_eq!(decoded.modulation, "Auto");
            assert_eq!(RadioProtocol::pack_channel(&decoded, endian), raw);
        }
    }

    fn frequency_sample(rx: u32, tx: u32, endian: Endianness) -> Vec<u8> {
        let mut raw = vec![0; BLOCK_SIZE];
        match endian {
            Endianness::Big => {
                BigEndian::write_u32(&mut raw[..4], rx);
                BigEndian::write_u32(&mut raw[4..8], tx);
            }
            Endianness::Little => {
                LittleEndian::write_u32(&mut raw[..4], rx);
                LittleEndian::write_u32(&mut raw[4..8], tx);
            }
        }
        raw
    }

    #[test]
    fn channel_byte_order_skips_ambiguous_samples_and_uses_rx_and_tx() {
        let ambiguous = frequency_sample(0x013A3701, 0x013A3701, Endianness::Big);
        for endian in [Endianness::Big, Endianness::Little] {
            let strong = frequency_sample(14_535_000, 14_535_000, endian);
            assert_eq!(
                RadioProtocol::channel_endianness_from_samples(&[ambiguous.clone(), strong])
                    .unwrap(),
                (Some(endian), false),
            );
        }
        let tx_disambiguates = frequency_sample(0x013A3701, 14_535_000, Endianness::Big);
        assert_eq!(
            RadioProtocol::channel_endianness_from_samples(&[tx_disambiguates]).unwrap(),
            (Some(Endianness::Big), false)
        );
        let high_band = frequency_sample(120_000_000, 120_000_000, Endianness::Big);
        assert_eq!(
            RadioProtocol::channel_endianness_from_samples(&[high_band]).unwrap(),
            (Some(Endianness::Big), false)
        );
        for tx in [0, u32::MAX] {
            let receive_only = frequency_sample(14_535_000, tx, Endianness::Big);
            assert_eq!(
                RadioProtocol::channel_endianness_from_samples(&[receive_only]).unwrap(),
                (Some(Endianness::Big), false)
            );
        }
    }

    #[test]
    fn channel_byte_order_rejects_conflicts_and_does_not_guess_ambiguous_records() {
        let big = frequency_sample(14_535_000, 14_535_000, Endianness::Big);
        let little = frequency_sample(14_535_000, 14_535_000, Endianness::Little);
        assert!(RadioProtocol::channel_endianness_from_samples(&[big, little]).is_err());
        let ambiguous = frequency_sample(0x013A3701, 0x013A3701, Endianness::Big);
        assert_eq!(
            RadioProtocol::channel_endianness_from_samples(&[ambiguous]).unwrap(),
            (None, false)
        );
        assert_eq!(
            RadioProtocol::channel_endianness_from_samples(&[vec![0xFF; BLOCK_SIZE]]).unwrap(),
            (None, true)
        );
        assert!(RadioProtocol::channel_endianness_from_samples(&[vec![0; 4]]).is_err());
    }

    #[test]
    fn empty_radio_byte_order_requires_a_recognized_settings_magic() {
        assert_eq!(
            RadioProtocol::recognized_settings_endianness(&SETTINGS_MAGIC.to_be_bytes()),
            Some(Endianness::Big)
        );
        assert_eq!(
            RadioProtocol::recognized_settings_endianness(&SETTINGS_MAGIC.to_le_bytes()),
            Some(Endianness::Little)
        );
        assert_eq!(
            RadioProtocol::recognized_settings_endianness(&[0xFF; BLOCK_SIZE]),
            None
        );
        assert_eq!(RadioProtocol::recognized_settings_endianness(&[0]), None);
    }

    #[test]
    fn signal_packet_summary_does_not_invent_a_battery_measurement() {
        let packet = RemotePacket::SignalStrength {
            strength: 20,
            mode: 0,
            battery: 0,
        };
        assert!(!packet.summary().contains("batt"));
        assert_eq!(packet.detail_key(), "RSSI:20:0");
    }

    #[test]
    fn parses_group_label_table() {
        let raw = [
            0x48, 0x61, 0x6D, 0x00, 0x00, 0x00, 0x44, 0x69, 0x73, 0x70, 0x61, 0x00, 0x41, 0x69,
            0x72, 0x00, 0x00, 0x00, 0x57, 0x65, 0x61, 0x74, 0x68, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let labels = RadioProtocol::parse_group_labels(&raw);

        assert_eq!(labels.len(), GROUP_LABEL_COUNT);
        assert_eq!(labels[0], "Ham");
        assert_eq!(labels[1], "Dispa");
        assert_eq!(labels[2], "Air");
        assert_eq!(labels[3], "Weath");
        assert!(labels[4].is_empty());
    }

    #[test]
    fn packs_group_label_table_with_fixed_width_records() {
        let labels = vec![
            "Ham".to_string(),
            "Dispatch".to_string(),
            "Air".to_string(),
            "Weather".to_string(),
        ];

        let packed = RadioProtocol::pack_group_labels(&labels);

        assert_eq!(packed.len(), GROUP_LABEL_COUNT * GROUP_LABEL_RECORD_SIZE);
        assert_eq!(&packed[0..6], b"Ham\0\0\0");
        assert_eq!(&packed[6..12], b"Dispa\0");
        assert_eq!(&packed[12..18], b"Air\0\0\0");
        assert_eq!(&packed[18..24], b"Weath\0");
    }

    #[test]
    fn group_table_has_fifteen_five_character_names_and_ignores_reserved_slot() {
        let mut raw = [0xFF; 96];
        raw[..6].copy_from_slice(b"A\0B\0\0X");
        raw[84..90].copy_from_slice(b"LAST!\0");
        raw[90..96].copy_from_slice(b"unused");
        let labels = RadioProtocol::parse_group_labels(&raw);
        assert_eq!(labels.len(), 15);
        assert_eq!(labels[0], "A B");
        assert!(labels[1].is_empty());
        assert_eq!(labels[14], "LAST!");
        let packed = RadioProtocol::pack_group_labels(&labels);
        assert_eq!(packed.len(), 90);
        assert_eq!(&packed[84..90], b"LAST!\0");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn uses_pty_safe_baud_for_real_ptys_only() {
        assert!(should_use_pty_safe_baud("/dev/ttys036"));
        assert_eq!(effective_baud_rate("/dev/ttys036", BAUD_RATE), 0);
        assert_eq!(effective_baud_rate("/dev/pts/1", BAUD_RATE), 0);
        assert!(!should_use_pty_safe_baud("/dev/cu.usbserial-210"));
        assert_eq!(
            effective_baud_rate("/dev/cu.usbserial-210", BAUD_RATE),
            BAUD_RATE
        );
        assert!(!should_use_pty_safe_baud(
            "ble://12345678-1234-5678-9ABC-DEF012345678"
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_preserves_requested_baud_for_all_port_names() {
        for port_name in ["/dev/ttys036", "/dev/pts/1", "/dev/ttyUSB0", "COM1"] {
            for baud_rate in [0, 9_600, BAUD_RATE] {
                assert_eq!(effective_baud_rate(port_name, baud_rate), baud_rate);
            }
        }
    }
}
