use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::protocol::{Channel, GROUP_LABEL_COUNT, GROUP_LABEL_TEXT_SIZE};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelFileFormat {
    Csv,
    Json,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChannelFileDocument {
    pub channels: Vec<Channel>,
    // Explicit inactive slots; write flows must pass these to deletion handling.
    #[serde(default)]
    pub inactive_channels: Vec<u16>,
    #[serde(default)]
    pub group_labels: Option<Vec<String>>,
}

pub fn is_populated_channel(channel: &Channel) -> bool {
    channel
        .rx_freq
        .parse::<f64>()
        .is_ok_and(|rx| (18.0..=1300.0).contains(&rx))
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
    Ok(load_channel_document_from_path(path)?.channels)
}

pub fn load_channel_document_from_path(path: &Path) -> Result<ChannelFileDocument> {
    let format = infer_channel_file_format(path)?;
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    load_channel_document_from_reader(reader, format)
}

pub fn load_channels_from_reader<R: Read>(
    reader: R,
    format: ChannelFileFormat,
) -> Result<Vec<Channel>> {
    Ok(load_channel_document_from_reader(reader, format)?.channels)
}

pub fn load_channel_document_from_reader<R: Read>(
    reader: R,
    format: ChannelFileFormat,
) -> Result<ChannelFileDocument> {
    match format {
        ChannelFileFormat::Csv => load_channels_from_csv(reader),
        ChannelFileFormat::Json => {
            #[derive(Deserialize)]
            #[serde(untagged)]
            enum JsonDocument {
                Channels(Vec<Channel>),
                Document(ChannelFileDocument),
            }
            let document = match serde_json::from_reader(reader)
                .context("Failed to parse channel JSON payload")?
            {
                JsonDocument::Channels(channels) => ChannelFileDocument {
                    channels,
                    ..Default::default()
                },
                JsonDocument::Document(document) => document,
            };
            validate_document_metadata(&document)?;
            Ok(document)
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
    mut writer: W,
    channels: &[Channel],
    format: ChannelFileFormat,
) -> Result<()> {
    match format {
        ChannelFileFormat::Csv => save_channels_to_csv(&mut writer, channels, &[], None),
        ChannelFileFormat::Json => serde_json::to_writer_pretty(&mut writer, channels)
            .context("Failed to write channel JSON"),
    }?;
    writer.flush().context("Failed to flush channel output")
}

pub fn save_channel_document_to_path(path: &Path, document: &ChannelFileDocument) -> Result<()> {
    let format = infer_channel_file_format(path)?;
    validate_document_metadata(document)?;
    let file =
        File::create(path).with_context(|| format!("Failed to create {}", path.display()))?;
    save_channel_document_to_writer(BufWriter::new(file), document, format)
}

pub fn save_channel_document_to_writer<W: Write>(
    mut writer: W,
    document: &ChannelFileDocument,
    format: ChannelFileFormat,
) -> Result<()> {
    validate_document_metadata(document)?;
    match format {
        ChannelFileFormat::Csv => save_channels_to_csv(
            &mut writer,
            &document.channels,
            &document.inactive_channels,
            document.group_labels.as_deref(),
        ),
        ChannelFileFormat::Json => serde_json::to_writer_pretty(&mut writer, document)
            .context("Failed to write channel JSON"),
    }?;
    writer.flush().context("Failed to flush channel output")
}

fn load_channels_from_csv<R: Read>(reader: R) -> Result<ChannelFileDocument> {
    let mut csv_reader = csv::Reader::from_reader(reader);
    let mut document = ChannelFileDocument::default();
    let mut seen = HashSet::new();

    for (index, result) in csv_reader
        .deserialize::<HashMap<String, String>>()
        .enumerate()
    {
        let row = result.with_context(|| format!("Failed to parse CSV row {}", index + 2))?;
        (|| -> Result<()> {
            let number = get_value(&row, "Channel_Num")
                .context("Missing Channel_Num column")?
                .trim()
                .parse::<u16>()
                .context("Channel_Num must be an integer from 1 to 198")?;
            if number == 0
                && get_value(&row, "Active")
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("GroupLabels"))
            {
                if document.group_labels.is_some() {
                    bail!("Duplicate GroupLabels metadata row");
                }
                if !row_fields_are_blank(&row, &["Channel_Num", "Active", "Name"]) {
                    bail!("GroupLabels row must not contain channel fields");
                }
                let text = get_value(&row, "Name").context("Missing GroupLabels name field")?;
                let width = GROUP_LABEL_COUNT * GROUP_LABEL_TEXT_SIZE;
                if text.len() > width || !is_printable_ascii(text) {
                    bail!("GroupLabels must contain at most {width} printable ASCII bytes");
                }
                let padded = format!("{text:width$}");
                document.group_labels = Some(
                    padded
                        .as_bytes()
                        .as_chunks::<GROUP_LABEL_TEXT_SIZE>()
                        .0
                        .iter()
                        .map(|label| String::from_utf8_lossy(label).trim().to_string())
                        .collect(),
                );
                return Ok(());
            }
            if !(1..=198).contains(&number) {
                bail!("Channel_Num must be from 1 to 198");
            }
            if !seen.insert(number) {
                bail!("Channel {number} appears more than once");
            }
            let active = parse_bool(get_value(&row, "Active"), true, "Active")?;
            let position = parse_position(&row)?;
            // Only fully blank inactive payloads bypass populated-channel validation.
            if !active && row_fields_are_blank(&row, &["Channel_Num", "Active", "Position"]) {
                document.inactive_channels.push(number);
            } else {
                let mut channel = parse_channel_row(&row, number)?;
                channel.position = position;
                crate::device::validate_channels(std::slice::from_ref(&channel))?;
                if active {
                    document.channels.push(channel);
                } else {
                    document.inactive_channels.push(number);
                }
            }
            Ok(())
        })()
        .with_context(|| format!("Invalid CSV row {}", index + 2))?;
    }

    Ok(document)
}

fn save_channels_to_csv<W: Write>(
    writer: W,
    channels: &[Channel],
    inactive_channels: &[u16],
    group_labels: Option<&[String]>,
) -> Result<()> {
    let mut csv_writer = csv::Writer::from_writer(writer);
    csv_writer.write_record([
        "Channel_Num",
        "Active",
        "Name",
        "RX",
        "TX",
        "RX_Tone",
        "TX_Tone",
        "TX_Power",
        "Slot1",
        "Slot2",
        "Slot3",
        "Slot4",
        "Bandwidth",
        "Modulation",
        "BusyLock",
        "Reversed",
        "PTTID",
        "Position",
    ])?;
    for channel in channels {
        if !is_populated_channel(channel) {
            bail!(
                "Channel {} has no populated RX frequency",
                channel.channel_num
            );
        }
        crate::device::validate_channels(std::slice::from_ref(channel))?;
        csv_writer
            .write_record([
                channel.channel_num.to_string(),
                is_populated_channel(channel).to_string(),
                channel.name.clone(),
                channel.rx_freq.clone(),
                channel.tx_freq.clone(),
                channel.rx_tone.clone(),
                channel.tx_tone.clone(),
                channel.power.to_string(),
                group_csv_value(channel.groups[0]),
                group_csv_value(channel.groups[1]),
                group_csv_value(channel.groups[2]),
                group_csv_value(channel.groups[3]),
                channel.bandwidth.clone(),
                channel.modulation.clone(),
                channel.busy_lock.to_string(),
                channel.reverse.to_string(),
                channel.ptt_id.to_string(),
                channel.position.to_string(),
            ])
            .context("Failed to write CSV row")?;
    }
    for number in inactive_channels {
        let mut row = vec![String::new(); 18];
        row[0] = number.to_string();
        row[1] = "false".to_string();
        csv_writer.write_record(row)?;
    }
    if let Some(labels) = group_labels {
        let mut row = vec![String::new(); 18];
        row[0] = "0".to_string();
        row[1] = "GroupLabels".to_string();
        row[2] = (0..GROUP_LABEL_COUNT)
            .map(|index| {
                format!(
                    "{:<width$}",
                    labels.get(index).map_or("", String::as_str),
                    width = GROUP_LABEL_TEXT_SIZE
                )
            })
            .collect();
        csv_writer.write_record(row)?;
    }
    csv_writer.flush().context("Failed to flush CSV writer")
}

fn get_value<'a>(row: &'a HashMap<String, String>, key: &str) -> Option<&'a String> {
    row.get(key).or_else(|| {
        row.iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    })
}

