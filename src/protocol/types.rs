use serde::{Deserialize, Serialize};

pub const BAUD_RATE: u32 = 38400;
pub const BIN_FLASH_BAUD_RATE: u32 = 115200;
pub const EEPROM_SIZE: usize = 0x2000; // 8 KiB
pub const BLOCK_SIZE: usize = 32;
pub const TOTAL_BLOCKS: usize = EEPROM_SIZE / BLOCK_SIZE;
pub const GROUP_LABEL_COUNT: usize = 16;
pub const GROUP_LABEL_SIZE: usize = 6;
pub const GROUP_LABELS_OFFSET: usize = 0x1C90;
pub const GROUP_LABELS_BLOCK_START: usize = GROUP_LABELS_OFFSET / BLOCK_SIZE;
pub const GROUP_LABELS_BLOCK_OFFSET: usize = GROUP_LABELS_OFFSET % BLOCK_SIZE;
pub const GROUP_LABELS_BLOCK_COUNT: usize =
    (GROUP_LABELS_BLOCK_OFFSET + (GROUP_LABEL_COUNT * GROUP_LABEL_SIZE)).div_ceil(BLOCK_SIZE);

pub const PKT_PING1: u8 = 0x01;
pub const PKT_DISABLE: u8 = 0x45;
pub const PKT_ENABLE: u8 = 0x46;
pub const PKT_REBOOT: u8 = 0x49;
pub const PKT_READ_EEPROM: u8 = 0x30;
pub const PKT_WRITE_EEPROM: u8 = 0x31;
pub const PKT_REMOTE_ON: u8 = 0x4A;
pub const PKT_REMOTE_OFF: u8 = 0x4B;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Endianness {
    Little,
    Big,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Channel {
    #[serde(alias = "Channel_Num")]
    pub channel_num: u16,
    #[serde(default)]
    pub name: String,
    #[serde(alias = "RX", alias = "RX_Freq", default)]
    pub rx_freq: String,
    #[serde(alias = "TX", alias = "TX_Freq", default)]
    pub tx_freq: String,
    #[serde(alias = "RX_Tone", default)]
    pub rx_tone: String,
    #[serde(alias = "TX_Tone", default)]
    pub tx_tone: String,
    #[serde(alias = "TX_Power", default)]
    pub power: u8,
    #[serde(default)]
    pub bandwidth: String,
    #[serde(default)]
    pub modulation: String,
    #[serde(alias = "Reversed", default)]
    pub reverse: bool,
    #[serde(alias = "BusyLock", default)]
    pub busy_lock: bool,
    #[serde(default)]
    pub groups: [u8; 4],
    #[serde(alias = "PTTID", default)]
    pub ptt_id: u8,
    #[serde(alias = "Active", default)]
    pub position: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BandPlan {
    pub index: u8,
    pub start_freq: u32,
    pub end_freq: u32,
    pub max_power: u8,
    pub tx_allowed: bool,
    pub wrap: bool,
    pub modulation: u8,
    pub bandwidth: u8,
    #[serde(default)]
    pub raw_flags: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanPreset {
    pub index: u8,
    pub start_freq: u32,
    pub range: u16,
    pub step: u16,
    pub resume: u8,
    pub persist: u8,
    pub modulation: u8,
    pub ultrascan: u8,
    pub label: String,
    #[serde(default)]
    pub raw_mode: u8,
    #[serde(default)]
    pub raw_tail: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DTMFPreset {
    pub index: u8,
    pub digits: Vec<u8>,
    pub label: String,
}

pub fn normalize_group_labels(labels: &[String]) -> Vec<String> {
    let mut normalized = vec![String::new(); GROUP_LABEL_COUNT];
    for (slot, label) in normalized.iter_mut().zip(labels.iter()) {
        *slot = label
            .trim()
            .chars()
            .take(GROUP_LABEL_SIZE)
            .collect::<String>();
    }
    normalized
}

pub fn group_letter(group: u8) -> Option<char> {
    if (1..=26).contains(&group) {
        Some((b'A' + group - 1) as char)
    } else {
        None
    }
}

pub fn group_label(labels: &[String], group: u8) -> Option<&str> {
    let index = group.checked_sub(1)? as usize;
    let label = labels.get(index)?.trim();
    if label.is_empty() { None } else { Some(label) }
}

pub fn group_display(group: u8, labels: &[String]) -> String {
    match group_letter(group) {
        Some(letter) => match group_label(labels, group) {
            Some(label) => format!("{letter} {label}"),
            None => letter.to_string(),
        },
        None => group.to_string(),
    }
}

pub fn group_display_compact(group: u8, labels: &[String]) -> String {
    match group_letter(group) {
        Some(letter) => match group_label(labels, group) {
            Some(label) => format!("{letter}/{label}"),
            None => letter.to_string(),
        },
        None => group.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SettingsBlock {
    pub magic: u16,               // 0x00 (0xD82F)
    pub squelch: u8,              // 0x02
    pub dual_watch: u8,           // 0x03
    pub auto_floor: u8,           // 0x04 (not used)
    pub active_vfo: u8,           // 0x05
    pub step: u16,                // 0x06 (10Hz units)
    pub rx_split: u16,            // 0x08 (100kHz units)
    pub tx_split: u16,            // 0x0a (100kHz units)
    pub ptt_mode: u8,             // 0x0c
    pub tx_mod_meter: u8,         // 0x0d
    pub mic_gain: u8,             // 0x0e
    pub tx_deviation: u8,         // 0x0f
    pub xtal671_defunct: i8,      // 0x10 (no longer used)
    pub batt_style: u8,           // 0x11
    pub scan_range: u16,          // 0x12
    pub scan_persist: u16,        // 0x14
    pub scan_resume: u8,          // 0x16
    pub ultra_scan: u8,           // 0x17
    pub tone_monitor: u8,         // 0x18
    pub lcd_brightness: u8,       // 0x19
    pub lcd_timeout: u8,          // 0x1a
    pub breathe: u8,              // 0x1b
    pub dtmf_dev: u8,             // 0x1c
    pub gamma: u8,                // 0x1d
    pub repeater_tone: u16,       // 0x1e
    pub vfo_state: [VfoState; 2], // 0x20
    pub key_lock: u8,             // 0x46
    pub bluetooth: u8,            // 0x47
    pub power_save: u8,           // 0x48
    pub key_tones: u8,            // 0x49
    pub ste: u8,                  // 0x4a
    pub rf_gain: u8,              // 0x4b
    pub s_bar_style: u8,          // 0x4c
    pub sq_noise_lev: u8,         // 0x4d
    pub last_fmt_freq: u32,       // 0x4e
    pub vox: u8,                  // 0x52
    pub vox_tail: u16,            // 0x53
    pub tx_timeout: u8,           // 0x55
    pub dimmer: u8,               // 0x56
    pub dtmf_speed: u8,           // 0x57
    pub noise_gate: u8,           // 0x58
    pub scan_update: u8,          // 0x59
    pub asl: u8,                  // 0x5a
    pub disable_fmt: u8,          // 0x5b
    pub pin: u16,                 // 0x5c
    pub pin_action: u8,           // 0x5e
    pub lcd_inverted: u8,         // 0x5f
    pub af_filters: u8,           // 0x60
    pub if_freq: u8,              // 0x61
    pub s_bar_always_on: u8,      // 0x62
    pub locked_vfo: u8,           // 0x63
    pub vfo_lock_active: u8,      // 0x64
    pub dual_watch_delay: u8,     // 0x65
    pub sub_tone_deviation: u8,   // 0x66
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VfoState {
    pub group: u8,
    pub last_group: u8,
    pub group_mode_channels: [u8; 16],
    pub mode: u8,
}

#[derive(Debug, Clone, Copy)]
pub enum SettingType {
    Numeric {
        min: i32,
        max: i32,
        unit: &'static str,
    },
    Boolean,
    Enum(&'static [&'static str]),
}

pub struct SettingMetadata {
    pub menu_num: &'static str,
    pub name: &'static str,
    pub setting_type: SettingType,
}

#[cfg(test)]
mod tests {
    use super::*;
    use csv::Reader;
    use std::collections::HashMap;

    fn parse_row(row: HashMap<String, String>) -> Channel {
        let get_val = |key: &str| -> Option<&String> {
            row.get(key).or_else(|| {
                let key_lower = key.to_lowercase();
                row.iter()
                    .find(|(k, _)| k.to_lowercase() == key_lower)
                    .map(|(_, v)| v)
            })
        };

        let ch_num = get_val("Channel_Num")
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        let mut groups = [0u8; 4];
        for (i, slot) in ["Slot1", "Slot2", "Slot3", "Slot4"].iter().enumerate() {
            if let Some(val_str) = get_val(slot) {
                groups[i] = match val_str.as_str() {
                    "A" => 1,
                    "B" => 2,
                    "C" => 3,
                    "D" => 4,
                    "E" => 5,
                    "F" => 6,
                    "G" => 7,
                    "H" => 8,
                    "I" => 9,
                    "J" => 10,
                    "K" => 11,
                    "L" => 12,
                    "M" => 13,
                    "N" => 14,
                    "O" => 15,
                    "P" => 16,
                    "Q" => 17,
                    "R" => 18,
                    "S" => 19,
                    "T" => 20,
                    "U" => 21,
                    "V" => 22,
                    "W" => 23,
                    "X" => 24,
                    "Y" => 25,
                    "Z" => 26,
                    s => s.parse::<u8>().unwrap_or(0),
                };
            }
        }

        Channel {
            channel_num: ch_num,
            name: get_val("Name").cloned().unwrap_or_default(),
            rx_freq: get_val("RX")
                .or_else(|| get_val("RX_Freq"))
                .cloned()
                .unwrap_or_else(|| "0".to_string()),
            tx_freq: get_val("TX")
                .or_else(|| get_val("TX_Freq"))
                .cloned()
                .unwrap_or_else(|| "0".to_string()),
            rx_tone: get_val("RX_Tone")
                .cloned()
                .unwrap_or_else(|| "Off".to_string()),
            tx_tone: get_val("TX_Tone")
                .cloned()
                .unwrap_or_else(|| "Off".to_string()),
            power: get_val("TX_Power")
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(0),
            bandwidth: get_val("Bandwidth")
                .cloned()
                .unwrap_or_else(|| "Wide".to_string()),
            modulation: get_val("Modulation")
                .cloned()
                .unwrap_or_else(|| "FM".to_string()),
            reverse: get_val("Reversed")
                .map(|s| s.to_lowercase() == "true")
                .unwrap_or(false),
            busy_lock: get_val("BusyLock")
                .map(|s| s.to_lowercase() == "true")
                .unwrap_or(false),
            groups,
            ptt_id: match get_val("PTTID").map(|s| s.as_str()).unwrap_or("Off") {
                "Off" => 0,
                "BOT" => 1,
                "EOT" => 2,
                "Both" => 3,
                _ => 0,
            },
            position: if get_val("Active")
                .map(|s| s.to_lowercase() == "true")
                .unwrap_or(true)
            {
                1
            } else {
                0
            },
        }
    }

    #[test]
    fn test_channel_csv_deserialization() {
        let data = "Channel_Num,Active,Name,RX,TX,RX_Tone,TX_Tone,TX_Power,Slot1,Slot2,Slot3,Slot4,Bandwidth,Modulation,BusyLock,Reversed,PTTID\n\
                    1,True,W6SLG,145.31000,144.71000,Off,94.8,255,A,,,,Wide,FM,False,False,Off";
        let mut rdr = Reader::from_reader(data.as_bytes());
        let mut iter = rdr.deserialize::<HashMap<String, String>>();
        let row = iter.next().unwrap().unwrap();
        let result = parse_row(row);

        assert_eq!(result.channel_num, 1);
        assert_eq!(result.name, "W6SLG");
        assert_eq!(result.rx_freq, "145.31000");
        assert_eq!(result.tx_freq, "144.71000");
        assert_eq!(result.rx_tone, "Off");
        assert_eq!(result.tx_tone, "94.8");
        assert_eq!(result.power, 255);
        assert_eq!(result.position, 1); // Active=True -> position=1
    }

    #[test]
    fn test_channel_csv_inactive() {
        let data = "Channel_Num,Active,Name,RX,TX,RX_Tone,TX_Tone,TX_Power,Slot1,Slot2,Slot3,Slot4,Bandwidth,Modulation,BusyLock,Reversed,PTTID\n\
                    1,False,TEST,145.31000,144.71000,Off,Off,255,,,,,Wide,FM,False,False,Off";
        let mut rdr = Reader::from_reader(data.as_bytes());
        let mut iter = rdr.deserialize::<HashMap<String, String>>();
        let row = iter.next().unwrap().unwrap();
        let result = parse_row(row);

        assert_eq!(result.channel_num, 1);
        assert_eq!(result.name, "TEST");
        assert_eq!(result.position, 0); // Active=False -> position=0
    }

    #[test]
    fn test_channel_lowercase_csv_deserialization() {
        let data = "channel_num,name,rx_freq,tx_freq\n1,TEST,144.0,144.0";
        let mut rdr = Reader::from_reader(data.as_bytes());
        let mut iter = rdr.deserialize::<HashMap<String, String>>();
        let row = iter.next().unwrap().unwrap();
        let result = parse_row(row);

        assert_eq!(result.channel_num, 1);
        assert_eq!(result.name, "TEST");
        assert_eq!(result.rx_freq, "144.0");
    }

    #[test]
    fn group_label_block_count_covers_full_table() {
        let covered_bytes = GROUP_LABELS_BLOCK_COUNT * BLOCK_SIZE;
        let required_end = GROUP_LABELS_BLOCK_OFFSET + (GROUP_LABEL_COUNT * GROUP_LABEL_SIZE);

        assert!(covered_bytes >= required_end);
        assert_eq!(GROUP_LABELS_BLOCK_COUNT, 4);
    }
}
