use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::state::{App, AppEvent, AppMode, MainTab};
use crate::protocol::{BIN_FLASH_BAUD_RATE, BLOCK_SIZE, Channel, EEPROM_SIZE, RadioProtocol};

impl App {
    pub fn select_port(&mut self) {
        if self.ports.is_empty() {
            self.mode = AppMode::Error("No serial ports found".to_string());
            return;
        }

        let port_name = self.ports[self.selected_port_index].clone();
        match RadioProtocol::new(&port_name) {
            Ok(_) => {
                self.protocol_port_name = Some(port_name.clone());
                self.mode = AppMode::Main(MainTab::Channels);
                self.status_message = format!("Connected to {}", port_name);
                self.log(&format!("Opened port {}", port_name));
            }
            Err(e) => {
                self.mode = AppMode::Error(format!("Failed to open port: {}", e));
            }
        }
    }

    pub fn pick_import_file(&mut self) {
        let _ = self.event_tx.send(AppEvent::ShowImportDialog);
    }

    pub fn pick_write_file(&mut self) {
        let _ = self.event_tx.send(AppEvent::ShowWriteDialog);
    }

    pub fn pick_export_file(&mut self) {
        let _ = self.event_tx.send(AppEvent::ShowExportDialog);
    }

    pub fn start_clear_channel(&mut self, channel_num: u16) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Handshaking...".to_string()));
            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let _ = tx.send(AppEvent::Status(format!(
                "Clearing Channel {}...",
                channel_num
            )));
            let blk = (channel_num + 1) as u8;
            let empty_data = vec![0xFFu8; 32];