fn row_fields_are_blank(row: &HashMap<String, String>, except: &[&str]) -> bool {
    row.iter().all(|(key, value)| {
        except.iter().any(|name| key.eq_ignore_ascii_case(name)) || value.trim().is_empty()
    })
}

fn parse_position(row: &HashMap<String, String>) -> Result<u8> {
    match get_value(row, "Position") {
        Some(value) if !value.trim().is_empty() => match value.trim() {
            "0" => Ok(0),
            "1" => Ok(1),
            _ => bail!("Position must be 0 or 1"),
        },
        _ => Ok(0),
    }
}

fn group_csv_value(group: u8) -> String {
    if group == 0 {
        String::new()
    } else {
        char::from(b'A' + group - 1).to_string()
    }
}

fn is_printable_ascii(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}

fn validate_document_metadata(document: &ChannelFileDocument) -> Result<()> {
    let mut seen = HashSet::new();
    for number in document
        .channels
        .iter()
        .map(|channel| channel.channel_num)
        .chain(document.inactive_channels.iter().copied())
    {
        if !(1..=198).contains(&number) || !seen.insert(number) {
            bail!("Channel numbers must be unique and from 1 to 198");
        }
    }
    if let Some(labels) = &document.group_labels
        && (labels.len() > GROUP_LABEL_COUNT
            || labels
                .iter()
                .any(|label| label.len() > GROUP_LABEL_TEXT_SIZE || !is_printable_ascii(label)))
    {
        bail!(
            "GroupLabels accepts at most {GROUP_LABEL_COUNT} labels of {GROUP_LABEL_TEXT_SIZE} printable ASCII bytes"
        );
    }
    Ok(())
}

