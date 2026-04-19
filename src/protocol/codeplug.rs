use super::radio::{
    BAND_PLAN_RECORD_COUNT, BAND_PLAN_RECORD_SIZE, RadioProtocol, SCAN_PRESET_RECORD_COUNT,
    SCAN_PRESET_RECORD_SIZE,
};
use super::types::*;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Load a codeplug (.nfw) file from disk
pub fn load_codeplug<P: AsRef<Path>>(path: P) -> Result<Vec<u8>> {
    let data = fs::read(path.as_ref()).context("Failed to read codeplug file")?;

    if data.len() != EEPROM_SIZE {
        anyhow::bail!(
            "Invalid codeplug size: expected {} bytes, got {} bytes",
            EEPROM_SIZE,
            data.len()
        );
    }

    Ok(data)
}

/// Save a codeplug (.nfw) file to disk
pub fn save_codeplug<P: AsRef<Path>>(path: P, data: &[u8]) -> Result<()> {
    if data.len() != EEPROM_SIZE {
        anyhow::bail!(
            "Invalid codeplug size: expected {} bytes, got {} bytes",
            EEPROM_SIZE,
            data.len()
        );
    }

    fs::write(path.as_ref(), data).context("Failed to write codeplug file")?;

    Ok(())
}

pub fn infer_endianness_from_codeplug(data: &[u8]) -> Endianness {
    let marker_offset = 240 * BLOCK_SIZE;
    if data.get(marker_offset).copied() == Some(0x57) {
        Endianness::Little
    } else {
        Endianness::Big
    }
}

pub fn infer_channel_endianness_from_codeplug(data: &[u8]) -> Endianness {
    for block_num in 2..10 {
        let offset = block_num * BLOCK_SIZE;
        if offset + 4 > data.len() {
            break;
        }

        let block = &data[offset..offset + BLOCK_SIZE];
        let rx_freq_le = u32::from_le_bytes(block[0..4].try_into().unwrap_or([0; 4]));
        let rx_freq_be = u32::from_be_bytes(block[0..4].try_into().unwrap_or([0; 4]));

        let little_valid = (100_000..=100_000_000).contains(&rx_freq_le);
        let big_valid = (100_000..=100_000_000).contains(&rx_freq_be);

        if little_valid && !big_valid {
            return Endianness::Little;
        }
        if big_valid && !little_valid {
            return Endianness::Big;
        }
        if little_valid && big_valid {
            return Endianness::Little;
        }
    }

    infer_endianness_from_codeplug(data)
}

/// Extract channels from a codeplug
pub fn extract_channels_from_codeplug(data: &[u8], endian: Endianness) -> Vec<Channel> {
    let mut channels = Vec::new();

    // Channels start at EEPROM block 2 and occupy 198 slots.
    for i in 0..198 {
        let offset = (i + 2) * BLOCK_SIZE;
        if offset + BLOCK_SIZE <= data.len() {
            let block = &data[offset..offset + BLOCK_SIZE];
            if let Some(channel) = RadioProtocol::parse_channel(block, (i + 1) as u16, endian) {
                channels.push(channel);
            }
        }
    }

    channels
}

pub fn extract_vfo_memories_from_codeplug(
    data: &[u8],
    endian: Endianness,
) -> Vec<(String, Channel)> {
    let mut memories = Vec::new();

    for (index, slot) in ["A", "B"].iter().enumerate() {
        let offset = index * BLOCK_SIZE;
        if offset + BLOCK_SIZE > data.len() {
            break;
        }

        let block = &data[offset..offset + BLOCK_SIZE];
        if let Some(channel) = RadioProtocol::parse_channel(block, 0, endian) {
            memories.push(((*slot).to_string(), channel));
        }
    }

    memories
}

/// Extract settings from a codeplug
pub fn extract_settings_from_codeplug(data: &[u8], _endian: Endianness) -> Option<SettingsBlock> {
    let settings_offset = 0x1900;
    if settings_offset + 128 <= data.len() {
        let block = &data[settings_offset..settings_offset + 128];
        let settings_endian = RadioProtocol::infer_settings_endianness(block);
        Some(RadioProtocol::parse_settings_block(block, settings_endian))
    } else {
        None
    }
}

/// Extract scan presets from a codeplug
pub fn extract_scan_presets_from_codeplug(data: &[u8], _endian: Endianness) -> Vec<ScanPreset> {
    let mut presets = Vec::new();
    let start_offset = 0x1B00;
    let byte_len = SCAN_PRESET_RECORD_COUNT * SCAN_PRESET_RECORD_SIZE;

    if start_offset + byte_len > data.len() {
        return presets;
    }

    let scan_endian =
        RadioProtocol::infer_scan_preset_endianness(&data[start_offset..start_offset + byte_len]);

    for i in 0..SCAN_PRESET_RECORD_COUNT {
        let offset = start_offset + (i * SCAN_PRESET_RECORD_SIZE);
        if offset + SCAN_PRESET_RECORD_SIZE <= data.len() {
            let preset_data = &data[offset..offset + SCAN_PRESET_RECORD_SIZE];
            let preset = RadioProtocol::parse_scan_preset(preset_data, i as u8, scan_endian);
            presets.push(preset);
        }
    }

    presets
}