            match proto.write_block(blk, &empty_data) {
                Ok(true) => {
                    let _ = tx.send(AppEvent::Progress(1.0));
                    let _ = tx.send(AppEvent::Status("Channel cleared successfully".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete);
                }
                Ok(false) => {
                    let _ = tx.send(AppEvent::Error("Radio rejected write".to_string()));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Write failed: {}", e)));
                }
            }
        });
    }

    pub fn start_read_channels(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Handshaking...".to_string()));
            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let mut eeprom = vec![0u8; EEPROM_SIZE];

            // Read settings and channels (blocks 0-199)
            let blocks_to_read = 200;
            for blk in 0..blocks_to_read {
                match proto.read_block(blk as u8) {
                    Ok(data) => {
                        let start = blk * BLOCK_SIZE;
                        eeprom[start..start + BLOCK_SIZE].copy_from_slice(&data);
                        let _ = tx.send(AppEvent::Status(format!(
                            "Reading EEPROM... {}/{}",
                            blk + 1,
                            blocks_to_read
                        )));
                        let _ = tx.send(AppEvent::Progress(
                            (blk + 1) as f64 / (blocks_to_read + 1) as f64,
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!(
                            "Failed to read block {}: {}",
                            blk, e
                        )));
                        return;
                    }
                }
            }

            // Detect Endianness
            let _ = tx.send(AppEvent::Status("Detecting Endianness...".to_string()));
            let endian = match proto.detect_endianness() {
                Ok(e) => e,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to detect endianness: {}",
                        e
                    )));
                    return;
                }
            };

            let mut channels = Vec::new();
            for i in 0..198 {
                let blk = i + 2;
                let start = blk * BLOCK_SIZE;
                let end = start + BLOCK_SIZE;
                if end <= eeprom.len() {
                    if let Some(ch) =
                        RadioProtocol::parse_channel(&eeprom[start..end], (i + 1) as u16, endian)
                    {
                        channels.push(ch);
                    }
                }
                let _ = tx.send(AppEvent::Status(format!(
                    "Reading channels... {}",
                    channels.len()
                )));
                let _ = tx.send(AppEvent::Progress(0.5 + (i as f64 / 198.0) * 0.5));
            }

            let _ = tx.send(AppEvent::Status(format!(
                "Read {} channels",
                channels.len()
            )));
            let _ = tx.send(AppEvent::Progress(1.0));
            let _ = tx.send(AppEvent::ReadChannelsComplete(channels, endian));
        });
    }

    pub fn start_read_presets(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let endian = self.endian;
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Reading Presets...".to_string()));
            let start_blk = 0x1AE0 / 32; // Block 215
            let mut data = Vec::new();
            // Read 5 blocks (160 bytes) for 10 presets × 14 bytes each = 140 bytes
            for i in 0..5 {
                match proto.read_block((start_blk + i) as u8) {
                    Ok(blk) => {
                        data.extend_from_slice(&blk);
                        let _ = tx.send(AppEvent::Progress((i + 1) as f64 / 5.0));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("Failed to read presets: {}", e)));
                        return;
                    }
                }
            }

            let mut presets = Vec::new();
            // Each preset is 14 bytes, parse 10 presets
            for i in 0..10 {
                let start = i * 14;
                let end = start + 14;
                if end <= data.len() {
                    presets.push(RadioProtocol::parse_scan_preset(
                        &data[start..end],
                        i as u8,
                        endian,
                    ));
                }
            }
            let _ = tx.send(AppEvent::ReadPresetsComplete(presets));
        });
    }

    pub fn start_read_bandplan(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let endian = self.endian;
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Reading BandPlan...".to_string()));
            let start_blk = 0x1A00 / 32;
            let mut data = Vec::new();
            for i in 0..7 {
                match proto.read_block((start_blk + i) as u8) {
                    Ok(blk) => {
                        data.extend_from_slice(&blk);
                        let _ = tx.send(AppEvent::Progress((i + 1) as f64 / 7.0));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("Failed to read bandplan: {}", e)));
                        return;
                    }
                }
            }

            let mut plans = Vec::new();
            for i in 0..20 {
                let start = 2 + i * 10;
                let end = start + 10;
                if end <= data.len() {
                    plans.push(RadioProtocol::parse_bandplan(
                        &data[start..end],
                        i as u8,
                        endian,
                    ));
                }
            }
            let _ = tx.send(AppEvent::ReadBandPlanComplete(plans));
        });
    }

    pub fn start_read_dtmf(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Reading DTMF...".to_string()));
            let start_blk = 0x1CF0 / 32;
            let mut data = Vec::new();
            for i in 0..9 {
                match proto.read_block((start_blk + i) as u8) {
                    Ok(blk) => {
                        data.extend_from_slice(&blk);
                        let _ = tx.send(AppEvent::Progress((i + 1) as f64 / 9.0));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("Failed to read DTMF: {}", e)));
                        return;
                    }
                }
            }

            let mut presets = Vec::new();
            for i in 0..20 {
                let start = i * 13;
                let end = start + 13;
                if end <= data.len() {
                    presets.push(RadioProtocol::parse_dtmf_preset(&data[start..end], i as u8));
                }
            }
            let _ = tx.send(AppEvent::ReadDTMFComplete(presets));
        });
    }

    pub fn start_write_dtmf(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let dtmf_presets = self.dtmf_presets.clone();
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Handshaking...".to_string()));
            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let _ = tx.send(AppEvent::Status("Writing DTMF...".to_string()));
            let start_blk = 0x1CF0 / 32;

            let mut data = vec![0xFFu8; 9 * 32];
            for (i, preset) in dtmf_presets.iter().enumerate() {
                if i >= 20 {
                    break;
                }
                let start = i * 13;
                let packed = RadioProtocol::pack_dtmf_preset(preset);
                data[start..start + packed.len()].copy_from_slice(&packed);
            }

            for i in 0..9 {
                let blk_data = &data[i * 32..(i + 1) * 32];
                match proto.write_block((start_blk + i) as u8, blk_data) {
                    Ok(true) => {
                        let _ = tx.send(AppEvent::Progress((i + 1) as f64 / 9.0));
                    }
                    _ => {
                        let _ =
                            tx.send(AppEvent::Error(format!("Failed to write DTMF block {}", i)));
                        return;
                    }
                }
            }

            let _ = tx.send(AppEvent::Status("DTMF written successfully".to_string()));
            let _ = tx.send(AppEvent::WriteComplete);
        });
    }

    pub fn start_read_settings(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Detecting Endianness...".to_string()));
            let endian = match proto.detect_endianness() {
                Ok(e) => e,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to detect endianness: {}",
                        e
                    )));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Reading Settings...".to_string()));
            let start_blk = 0x1900 / 32;
            let mut data = Vec::new();
            for i in 0..4 {
                match proto.read_block((start_blk + i) as u8) {
                    Ok(blk) => {
                        data.extend_from_slice(&blk);
                        let _ = tx.send(AppEvent::Progress((i + 1) as f64 / 4.0));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("Failed to read settings: {}", e)));
                        return;
                    }
                }
            }
            let settings = RadioProtocol::parse_settings_block(&data, endian);
            let _ = tx.send(AppEvent::ReadSettingsComplete(settings, endian));
        });
    }

    pub fn start_write_settings_and_reboot(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let settings = match &self.settings {
            Some(s) => s.clone(),
            None => return,
        };
        let endian = self.endian;
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Handshaking...".to_string()));
            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let _ = tx.send(AppEvent::Status("Writing Settings...".to_string()));
            let start_blk = 0x1900 / 32;
            let data = RadioProtocol::pack_settings_block(&settings, endian);

            for i in 0..4 {
                let blk_data = &data[i * 32..(i + 1) * 32];
                match proto.write_block((start_blk + i) as u8, blk_data) {
                    Ok(true) => {
                        let _ = tx.send(AppEvent::Progress((i + 1) as f64 / 4.0));
                    }
                    _ => {
                        let _ = tx.send(AppEvent::Error(format!(
                            "Failed to write settings block {}",
                            i
                        )));
                        return;
                    }
                }
            }

            let _ = tx.send(AppEvent::Status("Rebooting radio...".to_string()));
            let _ = proto.reboot();

            let _ = tx.send(AppEvent::Status(
                "Settings written successfully".to_string(),
            ));
            let _ = tx.send(AppEvent::WriteComplete);
        });
        self.settings_dirty = false;
    }

    pub fn remote_on(&mut self) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let (key_tx, key_rx) = mpsc::channel();
        self.remote_tx = Some(key_tx);
        self.remote_active = true;
        self.remote_stop_signal.store(false, Ordering::SeqCst);
        let stop_signal = self.remote_stop_signal.clone();

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };
            if proto.remote_on().unwrap_or(false) {
                let _ = tx.send(AppEvent::Status("Remote Mode ON".to_string()));

                // Start listening for packets
                loop {
                    if stop_signal.load(Ordering::SeqCst) {
                        let _ = proto.remote_off();
                        break;
                    }

                    // Check for outgoing keys
                    if let Ok(key) = key_rx.try_recv() {
                        let _ = proto.send_bytes(&[key]);
                        std::thread::sleep(Duration::from_millis(100));
                        let _ = proto.send_bytes(&[0xFF]);
                    }

                    match proto.parse_remote_packet() {
                        Ok(Some(pkt)) => {
                            let _ = tx.send(AppEvent::RemotePacket(pkt));
                        }
                        Ok(None) => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            }
        });
    }

    pub fn remote_off(&mut self) {
        if self.remote_active {
            self.remote_stop_signal.store(true, Ordering::SeqCst);
            self.remote_active = false;
            self.status_message = "Remote Mode OFF".to_string();
        }
    }

    pub fn send_key(&mut self, key_code: u8) {
        if let Some(tx) = &self.remote_tx {
            let _ = tx.send(key_code);
            let tx_clone = tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                let _ = tx_clone.send(0xFF);
            });
        }
    }

    pub fn start_write_channel(&mut self, index: usize) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let channel = match self.channels.get(index) {
            Some(ch) => ch.clone(),
            None => return,
        };
        let endian = self.endian;
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Handshaking...".to_string()));
            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let _ = tx.send(AppEvent::Status(format!(
                "Writing Channel {}...",
                channel.channel_num
            )));
            let blk = (channel.channel_num + 1) as u8; // Channels start at block 2
            let data = RadioProtocol::pack_channel(&channel, endian);

            match proto.write_block(blk, &data) {
                Ok(true) => {
                    let _ = tx.send(AppEvent::Progress(1.0));
                    let _ = tx.send(AppEvent::Status("Channel written successfully".to_string()));
                    let _ = tx.send(AppEvent::WriteComplete);
                }
                Ok(false) => {
                    let _ = tx.send(AppEvent::Error("Radio rejected write".to_string()));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Write failed: {}", e)));
                }
            }
        });
    }

    pub fn start_write_multiple_channels(&mut self, _start_index: usize, reboot: bool) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let channels = self.channels.clone();
        let deleted_channels = self.deleted_channels.clone();
        let endian = self.endian;
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Handshaking...".to_string()));
            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let active_channels: Vec<(u16, Channel)> = channels
                .into_iter()
                .filter(|ch| !deleted_channels.contains(&ch.channel_num))
                .map(|ch| (ch.channel_num, ch))
                .collect();

            let total_operations = active_channels.len() + deleted_channels.len();
            let mut progress = 0.0;

            for (_, ch) in &active_channels {
                let _ = tx.send(AppEvent::Status(format!(
                    "Writing Channel {}...",
                    ch.channel_num
                )));
                let blk = (ch.channel_num + 1) as u8;
                let data = RadioProtocol::pack_channel(ch, endian);
                if !proto.write_block(blk, &data).unwrap_or(false) {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to write channel {}",
                        ch.channel_num
                    )));
                    return;
                }
                progress += 1.0;
                let _ = tx.send(AppEvent::Progress(progress / total_operations as f64));
            }

            for &ch_num in &deleted_channels {
                let _ = tx.send(AppEvent::Status(format!("Clearing Channel {}...", ch_num)));
                let blk = (ch_num + 1) as u8;
                let empty_data = vec![0xFFu8; 32];
                if !proto.write_block(blk, &empty_data).unwrap_or(false) {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to clear channel {}",
                        ch_num
                    )));
                    return;
                }
                progress += 1.0;
                let _ = tx.send(AppEvent::Progress(progress / total_operations as f64));
            }

            if reboot {
                let _ = tx.send(AppEvent::Status("Rebooting radio...".to_string()));
                let _ = proto.reboot();
            }

            let _ = tx.send(AppEvent::Status(
                "Channels updated successfully".to_string(),
            ));
            let _ = tx.send(AppEvent::WriteComplete);
        });
    }

    pub fn start_write_csv_channels(&mut self, path: PathBuf) {
        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => return,
        };
        let tx = self.event_tx.clone();
        let endian = self.endian;
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let mut rdr = match csv::Reader::from_path(&path) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open CSV: {}", e)));
                    return;
                }
            };

            let mut channels = Vec::new();
            for result in rdr.deserialize::<std::collections::HashMap<String, String>>() {
                let row = match result {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("Failed to parse CSV row: {}", e)));
                        return;
                    }
                };

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
                if ch_num == 0 {
                    continue;
                }

                let mut groups = [0u8; 4];
                for (i, slot) in ["Slot1", "Slot2", "Slot3", "Slot4"].iter().enumerate() {
                    if let Some(val_str) = get_val(slot) {
                        groups[i] = match val_str.as_str() {
                            "A" => 10,
                            "B" => 11,
                            "C" => 12,
                            "D" => 13,
                            "E" => 14,
                            "F" => 15,
                            s => s.parse::<u8>().unwrap_or(0),
                        };
                    }
                }

                let ch = Channel {
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
                        0
                    } else {
                        1
                    },
                };
                channels.push(ch);
            }

            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to open port: {}", e)));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status("Handshaking...".to_string()));
            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let total = channels.len();
            for (i, ch) in channels.iter().enumerate() {
                let _ = tx.send(AppEvent::Status(format!(
                    "Writing Channel {}...",
                    ch.channel_num
                )));
                let blk = (ch.channel_num - 1 + 2) as u8;
                let data = RadioProtocol::pack_channel(ch, endian);

                if let Err(e) = proto.write_block(blk, &data) {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to write channel {}: {}",
                        ch.channel_num, e
                    )));
                    return;
                }
                let _ = tx.send(AppEvent::Progress((i + 1) as f64 / total as f64));
            }

            let _ = tx.send(AppEvent::Status(
                "CSV Channels written successfully".to_string(),
            ));
            let _ = tx.send(AppEvent::WriteComplete);
        });
    }

    pub fn load_csv(&mut self, path: &PathBuf) -> Result<()> {
        let mut rdr = csv::Reader::from_path(path)?;
        let mut channels = Vec::new();
        for result in rdr.deserialize::<std::collections::HashMap<String, String>>() {
            let row = match result {
                Ok(r) => r,
                Err(e) => {
                    self.log(&format!("Skipping invalid CSV row: {}", e));
                    continue;
                }
            };

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
            if ch_num == 0 {
                continue;
            }

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
                        s => s.parse::<u8>().unwrap_or(0),
                    };
                }
            }

            let ch = Channel {
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
                    0
                } else {
                    1
                },
            };
            channels.push(ch);
        }
        self.channels = channels;
        self.channel_state.select(Some(0));
        self.log(&format!("Loaded {} channels from CSV", self.channels.len()));
        Ok(())
    }

    pub fn start_import_and_write(&mut self, path: PathBuf) {
        match self.load_csv(&path) {
            Ok(_) => {
                self.start_write_multiple_channels(0, true);
            }
            Err(e) => {
                self.mode = AppMode::Error(format!("Failed to load CSV: {}", e));
            }
        }
    }

    pub fn export_csv(&mut self, path: PathBuf) -> Result<()> {
        let mut wtr = csv::Writer::from_path(&path)?;
        for ch in &self.channels {
            wtr.serialize(ch)?;
        }
        wtr.flush()?;
        self.log(&format!("Exported {} channels to CSV", self.channels.len()));
        Ok(())
    }

    fn show_file_dialog<F>(&mut self, _title: &str, filters: &[(&str, &[&str])], on_select: F)
    where
        F: FnOnce(Option<PathBuf>),
    {
        self.dialog_open = true;
        self.suspend_ui();

        let mut dialog = rfd::FileDialog::new();
        for (name, extensions) in filters {
            dialog = dialog.add_filter(name.to_string(), *extensions);
        }
        let res = dialog.pick_file();

        self.resume_ui();
        self.dialog_open = false;

        on_select(res);
    }

    fn show_save_dialog<F>(
        &mut self,
        _title: &str,
        default_name: &str,
        filters: &[(&str, &[&str])],
        on_select: F,
    ) where
        F: FnOnce(Option<PathBuf>),
    {
        self.dialog_open = true;
        self.suspend_ui();

        let mut dialog = rfd::FileDialog::new().set_file_name(default_name);
        for (name, extensions) in filters {
            dialog = dialog.add_filter(name.to_string(), *extensions);
        }
        let res = dialog.save_file();

        self.resume_ui();
        self.dialog_open = false;

        on_select(res);
    }

    pub fn show_import_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Import CSV", &[("CSV", &["csv"])], move |path| {
            if let Some(p) = path {
                let _ = tx.send(AppEvent::LoadCSV(p));
            }
        });
    }

    pub fn show_export_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_save_dialog(
            "Export Channels",
            "channels.csv",
            &[("CSV", &["csv"])],
            move |path| {
                if let Some(p) = path {
                    let _ = tx.send(AppEvent::ExportCSV(p));
                }
            },
        );
    }

    pub fn show_write_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Write CSV", &[("CSV", &["csv"])], move |path| {
            if let Some(p) = path {
                let _ = tx.send(AppEvent::WriteCSV(p));
            }
        });
    }

    pub fn show_codeplug_import_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Import Codeplug", &[("Codeplug", &["nfw"])], move |path| {
            if let Some(p) = path {
                let _ = tx.send(AppEvent::LoadCodeplug(p));
            }
        });
    }

    pub fn show_codeplug_export_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_save_dialog(
            "Export Codeplug",
            "radio_codeplug.nfw",
            &[("Codeplug", &["nfw"])],
            move |path| {
                if let Some(p) = path {
                    let _ = tx.send(AppEvent::ExportCodeplug(p));
                }
            },
        );
    }

    pub fn start_import_codeplug(&mut self, path: PathBuf) {
        use crate::protocol::codeplug;

        let endian = self.endian;
        let tx = self.event_tx.clone();
        self.mode = AppMode::Reading;
        self.progress = 0.0;

        thread::spawn(move || {
            let _ = tx.send(AppEvent::Status("Starting codeplug import...".to_string()));
            let _ = tx.send(AppEvent::Progress(0.0));
            let _ = tx.send(AppEvent::Status("Reading codeplug file...".to_string()));
            let _ = tx.send(AppEvent::Progress(0.2));

            match codeplug::load_codeplug(&path) {
                Ok(data) => {
                    let _ = tx.send(AppEvent::Status("Parsing codeplug data...".to_string()));
                    let _ = tx.send(AppEvent::Progress(0.4));
                    let channels = codeplug::extract_channels_from_codeplug(&data, endian);
                    let _ = tx.send(AppEvent::Status("Extracting channels...".to_string()));
                    let _ = tx.send(AppEvent::Progress(0.6));
                    let settings = codeplug::extract_settings_from_codeplug(&data, endian);
                    let _ = tx.send(AppEvent::Status("Extracting settings...".to_string()));
                    let _ = tx.send(AppEvent::Progress(0.8));
                    let _scan_presets = codeplug::extract_scan_presets_from_codeplug(&data, endian);
                    let _ = tx.send(AppEvent::Progress(1.0));
                    let _ = tx.send(AppEvent::CodeplugLoaded(path, data));
                    let _ = tx.send(AppEvent::Status(format!(
                        "Codeplug loaded: {} channels, settings {}",
                        channels.len(),
                        if settings.is_some() {
                            "present"
                        } else {
                            "missing"
                        }
                    )));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to load codeplug: {}", e)));
                }
            }
        });
    }

    pub fn start_export_codeplug(&self, path: PathBuf) {
        use crate::protocol::codeplug;

        let has_channels = !self.channels.is_empty();
        let has_settings = self.settings.is_some();

        if !has_channels || !has_settings {
            let _ = self.event_tx.send(AppEvent::Error(
                "Please read channels and settings from radio before exporting codeplug"
                    .to_string(),
            ));
            return;
        }

        let channels = self.channels.clone();
        let settings = self.settings.clone().unwrap();
        let scan_presets = self.scan_presets.clone();
        let band_plans = self.band_plans.clone();
        let dtmf_presets = self.dtmf_presets.clone();
        let endian = self.endian;
        let tx = self.event_tx.clone();

        thread::spawn(move || {
            let _ = tx.send(AppEvent::Status("Creating codeplug...".to_string()));

            let codeplug_data = codeplug::create_codeplug(
                &channels,
                &settings,
                &scan_presets,
                &band_plans,
                &dtmf_presets,
                endian,
            );

            match codeplug::save_codeplug(&path, &codeplug_data) {
                Ok(_) => {
                    let _ = tx.send(AppEvent::Status(format!(
                        "Codeplug exported to {}",
                        path.display()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to save codeplug: {}", e)));
                }
            }
        });
    }

    pub fn start_write_codeplug(&mut self) {
        if self.codeplug_data.is_none() {
            let _ = self.event_tx.send(AppEvent::Error(
                "No codeplug loaded. Please import a codeplug first.".to_string(),
            ));
            return;
        }

        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => {
                let _ = self
                    .event_tx
                    .send(AppEvent::Error("Not connected to radio".to_string()));
                return;
            }
        };

        let codeplug_data = self.codeplug_data.clone().unwrap();
        let tx = self.event_tx.clone();
        self.mode = AppMode::Writing;
        self.progress = 0.0;

        thread::spawn(move || {
            let _ = tx.send(AppEvent::Status("Writing codeplug to radio...".to_string()));

            let mut proto = match RadioProtocol::new(&port_name) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Failed to connect: {}", e)));
                    return;
                }
            };

            if !proto.handshake().unwrap_or(false) {
                let _ = tx.send(AppEvent::Error("Handshake failed".to_string()));
                return;
            }

            let total_blocks = EEPROM_SIZE / BLOCK_SIZE;
            for i in 0..total_blocks {
                let offset = i * BLOCK_SIZE;
                let block = &codeplug_data[offset..offset + BLOCK_SIZE];

                match proto.write_block(i as u8, block) {
                    Ok(_) => {
                        let progress = (i + 1) as f64 / total_blocks as f64;
                        let _ = tx.send(AppEvent::Progress(progress));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!(
                            "Failed to write block {}: {}",
                            i, e
                        )));
                        return;
                    }
                }
            }

            let _ = tx.send(AppEvent::Status(
                "Codeplug written successfully! Rebooting radio...".to_string(),
            ));

            thread::sleep(Duration::from_millis(500));
            let _ = proto.reboot();
            thread::sleep(Duration::from_millis(500));

            let _ = tx.send(AppEvent::WriteComplete);
        });
    }

    pub fn show_bin_firmware_dialog(&mut self) {
        let tx = self.event_tx.clone();
        self.show_file_dialog("Select Firmware", &[("Firmware", &["bin"])], move |path| {
            if let Some(path) = path {
                let _ = tx.send(AppEvent::LoadBinFirmware(path));
            } else {
                let _ = tx.send(AppEvent::Status(
                    "Firmware file selection cancelled.".to_string(),
                ));
            }
        });
    }

    pub fn start_load_bin_firmware(&mut self, path: PathBuf) {
        let tx = self.event_tx.clone();

        thread::spawn(move || {
            let _ = tx.send(AppEvent::Status("Loading firmware file...".to_string()));

            match std::fs::read(&path) {
                Ok(data) => {
                    let _ = tx.send(AppEvent::Status("Firmware file loaded".to_string()));
                    let _ = tx.send(AppEvent::BinFirmwareLoaded(path, data));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "Failed to read firmware file: {}",
                        e
                    )));
                }
            }
        });
    }

    #[allow(unused_assignments)]
    pub fn start_bin_flash(&mut self) {
        let firmware_data = match &self.bin_firmware_data {
            Some(data) => data.clone(),
            None => {
                self.mode = AppMode::Error(
                    "No firmware loaded. Press 'i' to import a firmware file.".to_string(),
                );
                return;
            }
        };

        let port_name = match &self.protocol_port_name {
            Some(p) => p.clone(),
            None => {
                self.mode = AppMode::Error(
                    "Not connected to a port. Please select a port first.".to_string(),
                );
                return;
            }
        };

        let tx = self.event_tx.clone();
        self.progress = 0.0;
        let _ = tx.send(AppEvent::Status(
            "Waiting for radio handshake (0xA5)...".to_string(),
        ));
        self.mode = AppMode::BinFlashing;

        thread::spawn(move || {
            const INIT_SEQUENCE: [u8; 36] = [
                0xA0, 0xEE, 0x74, 0x71, 0x07, 0x74, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
                0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
                0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
            ];

            let rounded_len = ((firmware_data.len() + 31) / 32) * 32;
            if rounded_len > 0xf800 {
                let _ = tx.send(AppEvent::BinFlashFailed(
                    "Firmware file too large".to_string(),
                ));
                return;
            }
            let last_block = (rounded_len / 32) as u16;

            let _ = tx.send(AppEvent::Status("Opening serial port...".to_string()));

            let mut proto = match RadioProtocol::new_with_baud(&port_name, BIN_FLASH_BAUD_RATE) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::BinFlashFailed(format!(
                        "Failed to open port: {}",
                        e
                    )));
                    return;
                }
            };

            let _ = tx.send(AppEvent::Status(
                "Turn OFF radio, hold PTT(H3) or Flashlight(H8), turn ON radio with button held"
                    .to_string(),
            ));

            let mut detected = false;
            let mut flashing = false;
            let mut block: u16 = 0;
            let mut need_to_send_block = true;
            let mut last_block_sent = false;
            let mut a5_count = 0;
            let init_sent_time = std::time::Instant::now();
            let mut block_send_time = std::time::Instant::now();

            let start_time = std::time::Instant::now();
            let timeout_secs = 60;

            loop {
                if start_time.elapsed().as_secs() > timeout_secs {
                    let _ = tx.send(AppEvent::BinFlashFailed(
                        "Timeout waiting for radio".to_string(),
                    ));
                    return;
                }

                if flashing && need_to_send_block && block <= last_block {
                    let is_last = block == last_block;
                    let _ = tx.send(AppEvent::Status(format!(
                        "Flashing block: {} / {}",
                        block, last_block
                    )));

                    let mut packet = [0u8; 36];
                    packet[0] = if is_last { 0xA2 } else { 0xA1 };
                    packet[1] = ((block >> 8) & 0xFF) as u8;
                    packet[2] = (block & 0xFF) as u8;
                    let start_idx = (block as usize) * 32;
                    if start_idx + 32 <= firmware_data.len() {
                        packet[4..36].copy_from_slice(&firmware_data[start_idx..start_idx + 32]);
                    } else {
                        packet[4..4 + (firmware_data.len() - start_idx)]
                            .copy_from_slice(&firmware_data[start_idx..]);
                    }
                    let mut checksum: u8 = 0;
                    for i in 4..36 {
                        checksum = checksum.wrapping_add(packet[i]);
                    }
                    packet[3] = checksum;

                    if let Err(e) = proto.send_bytes(&packet) {
                        let _ = tx.send(AppEvent::BinFlashFailed(format!("Send failed: {}", e)));
                        return;
                    }

                    let progress = (block + 1) as f64 / (last_block as f64 + 1.0);
                    let _ = tx.send(AppEvent::Progress(progress));

                    block += 1;
                    need_to_send_block = false;
                    block_send_time = std::time::Instant::now();

                    if is_last {
                        last_block_sent = true;
                    }
                }

                if last_block_sent
                    && !need_to_send_block
                    && block_send_time.elapsed() > Duration::from_millis(500)
                {
                    let _ = tx.send(AppEvent::Status(
                        "Flashing complete! Closing port...".to_string(),
                    ));
                    let _ = tx.send(AppEvent::Progress(1.0));

                    drop(proto);

                    std::thread::sleep(Duration::from_millis(200));

                    let _ = tx.send(AppEvent::Status("SUCCESS! Firmware flashed.\n\nTurn radio OFF then ON to boot new firmware.".to_string()));

                    let _ = tx.send(AppEvent::BinFlashComplete);
                    return;
                }

                if flashing
                    && !need_to_send_block
                    && !last_block_sent
                    && block_send_time.elapsed() > Duration::from_millis(500)
                {
                    block -= 1;
                    need_to_send_block = true;
                }

                match proto.read_byte() {
                    Ok(Some(byte)) => {
                        if byte == 0xA5 {
                            a5_count += 1;
                            if !detected && a5_count >= 3 {
                                detected = true;
                                let _ = tx.send(AppEvent::Status(
                                    "Handshake detected, sending init sequence...".to_string(),
                                ));
                                if let Err(e) = proto.send_bytes(&INIT_SEQUENCE) {
                                    let _ = tx.send(AppEvent::BinFlashFailed(format!(
                                        "Send failed: {}",
                                        e
                                    )));
                                    return;
                                }
                                a5_count = 0;
                            }
                        } else if byte == 0xA3 {
                            if detected
                                && !flashing
                                && init_sent_time.elapsed() > Duration::from_millis(100)
                            {
                                flashing = true;
                                let _ = tx.send(AppEvent::Status(
                                    "Radio ready, starting flash...".to_string(),
                                ));
                            } else if flashing && !need_to_send_block && !last_block_sent {
                                need_to_send_block = true;
                            }
                        }
                    }
                    Ok(None) => {
                        if detected
                            && !flashing
                            && init_sent_time.elapsed() > Duration::from_millis(400)
                        {
                            flashing = true;
                            let _ = tx.send(AppEvent::Status(
                                "Radio ready (timeout), starting flash...".to_string(),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::BinFlashFailed(format!("Read error: {}", e)));
                        return;
                    }
                }
            }
        });
    }
}