fn parse_channel_row(row: &HashMap<String, String>, channel_num: u16) -> Result<Channel> {
    let get_value = |key: &str| get_value(row, key);

    let mut groups = [0u8; 4];
    for (index, slot) in ["Slot1", "Slot2", "Slot3", "Slot4"].iter().enumerate() {
        if let Some(value) = get_value(slot) {
            groups[index] = parse_group_value(value)?;
        }
    }

    Ok(Channel {
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
        power: match get_value("TX_Power").or_else(|| get_value("power")) {
            Some(value) => value
                .trim()
                .parse::<u8>()
                .context("TX_Power must be from 0 to 255")?,
            None => 0,
        },
        bandwidth: get_value("Bandwidth")
            .cloned()
            .unwrap_or_else(|| "Wide".to_string()),
        modulation: get_value("Modulation")
            .cloned()
            .unwrap_or_else(|| "FM".to_string()),
        reverse: parse_bool(
            get_value("Reversed").or_else(|| get_value("reverse")),
            false,
            "Reversed",
        )?,
        busy_lock: parse_bool(
            get_value("BusyLock").or_else(|| get_value("busy_lock")),
            false,
            "BusyLock",
        )?,
        groups,
        ptt_id: match get_value("PTTID")
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
            .unwrap_or("off")
        {
            "off" | "0" => 0,
            "bot" | "1" => 1,
            "eot" | "2" => 2,
            "both" | "3" => 3,
            other => bail!("Invalid PTTID {other:?}: use Off, BOT, EOT, Both, or 0-3"),
        },
        position: 0,
    })
}

fn parse_bool(value: Option<&String>, default: bool, field: &str) -> Result<bool> {
    match value
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        None => Ok(default),
        Some("true" | "1") => Ok(true),
        Some("false" | "0") => Ok(false),
        Some(other) => bail!("Invalid {field} value {other:?}: use true or false"),
    }
}