/// Extract band plans from a codeplug
pub fn extract_band_plans_from_codeplug(data: &[u8], _endian: Endianness) -> Vec<BandPlan> {
    let mut plans = Vec::new();
    let start_offset = 0x1A02;
    let byte_len = BAND_PLAN_RECORD_COUNT * BAND_PLAN_RECORD_SIZE;

    if start_offset + byte_len > data.len() {
        return plans;
    }

    let band_endian =
        RadioProtocol::infer_bandplan_endianness(&data[start_offset..start_offset + byte_len]);

    for i in 0..BAND_PLAN_RECORD_COUNT {
        let offset = start_offset + (i * BAND_PLAN_RECORD_SIZE);
        if offset + BAND_PLAN_RECORD_SIZE <= data.len() {
            let plan_data = &data[offset..offset + BAND_PLAN_RECORD_SIZE];
            let plan = RadioProtocol::parse_bandplan(plan_data, i as u8, band_endian);
            plans.push(plan);
        }
    }

    plans
}

/// Extract DTMF presets from a codeplug
pub fn extract_dtmf_presets_from_codeplug(data: &[u8]) -> Vec<DTMFPreset> {
    let mut presets = Vec::new();
    let start_offset = 0x1CF0;

    for i in 0..20 {
        let offset = start_offset + (i * 13);
        if offset + 13 <= data.len() {
            let dtmf_data = &data[offset..offset + 13];
            let preset = RadioProtocol::parse_dtmf_preset(dtmf_data, i as u8);
            presets.push(preset);
        }
    }

    presets
}

pub fn extract_group_labels_from_codeplug(data: &[u8]) -> Vec<String> {
    let byte_len = GROUP_LABEL_COUNT * GROUP_LABEL_SIZE;
    if GROUP_LABELS_OFFSET + byte_len > data.len() {
        return vec![String::new(); GROUP_LABEL_COUNT];
    }

    RadioProtocol::parse_group_labels(&data[GROUP_LABELS_OFFSET..GROUP_LABELS_OFFSET + byte_len])
}

/// Create a complete codeplug from current radio data
pub fn create_codeplug(
    channels: &[Channel],
    settings: &SettingsBlock,
    scan_presets: &[ScanPreset],
    band_plans: &[BandPlan],
    dtmf_presets: &[DTMFPreset],
    group_labels: &[String],
    endian: Endianness,
) -> Vec<u8> {
    let mut codeplug = vec![0xFF; EEPROM_SIZE];

    // Channels live in EEPROM blocks 2..=199.
    for channel in channels {
        let offset = (channel.channel_num as usize + 1) * BLOCK_SIZE;
        if offset + BLOCK_SIZE <= codeplug.len() {
            let channel_data = RadioProtocol::pack_channel(channel, endian);
            codeplug[offset..offset + BLOCK_SIZE].copy_from_slice(&channel_data);
        }
    }

    // Write settings at 0x1900
    let settings_offset = 0x1900;
    let settings_data = RadioProtocol::pack_settings_block(settings, Endianness::Big);
    codeplug[settings_offset..settings_offset + settings_data.len()]
        .copy_from_slice(&settings_data);

    // Scan presets use a separate big-endian record layout starting at 0x1B00.
    for preset in scan_presets.iter().take(SCAN_PRESET_RECORD_COUNT) {
        let offset = 0x1B00 + (preset.index as usize * SCAN_PRESET_RECORD_SIZE);
        if offset + SCAN_PRESET_RECORD_SIZE <= codeplug.len() {
            let preset_data = RadioProtocol::pack_scan_preset(preset, Endianness::Big);
            codeplug[offset..offset + preset_data.len()].copy_from_slice(&preset_data);
        }
    }

    // Write band plans (starting at 0x1A02)
    for plan in band_plans.iter().take(BAND_PLAN_RECORD_COUNT) {
        let offset = 0x1A02 + (plan.index as usize * BAND_PLAN_RECORD_SIZE);
        if offset + BAND_PLAN_RECORD_SIZE <= codeplug.len() {
            let plan_data = RadioProtocol::pack_bandplan(plan, Endianness::Big);
            codeplug[offset..offset + plan_data.len()].copy_from_slice(&plan_data);
        }
    }

    // Write DTMF presets (starting at 0x1CF0)
    for preset in dtmf_presets.iter().take(20) {
        let offset = 0x1CF0 + (preset.index as usize * 13);
        if offset + 13 <= codeplug.len() {
            let dtmf_data = RadioProtocol::pack_dtmf_preset(preset);
            codeplug[offset..offset + dtmf_data.len()].copy_from_slice(&dtmf_data);
        }
    }

    let group_label_data = RadioProtocol::pack_group_labels(group_labels);
    codeplug[GROUP_LABELS_OFFSET..GROUP_LABELS_OFFSET + group_label_data.len()]
        .copy_from_slice(&group_label_data);

    codeplug
}
