use super::radio::RadioProtocol;
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

/// Extract channels from a codeplug
pub fn extract_channels_from_codeplug(data: &[u8], endian: Endianness) -> Vec<Channel> {
    let mut channels = Vec::new();

    // Channels start at block 0, each channel is 32 bytes
    for i in 0..198 {
        let offset = i * BLOCK_SIZE;
        if offset + BLOCK_SIZE <= data.len() {
            let block = &data[offset..offset + BLOCK_SIZE];
            if let Some(channel) = RadioProtocol::parse_channel(block, i as u16, endian) {
                channels.push(channel);
            }
        }
    }

    channels
}

/// Extract settings from a codeplug
pub fn extract_settings_from_codeplug(data: &[u8], endian: Endianness) -> Option<SettingsBlock> {
    let settings_offset = 0x1900;
    if settings_offset + 128 <= data.len() {
        let block = &data[settings_offset..settings_offset + 128];
        Some(RadioProtocol::parse_settings_block(block, endian))
    } else {
        None
    }
}

/// Extract scan presets from a codeplug
pub fn extract_scan_presets_from_codeplug(data: &[u8], endian: Endianness) -> Vec<ScanPreset> {
    let mut presets = Vec::new();
    let start_offset = 0x1B00;

    for i in 0..20 {
        let offset = start_offset + (i * 14);
        if offset + 14 <= data.len() {
            let preset_data = &data[offset..offset + 14];
            let preset = RadioProtocol::parse_scan_preset(preset_data, i as u8, endian);
            presets.push(preset);
        }
    }

    presets
}

/// Extract band plans from a codeplug
pub fn extract_band_plans_from_codeplug(data: &[u8], endian: Endianness) -> Vec<BandPlan> {
    let mut plans = Vec::new();
    let start_offset = 0x1A02;

    for i in 0..20 {
        let offset = start_offset + (i * 10);
        if offset + 10 <= data.len() {
            let plan_data = &data[offset..offset + 10];
            let plan = RadioProtocol::parse_bandplan(plan_data, i as u8, endian);
            plans.push(plan);
        }
    }

    plans
}

/// Extract DTMF presets from a codeplug
pub fn extract_dtmf_presets_from_codeplug(data: &[u8]) -> Vec<DTMFPreset> {
    let mut presets = Vec::new();
    let start_offset = 0x1D00;

    for i in 0..16 {
        let offset = start_offset + (i * 8);
        if offset + 8 <= data.len() {
            let dtmf_data = &data[offset..offset + 8];
            let preset = RadioProtocol::parse_dtmf_preset(dtmf_data, i as u8);
            presets.push(preset);
        }
    }

    presets
}

/// Create a complete codeplug from current radio data
pub fn create_codeplug(
    channels: &[Channel],
    settings: &SettingsBlock,
    scan_presets: &[ScanPreset],
    band_plans: &[BandPlan],
    dtmf_presets: &[DTMFPreset],
    endian: Endianness,
) -> Vec<u8> {
    let mut codeplug = vec![0xFF; EEPROM_SIZE];

    // Write channels (blocks 0-197)
    for channel in channels {
        let offset = channel.channel_num as usize * BLOCK_SIZE;
        if offset + BLOCK_SIZE <= codeplug.len() {
            let channel_data = RadioProtocol::pack_channel(channel, endian);
            codeplug[offset..offset + BLOCK_SIZE].copy_from_slice(&channel_data);
        }
    }

    // Write settings at 0x1900
    let settings_offset = 0x1900;
    let settings_data = RadioProtocol::pack_settings_block(settings, endian);
    codeplug[settings_offset..settings_offset + settings_data.len()]
        .copy_from_slice(&settings_data);

    // Write scan presets (starting at 0x1B00)
    for preset in scan_presets.iter().take(20) {
        let offset = 0x1B00 + (preset.index as usize * 14);
        if offset + 14 <= codeplug.len() {
            let preset_data = RadioProtocol::pack_scan_preset(preset, endian);
            codeplug[offset..offset + preset_data.len()].copy_from_slice(&preset_data);
        }
    }

    // Write band plans (starting at 0x1A02)
    for plan in band_plans.iter().take(20) {
        let offset = 0x1A02 + (plan.index as usize * 10);
        if offset + 10 <= codeplug.len() {
            let plan_data = RadioProtocol::pack_bandplan(plan, endian);
            codeplug[offset..offset + plan_data.len()].copy_from_slice(&plan_data);
        }
    }

    // Write DTMF presets (starting at 0x1D00)
    for preset in dtmf_presets.iter().take(16) {
        let offset = 0x1D00 + (preset.index as usize * 8);
        if offset + 8 <= codeplug.len() {
            let dtmf_data = RadioProtocol::pack_dtmf_preset(preset);
            codeplug[offset..offset + dtmf_data.len()].copy_from_slice(&dtmf_data);
        }
    }

    codeplug
}
