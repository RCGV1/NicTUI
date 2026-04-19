use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::protocol::Channel;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelFileFormat {
    Csv,
    Json,
}

pub fn infer_channel_file_format(path: &Path) -> Result<ChannelFileFormat> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("csv") => Ok(ChannelFileFormat::Csv),
        Some("json") => Ok(ChannelFileFormat::Json),
        _ => bail!(
            "Unsupported channel file format for {}. Use .csv or .json",
            path.display()
        ),
    }
}

pub fn load_channels_from_path(path: &Path) -> Result<Vec<Channel>> {
    let format = infer_channel_file_format(path)?;
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    load_channels_from_reader(reader, format)
}

pub fn load_channels_from_reader<R: Read>(
    reader: R,
    format: ChannelFileFormat,
) -> Result<Vec<Channel>> {
    match format {
        ChannelFileFormat::Csv => load_channels_from_csv(reader),
        ChannelFileFormat::Json => {
            serde_json::from_reader(reader).context("Failed to parse channel JSON payload")
        }
    }
}

pub fn save_channels_to_path(path: &Path, channels: &[Channel]) -> Result<()> {
    let format = infer_channel_file_format(path)?;
    let file =
        File::create(path).with_context(|| format!("Failed to create {}", path.display()))?;
    let writer = BufWriter::new(file);
    save_channels_to_writer(writer, channels, format)
}

pub fn save_channels_to_writer<W: Write>(
    writer: W,
    channels: &[Channel],
    format: ChannelFileFormat,
) -> Result<()> {
    match format {
        ChannelFileFormat::Csv => save_channels_to_csv(writer, channels),
        ChannelFileFormat::Json => {
            serde_json::to_writer_pretty(writer, channels).context("Failed to write channel JSON")
        }
    }
}

fn load_channels_from_csv<R: Read>(reader: R) -> Result<Vec<Channel>> {
    let mut csv_reader = csv::Reader::from_reader(reader);
    let mut channels = Vec::new();

    for result in csv_reader.deserialize::<HashMap<String, String>>() {
        let row = result.context("Failed to parse CSV row")?;
        if let Some(channel) = parse_channel_row(&row) {
            channels.push(channel);
        }
    }

    Ok(channels)
}

fn save_channels_to_csv<W: Write>(writer: W, channels: &[Channel]) -> Result<()> {
    let mut csv_writer = csv::Writer::from_writer(writer);
    for channel in channels {
        csv_writer
            .serialize(channel)
            .context("Failed to write CSV row")?;
    }
    csv_writer.flush().context("Failed to flush CSV writer")
}

fn parse_channel_row(row: &HashMap<String, String>) -> Option<Channel> {
    let get_value = |key: &str| -> Option<&String> {
        row.get(key).or_else(|| {
            let key_lower = key.to_ascii_lowercase();
            row.iter()
                .find(|(candidate, _)| candidate.to_ascii_lowercase() == key_lower)
                .map(|(_, value)| value)
        })
    };

    let channel_num = get_value("Channel_Num")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);

    if channel_num == 0 {
        return None;
    }

    let mut groups = [0u8; 4];
    for (index, slot) in ["Slot1", "Slot2", "Slot3", "Slot4"].iter().enumerate() {
        if let Some(value) = get_value(slot) {
            groups[index] = parse_group_value(value);
        }
    }

    Some(Channel {
        channel_num,
        name: get_value("Name").cloned().unwrap_or_default(),
        rx_freq: get_value("RX")
            .or_else(|| get_value("RX_Freq"))
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        tx_freq: get_value("TX")
            .or_else(|| get_value("TX_Freq"))
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        rx_tone: get_value("RX_Tone")
            .cloned()
            .unwrap_or_else(|| "Off".to_string()),
        tx_tone: get_value("TX_Tone")
            .cloned()
            .unwrap_or_else(|| "Off".to_string()),
        power: get_value("TX_Power")
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0),
        bandwidth: get_value("Bandwidth")
            .cloned()
            .unwrap_or_else(|| "Wide".to_string()),
        modulation: get_value("Modulation")
            .cloned()
            .unwrap_or_else(|| "FM".to_string()),
        reverse: get_value("Reversed")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        busy_lock: get_value("BusyLock")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        groups,
        ptt_id: match get_value("PTTID")
            .map(|value| value.as_str())
            .unwrap_or("Off")
        {
            "Off" => 0,
            "BOT" => 1,
            "EOT" => 2,
            "Both" => 3,
            _ => 0,
        },
        position: if get_value("Active")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(true)
        {
            1
        } else {
            0
        },
    })
}

fn parse_group_value(value: &str) -> u8 {
    match value {
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
        other => other.parse::<u8>().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::{ChannelFileFormat, load_channels_from_reader};

    #[test]
    fn loads_channels_from_case_insensitive_csv_headers() {
        let payload = "channel_num,name,rx_freq,tx_freq\n1,TEST,144.0,145.0\n";
        let channels =
            load_channels_from_reader(payload.as_bytes(), ChannelFileFormat::Csv).unwrap();

        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].channel_num, 1);
        assert_eq!(channels[0].name, "TEST");
        assert_eq!(channels[0].rx_freq, "144.0");
        assert_eq!(channels[0].tx_freq, "145.0");
    }
}