fn parse_group_value(value: &str) -> Result<u8> {
    // Numeric slots are legacy NicTUI's internal 0=empty, 1=A ... 15=O IDs,
    // never the programmer's zero-based enum ordinals. New exports use letters.
    let normalized = value.trim().to_ascii_uppercase();
    let group = match normalized.as_str() {
        "" => 0,
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
        other => other.parse::<u8>().context("Invalid group slot")?,
    };
    if group > 15 {
        bail!("Group slot must be 0-15 or A-O; larger values do not fit the radio field");
    }
    Ok(group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Endianness, RadioProtocol};

    const PROGRAMMER_HEADER: &str = "Channel_Num,Active,Name,RX,TX,RX_Tone,TX_Tone,TX_Power,Slot1,Slot2,Slot3,Slot4,Bandwidth,Modulation,BusyLock,Reversed,PTTID\r\n";
    const PROGRAMMER_CHANNEL: &str =
        "1,True,LOCAL,145.35000,144.75000,D023N,94.8,127,A,O,,,Narrow,FM,False,True,BoT\r\n";

    fn load_csv(payload: &str) -> Result<ChannelFileDocument> {
        load_channel_document_from_reader(payload.as_bytes(), ChannelFileFormat::Csv)
    }

    fn sample_channel() -> Channel {
        load_csv(&format!("{PROGRAMMER_HEADER}{PROGRAMMER_CHANNEL}"))
            .unwrap()
            .channels
            .remove(0)
    }

    fn document_roundtrip(
        document: &ChannelFileDocument,
        format: ChannelFileFormat,
    ) -> ChannelFileDocument {
        let mut bytes = Vec::new();
        save_channel_document_to_writer(&mut bytes, document, format).unwrap();
        load_channel_document_from_reader(bytes.as_slice(), format).unwrap()
    }

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

    #[test]
    fn csv_roundtrip_preserves_every_channel_field() {
        let channel = Channel {
            channel_num: 198,
            name: "TEST,NAME".into(),
            rx_freq: "145.35000".into(),
            tx_freq: "144.75000".into(),
            rx_tone: "D023n".into(),
            tx_tone: "94.8".into(),
            power: 255,
            bandwidth: "Narrow".into(),
            modulation: "Auto".into(),
            reverse: true,
            busy_lock: true,
            groups: [15, 3, 2, 1],
            ptt_id: 3,
            position: 0,
        };
        let mut bytes = Vec::new();
        save_channels_to_writer(
            &mut bytes,
            std::slice::from_ref(&channel),
            ChannelFileFormat::Csv,
        )
        .unwrap();
        let decoded = load_channels_from_reader(bytes.as_slice(), ChannelFileFormat::Csv).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded[0]).unwrap(),
            serde_json::to_value(channel).unwrap()
        );
    }

    #[test]
    fn populated_rx_is_active_independently_of_raw_position_bit() {
        for position in [0, 1] {
            let mut channel = sample_channel();
            channel.position = position;
            let original_raw = RadioProtocol::pack_channel(&channel, Endianness::Big);
            let mut bytes = Vec::new();
            save_channels_to_writer(&mut bytes, &[channel], ChannelFileFormat::Csv).unwrap();
            let mut csv = csv::Reader::from_reader(bytes.as_slice());
            let row = csv
                .deserialize::<HashMap<String, String>>()
                .next()
                .unwrap()
                .unwrap();
            assert_eq!(row["Active"], "true");
            assert_eq!(row["Position"], position.to_string());
            let document =
                load_channel_document_from_reader(bytes.as_slice(), ChannelFileFormat::Csv)
                    .unwrap();
            assert!(document.inactive_channels.is_empty());
            assert_eq!(
                RadioProtocol::pack_channel(&document.channels[0], Endianness::Big),
                original_raw
            );
        }
        let mut channel = sample_channel();
        for rx in ["18", "1300"] {
            channel.rx_freq = rx.into();
            assert!(is_populated_channel(&channel));
        }
        for rx in ["", "0", "17.99999", "1300.00001", "NaN", "inf", "bad"] {
            channel.rx_freq = rx.into();
            assert!(!is_populated_channel(&channel), "{rx}");
        }
    }

    #[test]
    fn csv_exports_letters_and_empty_unused_slots() {
        let mut channel = sample_channel();
        channel.groups = [1, 0, 15, 0];
        let mut bytes = Vec::new();
        save_channels_to_writer(&mut bytes, &[channel], ChannelFileFormat::Csv).unwrap();
        let mut csv = csv::Reader::from_reader(bytes.as_slice());
        let row = csv
            .deserialize::<HashMap<String, String>>()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(
            [&row["Slot1"], &row["Slot2"], &row["Slot3"], &row["Slot4"]],
            ["A", "", "O", ""]
        );
        assert_eq!(
            load_channels_from_reader(bytes.as_slice(), ChannelFileFormat::Csv).unwrap()[0].groups,
            [1, 0, 15, 0]
        );
    }

    #[test]
    fn legacy_numeric_slots_are_not_shifted() {
        let payload = format!(
            "{PROGRAMMER_HEADER}1,True,OLD,145.35000,144.75000,Off,Off,127,1,0,15,2,Wide,FM,False,False,0\r\n"
        );
        let document = load_csv(&payload).unwrap();
        assert!(document.inactive_channels.is_empty());
        assert_eq!(document.channels[0].groups, [1, 0, 15, 2]);
        assert_eq!(document.channels[0].position, 0);
        let decoded = document_roundtrip(&document, ChannelFileFormat::Csv);
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(document).unwrap()
        );
        for group in 0..=15 {
            assert_eq!(parse_group_value(&group.to_string()).unwrap(), group);
            assert_eq!(parse_group_value(&group_csv_value(group)).unwrap(), group);
        }
    }

    #[test]
    fn inactive_populated_rows_preserve_explicit_deletion_intent() {
        let payload = format!(
            "{PROGRAMMER_HEADER}{}",
            PROGRAMMER_CHANNEL.replacen(",True,", ",False,", 1)
        );
        let document = load_csv(&payload).unwrap();
        assert!(document.channels.is_empty());
        assert_eq!(document.inactive_channels, [1]);
        let decoded = document_roundtrip(&document, ChannelFileFormat::Csv);
        assert!(decoded.channels.is_empty());
        assert_eq!(decoded.inactive_channels, [1]);
    }

    #[test]
    fn csv_active_never_sets_raw_position() {
        let document = load_csv(&format!("{PROGRAMMER_HEADER}{PROGRAMMER_CHANNEL}")).unwrap();
        assert_eq!(document.channels[0].position, 0);
        let document = load_csv("Channel_Num,Active,RX,TX,Position\n1,True,145,145,1\n").unwrap();
        assert_eq!(document.channels[0].position, 1);
    }

    #[test]
    fn complete_programmer_export_preserves_inactive_slots_and_group_metadata() {
        let mut payload = format!("{PROGRAMMER_HEADER}{PROGRAMMER_CHANNEL}");
        for number in 2..=198 {
            payload.push_str(&format!("{number},False,{}\r\n", ",".repeat(14)));
        }
        let mut labels = vec![String::new(); GROUP_LABEL_COUNT];
        labels[0] = "LOCAL".into();
        labels[1] = "OPS,1".into();
        labels[14] = "LAST".into();
        let flat: String = labels.iter().map(|label| format!("{label:<5}")).collect();
        payload.push_str(&format!("0,GroupLabels,\"{flat}\"{}\r\n", ",".repeat(14)));
        let document = load_csv(&payload).unwrap();
        assert_eq!(document.channels.len(), 1);
        assert_eq!(document.channels[0].groups, [1, 15, 0, 0]);
        assert_eq!(document.inactive_channels, (2..=198).collect::<Vec<_>>());
        assert_eq!(document.group_labels.as_ref().unwrap(), &labels);
        for format in [ChannelFileFormat::Csv, ChannelFileFormat::Json] {
            assert_eq!(
                serde_json::to_value(document_roundtrip(&document, format)).unwrap(),
                serde_json::to_value(&document).unwrap()
            );
        }
        let channels =
            load_channels_from_reader(payload.as_bytes(), ChannelFileFormat::Csv).unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].channel_num, 1);
    }

    #[test]
    fn metadata_and_inactive_rows_are_case_insensitive_and_not_channels() {
        let document =
            load_csv("channel_num,active,name,tx_power\n198,fAlSe,,\n0,gRoUpLaBeLs,LOCAL,\n")
                .unwrap();
        assert!(document.channels.is_empty());
        assert_eq!(document.inactive_channels, [198]);
        let labels = document.group_labels.unwrap();
        assert_eq!(labels.len(), GROUP_LABEL_COUNT);
        assert_eq!(labels[0], "LOCAL");
        assert!(labels[1..].iter().all(String::is_empty));
        let document = load_csv("Channel_Num,Active,Name\n0,GroupLabels,\n").unwrap();
        assert!(document.group_labels.unwrap().iter().all(String::is_empty));
    }

    #[test]
    fn inactive_or_metadata_markers_do_not_hide_malformed_populated_rows() {
        for (field, value) in [
            ("RX", "bad"),
            ("RX", "0"),
            ("RX", "NaN"),
            ("TX", "bad"),
            ("TX_Power", ""),
            ("TX_Power", "256"),
            ("Slot1", "P"),
            ("Bandwidth", "invalid"),
            ("Modulation", "invalid"),
            ("BusyLock", "maybe"),
            ("Reversed", "maybe"),
            ("PTTID", "invalid"),
            ("RX_Tone", "D089N"),
        ] {
            for active in ["True", "False"] {
                let mut csv = csv::Reader::from_reader(PROGRAMMER_HEADER.as_bytes());
                let headers = csv.headers().unwrap().clone();
                let mut csv = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .from_reader(PROGRAMMER_CHANNEL.as_bytes());
                let mut row: Vec<String> = csv
                    .records()
                    .next()
                    .unwrap()
                    .unwrap()
                    .iter()
                    .map(str::to_string)
                    .collect();
                row[1] = active.into();
                row[headers.iter().position(|header| header == field).unwrap()] = value.into();
                let mut writer = csv::Writer::from_writer(Vec::new());
                writer.write_record(&headers).unwrap();
                writer.write_record(row).unwrap();
                let bytes = writer.into_inner().unwrap();
                let error =
                    load_channel_document_from_reader(bytes.as_slice(), ChannelFileFormat::Csv)
                        .unwrap_err();
                assert!(
                    error.to_string().contains("row 2"),
                    "{field}={value:?}: {error:#}"
                );
            }
        }
        for payload in [
            "Channel_Num,Active,TX_Power\n1,False,invalid\n",
            "Channel_Num,Active,Name\n1,True,\n",
            "Channel_Num,Active,Name\n199,False,\n",
            "Channel_Num,Active,Name\n0,False,\n",
            "Channel_Num,Active,Name,RX\n0,GroupLabels,LOCAL,145\n",
            "Channel_Num,Active,Name\n0,GroupLabels,LOCAL\n0,GroupLabels,OTHER\n",
            "Channel_Num,Active,Name\n1,False,\n1,False,\n",
            "Channel_Num,Active,Name\n0,GroupLabels,\u{e9}\n",
            "Channel_Num,Active,Name,Position\n1,False,,2\n",
        ] {
            assert!(load_csv(payload).is_err(), "{payload}");
        }
        let payload = format!(
            "Channel_Num,Active,Name\n0,GroupLabels,{}\n",
            "X".repeat(76)
        );
        assert!(load_csv(&payload).is_err());
    }

    #[test]
    fn json_vec_api_remains_an_array_and_preserves_raw_position() {
        let mut channel = sample_channel();
        channel.position = 0;
        let mut bytes = Vec::new();
        save_channels_to_writer(
            &mut bytes,
            std::slice::from_ref(&channel),
            ChannelFileFormat::Json,
        )
        .unwrap();
        assert!(
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .unwrap()
                .is_array()
        );
        let decoded = load_channels_from_reader(bytes.as_slice(), ChannelFileFormat::Json).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded[0]).unwrap(),
            serde_json::to_value(&channel).unwrap()
        );
        assert_eq!(
            load_channel_document_from_reader(bytes.as_slice(), ChannelFileFormat::Json)
                .unwrap()
                .channels
                .len(),
            1
        );
    }

    #[test]
    fn csv_rejects_invalid_rows_instead_of_skipping_or_defaulting_them() {
        for payload in [
            "Channel_Num,Name\ninvalid,TEST\n",
            "Channel_Num,Name\n0,TEST\n",
            "Channel_Num,TX_Power\n1,invalid\n",
            "Channel_Num,Active\n1,maybe\n",
            "Channel_Num,Slot1\n1,P\n",
            "Channel_Num,PTTID\n1,invalid\n",
        ] {
            let error =
                load_channels_from_reader(payload.as_bytes(), ChannelFileFormat::Csv).unwrap_err();
            assert!(error.to_string().contains("row 2"), "{error:#}");
        }
    }

    #[test]
    fn json_output_propagates_buffered_flush_failures() {
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffered = std::io::BufWriter::new(FailingWriter);
        assert!(save_channels_to_writer(buffered, &[], ChannelFileFormat::Json).is_err());
    }
}
