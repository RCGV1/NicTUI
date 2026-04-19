use anyhow::{Result, anyhow};
use byteorder::{BigEndian, ByteOrder, LittleEndian};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use super::metadata::SETTINGS_METADATA;
use super::types::*;

pub const SETTINGS_MAGIC: u16 = 0xD82F;
pub const BAND_PLAN_RECORD_COUNT: usize = 20;
pub const BAND_PLAN_RECORD_SIZE: usize = 10;
pub const SCAN_PRESET_RECORD_COUNT: usize = 8;
pub const SCAN_PRESET_RECORD_SIZE: usize = 20;
pub const GROUP_LABEL_RECORD_COUNT: usize = GROUP_LABEL_COUNT;
pub const GROUP_LABEL_RECORD_SIZE: usize = GROUP_LABEL_SIZE;

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
        battery: u8, // 0-100?
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
            RemotePacket::SignalStrength {
                strength,
                mode,
                battery,
            } => format!("RSSI {strength:>3} mode {mode:>2} batt {battery:>3}"),
            RemotePacket::NoiseLevel { level, mode } => {
                format!("NOISE {level:>3} mode {mode:>2}")
            }
            RemotePacket::SignalBarPos { y, aux } => {
                format!("SBAR y {y:>3} aux {aux:>3}")
            }
            RemotePacket::SmallStatus { id, value1, value2 } => {
                format!("STS {id:02X}  {value1:02X} {value2:02X}")
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

pub struct RadioProtocol {
    port: Box<dyn serialport::SerialPort>,
    pub log_callback: Option<Box<dyn Fn(String) + Send + Sync>>,
}

impl RadioProtocol {
    pub fn new(port_name: &str) -> Result<Self> {
        Self::new_with_baud(port_name, BAUD_RATE)
    }

    pub fn new_with_baud(port_name: &str, baud_rate: u32) -> Result<Self> {
        let builder = serialport::new(port_name, baud_rate).timeout(Duration::from_millis(50));
        #[cfg(unix)]
        let port: Box<dyn serialport::SerialPort> = {
            let mut port = builder.open_native()?;
            #[cfg(target_os = "macos")]
            {
                let _ = port.set_exclusive(false);
            }
            Box::new(port)
        };
        #[cfg(not(unix))]
        let mut port = builder.open()?;
        let _ = port.clear(serialport::ClearBuffer::All);
        Ok(Self {
            port,
            log_callback: None,
        })
    }

    fn log(&self, msg: String) {
        if let Some(ref cb) = self.log_callback {
            cb(msg);
        }
    }

    fn send(&mut self, data: &[u8]) -> Result<()> {
        self.log(format!("TX: {:02X?}", data));
        self.port.write_all(data)?;
        self.port.flush()?;
        Ok(())
    }

    pub fn send_bytes(&mut self, data: &[u8]) -> Result<()> {
        self.send(data)
    }

    pub fn read_byte(&mut self) -> Result<Option<u8>> {
        let mut buf = [0u8; 1];
        match self.port.read(&mut buf) {
            Ok(1) => {
                self.log(format!("RX: [{:02X}]", buf[0]));
                Ok(Some(buf[0]))
            }
            Ok(_) => Ok(None),
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn recv(&mut self, length: usize, timeout: Duration) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; length];
        let start = Instant::now();
        let mut read_bytes = 0;

        while read_bytes < length && start.elapsed() < timeout {
            match self.port.read(&mut buffer[read_bytes..]) {
                Ok(n) => read_bytes += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
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

        self.log(format!("RX: {:02X?}", buffer));
        Ok(buffer)
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

    pub fn send_key(&mut self, key_code: u8) -> Result<()> {
        self.send(&[key_code])?;
        Ok(())
    }

    pub fn press_remote_key(&mut self, key_code: u8) -> Result<()> {
        std::thread::sleep(Duration::from_millis(50));
        self.send_key(key_code)?;
        std::thread::sleep(Duration::from_millis(60));
        self.send_bytes(&[0xFF])?;
        std::thread::sleep(Duration::from_millis(20));
        Ok(())
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
        for blk_num in 2..10 {
            let blk = self.read_block(blk_num)?;
            let rx_freq_le = u32::from_le_bytes(blk[0..4].try_into().unwrap());
            let rx_freq_be = u32::from_be_bytes(blk[0..4].try_into().unwrap());

            let le_valid = (100000..=100000000).contains(&rx_freq_le);
            let be_valid = (100000..=100000000).contains(&rx_freq_be);

            if le_valid && !be_valid {
                return Ok(Endianness::Little);
            }
            if be_valid && !le_valid {
                return Ok(Endianness::Big);
            }
            if le_valid && be_valid {
                return Ok(Endianness::Little);
            }
        }
        Ok(Endianness::Big)
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
                .chunks_exact(BAND_PLAN_RECORD_SIZE)
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

    pub fn infer_scan_preset_endianness(raw: &[u8]) -> Endianness {
        let score = |endian| {
            let mut valid = 0usize;
            for chunk in raw
                .chunks_exact(SCAN_PRESET_RECORD_SIZE)
                .take(SCAN_PRESET_RECORD_COUNT)
            {
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
                let label = chunk[11..19].iter().all(|byte| {
                    *byte == 0 || *byte == b' ' || byte.is_ascii_alphanumeric() || *byte == b'.'
                });

                if start == 0 {
                    continue;
                }
                if (1_000_000..=130_000_000).contains(&start)
                    && range <= 10_000
                    && (100..=10_000).contains(&step)
                    && label
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

    pub fn read_block(&mut self, block_num: u8) -> Result<Vec<u8>> {
        for attempt in 0..6 {
            let _ = self.port.clear(serialport::ClearBuffer::Input);
            std::thread::sleep(Duration::from_millis(15));
            self.send(&[PKT_READ_EEPROM, block_num])?;

            match self.recv(1 + BLOCK_SIZE + 1, Duration::from_millis(1000)) {
                Ok(resp) if resp[0] == PKT_READ_EEPROM => {
                    let data = &resp[1..1 + BLOCK_SIZE];
                    let checksum = resp[1 + BLOCK_SIZE];
                    let calculated_sum: u8 = data.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
                    if calculated_sum == checksum {
                        std::thread::sleep(Duration::from_millis(8));
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

            let _ = self.port.clear(serialport::ClearBuffer::All);
            std::thread::sleep(Duration::from_millis(120 + attempt as u64 * 30));
        }
        Err(anyhow!("Failed to read block {} after retries", block_num))
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
            let _ = self.port.clear(serialport::ClearBuffer::Input);
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

            let _ = self.port.clear(serialport::ClearBuffer::All);
            std::thread::sleep(Duration::from_millis(120 + attempt as u64 * 30));
        }
        Ok(false)
    }

    pub fn parse_channel(raw: &[u8], channel_num: u16, endian: Endianness) -> Option<Channel> {
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
                0 => "LSB".to_string(),
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
        let rx_freq_raw = (ch.rx_freq.parse::<f64>().unwrap_or(0.0) * 100000.0) as u32;
        let tx_freq_raw = (ch.tx_freq.parse::<f64>().unwrap_or(0.0) * 100000.0) as u32;

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
            "LSB" => 0,
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
        let Some(id) = self.read_byte()? else {
            return Ok(None);
        };

        // NOP packets are just ignored
        if id == 0x00 {
            return Ok(None);
        }

        let packet =
            Self::parse_remote_packet_inner(id, |length, timeout| self.recv(length, timeout))?;
        if packet.is_none() {
            self.log(format!("REMOTE: unhandled packet {:02X}", id));
        }
        Ok(packet)
    }

    fn parse_remote_packet_inner<F>(id: u8, mut recv: F) -> Result<Option<RemotePacket>>
    where
        F: FnMut(usize, Duration) -> Result<Vec<u8>>,
    {
        match id {
            0x64 => {
                // Display Text
                let header = recv(7, Duration::from_millis(100))?;
                let font_size = header[0];
                let x = header[1];
                let y = header[2];
                let fg_color = LittleEndian::read_u16(&header[3..5]);
                let bg_color = LittleEndian::read_u16(&header[5..7]);

                // Read null-terminated string
                let mut text_bytes = Vec::new();
                loop {
                    let b = recv(1, Duration::from_millis(100))?[0];
                    if b == 0 {
                        break;
                    }
                    text_bytes.push(b);
                }
                // Skip padding
                let _ = recv(2, Duration::from_millis(10));

                Ok(Some(RemotePacket::DisplayText {
                    font_size,
                    x,
                    y,
                    fg_color,
                    bg_color,
                    text: String::from_utf8_lossy(&text_bytes).to_string(),
                }))
            }
            0x65 => {
                // Draw Rectangle
                let data = recv(6, Duration::from_millis(100))?;
                let x = data[0];
                let y = data[1];
                let width = data[2];
                let height = data[3];
                let color = LittleEndian::read_u16(&data[4..6]);
                let _ = recv(2, Duration::from_millis(10));
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
                let data = recv(7, Duration::from_millis(100))?;
                let symbol_id = data[0];
                let x = data[1];
                let y = data[2];
                let fg_color = LittleEndian::read_u16(&data[3..5]);
                let bg_color = LittleEndian::read_u16(&data[5..7]);
                let _ = recv(2, Duration::from_millis(10));
                Ok(Some(RemotePacket::DrawSymbol {
                    symbol_id,
                    x,
                    y,
                    fg_color,
                    bg_color,
                }))
            }
            0x67 => {
                // Signal Strength (2 bytes data + 2 bytes padding)
                let data = recv(2, Duration::from_millis(100))?;
                let padding = recv(2, Duration::from_millis(10))?;

                // Based on empirical testing, battery might be in padding bytes
                // Documentation doesn't mention battery, but hardware seems to send it
                let battery = if padding[0] <= 100 { padding[0] } else { 0 };

                Ok(Some(RemotePacket::SignalStrength {
                    strength: data[0],
                    mode: data[1],
                    battery,
                }))
            }
            // The Nicsure helper used by these packets shifts out two data bytes after the opcode.
            // Accept the high-bit form as well until the live wire value is captured directly.
            0x68 | 0xE8 => {
                let data = recv(2, Duration::from_millis(100))?;
                Ok(Some(RemotePacket::NoiseLevel {
                    level: data[0],
                    mode: data[1],
                }))
            }
            0x69 | 0xE9 => {
                let data = recv(2, Duration::from_millis(100))?;
                Ok(Some(RemotePacket::SignalBarPos {
                    y: data[0],
                    aux: data[1],
                }))
            }
            0x70..=0x7F => {
                let data = recv(2, Duration::from_millis(100))?;
                Ok(Some(RemotePacket::SmallStatus {
                    id,
                    value1: data[0],
                    value2: data[1],
                }))
            }
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
            tx_allowed: bits == 0x27,
            wrap: bits == 0x27,
            bandwidth: if bits == 0xA0 { 1 } else { 0 },
            modulation: if bits == 0x4A { 1 } else { 0 },
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
        raw[9] = if bp.raw_flags != 0 {
            bp.raw_flags
        } else if bp.modulation == 1 {
            0x4A
        } else if bp.tx_allowed || bp.wrap {
            0x27
        } else {
            0x00
        };
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
            modulation: match raw_mode & 0x0F {
                1 => 1,
                2 => 2,
                _ => 0,
            },
            ultrascan: raw[19],
            label: if label.is_empty() {
                format!("Preset {}", index + 1)
            } else {
                label
            },
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
        let mode_prefix = if sp.raw_mode & 0xF0 != 0 {
            sp.raw_mode & 0xF0
        } else {
            0x10
        };
        raw[10] = mode_prefix
            | match sp.modulation {
                1 => 1,
                2 => 2,
                _ => 0,
            };
        raw[11..19].fill(b' ');
        let label_bytes = sp.label.as_bytes();
        let label_len = label_bytes.len().min(8);
        raw[11..11 + label_len].copy_from_slice(&label_bytes[..label_len]);
        raw[19] = if sp.ultrascan != 0 {
            sp.ultrascan
        } else {
            sp.raw_tail
        };
        raw
    }

    pub fn parse_dtmf_preset(raw: &[u8], index: u8) -> DTMFPreset {
        let length = (raw[0] & 0x0F) as usize;
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

        let label = String::from_utf8_lossy(&raw[5..13])
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
        let mut raw = vec![0u8; 13];
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
        let label_len = label_bytes.len().min(8);
        raw[5..5 + label_len].copy_from_slice(&label_bytes[..label_len]);
        raw
    }

    pub fn parse_group_labels(raw: &[u8]) -> Vec<String> {
        let mut labels = Vec::with_capacity(GROUP_LABEL_RECORD_COUNT);
        for index in 0..GROUP_LABEL_RECORD_COUNT {
            let start = index * GROUP_LABEL_RECORD_SIZE;
            let end = start + GROUP_LABEL_RECORD_SIZE;
            let label = raw
                .get(start..end)
                .map(|bytes| {
                    String::from_utf8_lossy(bytes)
                        .trim_matches(char::from(0))
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
            let len = bytes.len().min(GROUP_LABEL_RECORD_SIZE);
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
        if index >= SETTINGS_METADATA.len() {
            return "N/A".to_string();
        }
        let meta = &SETTINGS_METADATA[index];
        let val = self.get_value(index);
        match meta.setting_type {
            SettingType::Boolean => {
                if val != 0 {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            SettingType::Enum(opts) => {
                if (val as usize) < opts.len() {
                    opts[val as usize].to_string()
                } else {
                    format!("Unknown ({})", val)
                }
            }
            SettingType::Numeric { unit, .. } => format!("{}{}", val, unit),
        }
    }
}

fn parse_tone(raw: u16) -> String {
    if raw == 0 {
        return "Off".to_string();
    }
    if raw & 0x8000 != 0 {
        let dcs = raw & 0x3FFF;
        let inverted = raw & 0x4000 != 0;
        return format!("D{}{}", dcs, if inverted { "i" } else { "n" });
    }
    format!("{:.1}", raw as f64 / 10.0)
}

fn pack_tone(tone: &str) -> u16 {
    if tone.to_lowercase() == "off" {
        return 0;
    }
    if let Some(rest) = tone.strip_prefix('D') {
        let inverted = rest.ends_with('i');
        let dcs_str = rest.trim_end_matches(['i', 'n']);
        let dcs = dcs_str.parse::<u16>().unwrap_or(0);
        let mut val = 0x8000 | (dcs & 0x3FFF);
        if inverted {
            val |= 0x4000;
        }
        return val;
    }
    (tone.parse::<f64>().unwrap_or(0.0) * 10.0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

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
    fn parses_small_status_packets_with_two_payload_bytes() {
        let mut queue = VecDeque::from([0x12, 0x34]);
        let packet = RadioProtocol::parse_remote_packet_inner(0x72, |length, timeout| {
            recv_from_queue(&mut queue, length, timeout)
        })
        .unwrap();

        match packet {
            Some(RemotePacket::SmallStatus { id, value1, value2 }) => {
                assert_eq!(id, 0x72);
                assert_eq!(value1, 0x12);
                assert_eq!(value2, 0x34);
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert!(queue.is_empty());
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
                assert_eq!(aux, 0xAA);
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        assert!(queue.is_empty());
    }

    #[test]
    fn parses_noise_packet_without_padding() {
        let mut queue = VecDeque::from([0x09, 0x02]);
        let packet = RadioProtocol::parse_remote_packet_inner(0xE8, |length, timeout| {
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
    fn parses_live_like_bandplan_record_big_endian() {
        let raw = [0x00, 0xA4, 0xCB, 0x80, 0x00, 0xD1, 0x0B, 0xA0, 0x00, 0x4A];
        let plan = RadioProtocol::parse_bandplan(&raw, 0, Endianness::Big);

        assert_eq!(plan.start_freq, 10_800_000);
        assert_eq!(plan.end_freq, 13_700_000);
        assert_eq!(plan.modulation, 1);
        assert_eq!(plan.raw_flags, 0x4A);
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
        assert_eq!(preset.label, "air");
        assert_eq!(preset.raw_mode, 0x11);
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
        assert_eq!(&packed[6..12], b"Dispat");
        assert_eq!(&packed[12..18], b"Air\0\0\0");
        assert_eq!(&packed[18..24], b"Weathe");
    }
}
