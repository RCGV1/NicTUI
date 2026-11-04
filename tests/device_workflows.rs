#![cfg(unix)]

use std::io::{self, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use nictui::device::{
    flash_firmware, read_dtmf_presets, update_dtmf_preset, update_scan_preset, write_band_plans,
    write_channels, write_codeplug, write_dtmf_presets, write_group_labels, write_scan_presets,
    write_settings,
};
use nictui::protocol::{
    BandPlan, Channel, DTMFPreset, EEPROM_SIZE, Endianness, RadioProtocol, ScanPreset,
};
use serialport::{SerialPort, TTYPort};

struct Radio {
    slave: TTYPort,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<(Vec<u8>, Vec<u8>)>>,
}

impl Radio {
    fn new(memory: Vec<u8>) -> Self {
        Self::with_corrupt_read(memory, None)
    }

    fn with_corrupt_read(mut memory: Vec<u8>, corrupt_block: Option<u8>) -> Self {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_millis(50)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut writes = Vec::new();
            while !stopped.load(Ordering::Relaxed) {
                let mut command = [0];
                match master.read_exact(&mut command) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("emulator read failed: {error}"),
                }
                match command[0] {
                    0x01 | 0x45 | 0x46 => master.write_all(&command).unwrap(),
                    0x30 => {
                        let mut block = [0];
                        master.read_exact(&mut block).unwrap();
                        let offset = usize::from(block[0]) * 32;
                        let data = &memory[offset..offset + 32];
                        let mut response = vec![0x30];
                        response.extend_from_slice(data);
                        let mut checksum =
                            data.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
                        if corrupt_block == Some(block[0]) {
                            checksum = checksum.wrapping_add(1);
                        }
                        response.push(checksum);
                        master.write_all(&response).unwrap();
                    }
                    0x31 => {
                        let mut body = [0; 34];
                        master.read_exact(&mut body).unwrap();
                        assert_eq!(
                            body[33],
                            body[1..33]
                                .iter()
                                .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
                        );
                        let offset = usize::from(body[0]) * 32;
                        memory[offset..offset + 32].copy_from_slice(&body[1..33]);
                        writes.push(body[0]);
                        master.write_all(&[0x31]).unwrap();
                    }
                    other => panic!("unexpected command {other:02X}"),
                }
            }
            (memory, writes)
        });
        Self {
            slave,
            stop,
            worker: Some(worker),
        }
    }

    fn port(&self) -> String {
        self.slave.name().unwrap()
    }

    fn finish(mut self) -> (Vec<u8>, Vec<u8>) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for Radio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn memory() -> Vec<u8> {
    let mut data = vec![0xAB; EEPROM_SIZE];
    data[0x1900..0x1902].copy_from_slice(&0xD82Fu16.to_be_bytes());
    data[0x1A00..0x1A02].copy_from_slice(&0xA46Du16.to_be_bytes());
    data
}

#[test]
fn channel_endianness_read_failures_abort_updates_and_deletions_without_writes() {
    let channel = Channel {
        channel_num: 1,
        name: "TEST".into(),
        rx_freq: "145.35000".into(),
        tx_freq: "0".into(),
        rx_tone: "Off".into(),
        tx_tone: "Off".into(),
        bandwidth: "Wide".into(),
        modulation: "FM".into(),
        ..Default::default()
    };
    for clear in [false, true] {
        let before = memory();
        let radio = Radio::with_corrupt_read(before.clone(), Some(2));
        let channels = if clear {
            &[][..]
        } else {
            std::slice::from_ref(&channel)
        };
        let deleted = if clear { &[1][..] } else { &[][..] };
        let error = write_channels(
            &radio.port(),
            channels,
            deleted,
            Endianness::Big,
            false,
            |_| {},
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("no channel blocks were written"),
            "{error:#}"
        );
        assert!(format!("{error:#}").contains("Failed to read block 2"));
        let (after, writes) = radio.finish();
        assert_eq!(after, before);
        assert!(writes.is_empty());
    }
}

#[test]
fn ambiguous_channel_byte_order_aborts_before_any_write() {
    let mut before = memory();
    let channel = Channel {
        channel_num: 1,
        name: "TEST".into(),
        rx_freq: "144.00000".into(),
        tx_freq: "144.00000".into(),
        rx_tone: "Off".into(),
        tx_tone: "Off".into(),
        bandwidth: "Wide".into(),
        modulation: "FM".into(),
        ..Default::default()
    };
    before[64..96].copy_from_slice(&RadioProtocol::pack_channel(&channel, Endianness::Big));
    let radio = Radio::new(before.clone());
    let error = write_channels(
        &radio.port(),
        &[channel],
        &[],
        Endianness::Big,
        false,
        |_| {},
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("ambiguous"), "{error:#}");
    let (after, writes) = radio.finish();
    assert_eq!(after, before);
    assert!(writes.is_empty());
}

#[test]
fn dtmf_reads_and_writes_use_true_offset_and_preserve_group_labels() {
    let mut before = memory();
    let preset = DTMFPreset {
        index: 0,
        digits: vec![1, 2, 3],
        label: "FIRST".into(),
    };
    before[0x1CF0..0x1CFC].copy_from_slice(&RadioProtocol::pack_dtmf_preset(&preset));
    let radio = Radio::new(before.clone());
    let presets = read_dtmf_presets(&radio.port(), |_| {}).unwrap();
    assert_eq!(presets[0].digits, preset.digits);
    assert_eq!(presets[0].label, preset.label);
    let replacement = DTMFPreset {
        index: 19,
        digits: vec![4, 5],
        label: "LAST".into(),
    };
    update_dtmf_preset(&radio.port(), &replacement, |_| {}).unwrap();
    let (after, writes) = radio.finish();
    assert_eq!(&after[..0x1DD4], &before[..0x1DD4]);
    assert_eq!(
        &after[0x1DD4..0x1DE0],
        RadioProtocol::pack_dtmf_preset(&replacement)
    );
    assert_eq!(&after[0x1DE0..], &before[0x1DE0..]);
    assert!(writes.iter().all(|block| (231..=238).contains(block)));
    let radio = Radio::new(before.clone());
    write_dtmf_presets(&radio.port(), &[preset], |_| {}).unwrap();
    let (after, _) = radio.finish();
    assert_eq!(&after[0x1C90..0x1CF0], &before[0x1C90..0x1CF0]);
    assert_eq!(&after[0x1DE0..], &before[0x1DE0..]);
}

#[test]
fn settings_write_preserves_unmodeled_firmware_fields() {
    let before = memory();
    let radio = Radio::new(before.clone());
    let mut settings =
        RadioProtocol::parse_settings_block(&before[0x1900..0x1980], Endianness::Big);
    settings.squelch = 3;
    write_settings(&radio.port(), &settings, Endianness::Big, false, |_| {}).unwrap();
    let (after, _) = radio.finish();
    assert_eq!(after[0x1902], 3);
    assert_eq!(&after[0x1967..], &before[0x1967..]);
}

#[test]
fn group_name_write_preserves_reserved_slot_and_neighboring_records() {
    let before = memory();
    let radio = Radio::new(before.clone());
    let mut labels = vec![String::new(); 15];
    labels[14] = "LAST!".into();
    write_group_labels(&radio.port(), &labels, |_| {}).unwrap();
    let (after, _) = radio.finish();
    assert_eq!(&after[..0x1C90], &before[..0x1C90]);
    assert_eq!(&after[0x1CE4..0x1CEA], b"LAST!\0");
    assert_eq!(&after[0x1CEA..], &before[0x1CEA..]);
}

#[test]
fn scan_write_preserves_group_names_in_the_shared_last_block() {
    let mut before = memory();
    before[0x1B00..0x1C90].fill(0);
    let preset = ScanPreset {
        index: 19,
        start_freq: 14_531_000,
        range: 1900,
        step: 1250,
        resume: 10,
        persist: 20,
        modulation: 2,
        ultrascan: 20,
        label: "LAST".into(),
        raw_tail: 0xAB,
        ..ScanPreset::default()
    };
    before[0x1B00..0x1B14]
        .copy_from_slice(&RadioProtocol::pack_scan_preset(&preset, Endianness::Big));
    let radio = Radio::new(before.clone());
    write_scan_presets(
        &radio.port(),
        std::slice::from_ref(&preset),
        Endianness::Big,
        |_| {},
    )
    .unwrap();
    let (after, _) = radio.finish();
    assert_eq!(&after[..0x1B00], &before[..0x1B00]);
    assert_eq!(
        &after[0x1C7C..0x1C90],
        RadioProtocol::pack_scan_preset(&preset, Endianness::Big)
    );
    assert_eq!(&after[0x1C90..], &before[0x1C90..]);
}

#[test]
fn scan_writes_reject_uncertain_radio_byte_order_before_any_write() {
    let preset = ScanPreset {
        index: 0,
        start_freq: 14_531_000,
        range: 1900,
        step: 1250,
        label: "TEST!".into(),
        ..ScanPreset::default()
    };
    let mut empty = memory();
    empty[0x1B00..0x1C90].fill(0);
    let mut erased = empty.clone();
    erased[0x1B00..0x1C90].fill(0xFF);
    let mut ambiguous = empty.clone();
    ambiguous[0x1B00..0x1B08].copy_from_slice(&[0, 1, 2, 0, 1, 2, 1, 2]);
    let mut conflicting = empty.clone();
    conflicting[0x1B00..0x1B14]
        .copy_from_slice(&RadioProtocol::pack_scan_preset(&preset, Endianness::Big));
    conflicting[0x1B14..0x1B28].copy_from_slice(&RadioProtocol::pack_scan_preset(
        &preset,
        Endianness::Little,
    ));

    for before in [empty, erased, ambiguous, conflicting, memory()] {
        for update in [false, true] {
            let radio = Radio::new(before.clone());
            let result = if update {
                update_scan_preset(&radio.port(), &preset, |_| {})
            } else {
                write_scan_presets(
                    &radio.port(),
                    std::slice::from_ref(&preset),
                    Endianness::Big,
                    |_| {},
                )
            };
            let error = result.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("no scan preset blocks were written"),
                "{error:#}"
            );
            let (after, writes) = radio.finish();
            assert_eq!(after, before);
            assert!(writes.is_empty());
        }
    }
}

#[test]
fn scan_updates_preserve_detected_byte_order_with_punctuated_labels() {
    for endian in [Endianness::Big, Endianness::Little] {
        let mut before = memory();
        before[0x1B00..0x1C90].fill(0);
        let preset = ScanPreset {
            index: 19,
            start_freq: 14_531_000,
            range: 1900,
            step: 1250,
            resume: 10,
            persist: 20,
            label: "AIR-1!".into(),
            ..ScanPreset::default()
        };
        before[0x1B00..0x1B14].copy_from_slice(&RadioProtocol::pack_scan_preset(&preset, endian));
        for update in [false, true] {
            let radio = Radio::new(before.clone());
            if update {
                update_scan_preset(&radio.port(), &preset, |_| {}).unwrap();
            } else {
                write_scan_presets(
                    &radio.port(),
                    std::slice::from_ref(&preset),
                    // An unrelated settings/default order must not choose scan encoding.
                    if endian == Endianness::Big {
                        Endianness::Little
                    } else {
                        Endianness::Big
                    },
                    |_| {},
                )
                .unwrap();
            }
            let (after, writes) = radio.finish();
            assert_eq!(
                &after[0x1C7C..0x1C90],
                RadioProtocol::pack_scan_preset(&preset, endian)
            );
            assert_eq!(&after[..0x1B00], &before[..0x1B00]);
            assert_eq!(&after[0x1C90..], &before[0x1C90..]);
            assert_eq!(writes, (216..229).collect::<Vec<_>>());
            if update {
                assert_eq!(&after[0x1B00..0x1C7C], &before[0x1B00..0x1C7C]);
            }
        }
    }
}

#[test]
fn band_write_preserves_version_marker_and_adjacent_sections() {
    let before = memory();
    let radio = Radio::new(before.clone());
    let plan = BandPlan {
        index: 19,
        start_freq: 40_000_000,
        end_freq: 50_000_000,
        max_power: 255,
        tx_allowed: true,
        wrap: true,
        modulation: 7,
        bandwidth: 5,
        raw_flags: 0,
    };
    write_band_plans(
        &radio.port(),
        std::slice::from_ref(&plan),
        Endianness::Big,
        |_| {},
    )
    .unwrap();
    let (after, _) = radio.finish();
    assert_eq!(&after[..0x1AC0], &before[..0x1AC0]);
    assert_eq!(
        &after[0x1AC0..0x1ACA],
        RadioProtocol::pack_bandplan(&plan, Endianness::Big)
    );
    assert_eq!(&after[0x1ACA..], &before[0x1ACA..]);
}

#[test]
fn raw_restore_rejects_wrong_version_or_endianness_before_any_write() {
    assert!(write_codeplug("/not/a/port", &vec![0xFF; EEPROM_SIZE], false, |_| {}).is_err());
    let radio = Radio::new(memory());
    let mut wrong_endian = memory();
    wrong_endian[0x1900..0x1902].reverse();
    let error = write_codeplug(&radio.port(), &wrong_endian, false, |_| {}).unwrap_err();
    assert!(error.to_string().contains("does not match"));
    assert!(radio.finish().1.is_empty());
}

#[test]
fn invalid_group_labels_and_firmware_fail_before_opening_a_port() {
    for labels in [
        vec!["abcdefg".into()],
        vec!["\u{00e9}".into()],
        vec!["".into(); 17],
    ] {
        let error = write_group_labels("/not/a/port", &labels, |_| {}).unwrap_err();
        assert!(!error.to_string().contains("open"));
    }
    assert!(
        flash_firmware("/not/a/port", &[], |_| {})
            .unwrap_err()
            .to_string()
            .contains("empty")
    );
}

#[test]
fn firmware_rejects_both_ble_uri_forms_before_transport_access() {
    for port in ["ble://fixture-only", "ble:fixture-only"] {
        let mut events = Vec::new();
        let error = flash_firmware(port, &[0x42; 32], |event| events.push(event)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Firmware flashing requires a USB serial connection"
        );
        assert!(
            events.is_empty(),
            "BLE rejection must precede opening status"
        );
    }
}

#[test]
fn firmware_partial_and_aligned_images_use_final_data_packet_and_require_ack() {
    for (length, init_ack, final_ack) in [
        (1usize, false, 0xA3),
        (32, false, 0xA3),
        (33, false, 0xA3),
        (33, false, 0x00),
        (32, true, 0xA3),
        (33, true, 0xA3),
        (33, true, 0x00),
    ] {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            master.write_all(&[0xA5]).unwrap();
            let mut init = [0; 36];
            master.read_exact(&mut init).unwrap();
            assert_eq!(init[0], 0xA0);
            if init_ack {
                master.write_all(&[0xA3]).unwrap();
            } else {
                // Exercise trailing bootloader chatter before the quiet fallback.
                master.write_all(&[0xA5, 0xA5]).unwrap();
            }
            let mut packets = Vec::new();
            for index in 0..length.div_ceil(32) {
                let mut packet = [0; 36];
                master.read_exact(&mut packet).unwrap();
                let last = index + 1 == length.div_ceil(32);
                assert_eq!(packet[0], if last { 0xA2 } else { 0xA1 });
                assert_eq!(&packet[1..3], &(index as u16).to_be_bytes());
                assert_eq!(
                    packet[3],
                    packet[4..]
                        .iter()
                        .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
                );
                master
                    .write_all(&[if last { final_ack } else { 0xA3 }])
                    .unwrap();
                packets.push(packet);
            }
            thread::sleep(Duration::from_millis(150));
            packets
        });
        let result = flash_firmware(&port, &vec![0x42; length], |_| {});
        assert_eq!(result.is_ok(), final_ack == 0xA3, "{result:?}");
        let packets = worker.join().unwrap();
        let last = packets.last().unwrap();
        let tail = (length - 1) % 32 + 1;
        assert!(last[4..4 + tail].iter().all(|byte| *byte == 0x42));
        assert!(last[4 + tail..].iter().all(|byte| *byte == 0));
    }
}

#[test]
fn firmware_retries_identical_a1_and_a2_packets_after_lost_ack() {
    for length in [32usize, 33] {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            master.write_all(&[0xA5]).unwrap();
            let mut packet = [0; 36];
            master.read_exact(&mut packet).unwrap();
            assert_eq!(packet[0], 0xA0);
            master.write_all(&[0xA3]).unwrap();
            for block in 0..length.div_ceil(32) {
                master.read_exact(&mut packet).unwrap();
                assert_eq!(
                    packet[0],
                    if block + 1 == length.div_ceil(32) {
                        0xA2
                    } else {
                        0xA1
                    }
                );
                assert_eq!(&packet[1..3], &(block as u16).to_be_bytes());
                let mut retry = [0; 36];
                master.read_exact(&mut retry).unwrap();
                assert_eq!(retry, packet, "Lost ACK must resend exactly the same block");
                master.write_all(&[0xA3]).unwrap();
            }
            thread::sleep(Duration::from_millis(100));
        });
        let mut progress = Vec::new();
        flash_firmware(&port, &vec![0x42; length], |event| {
            if let nictui::device::ProgressEvent::Progress(value) = event {
                progress.push(value);
            }
        })
        .unwrap();
        let blocks = length.div_ceil(32);
        assert_eq!(
            progress,
            (1..=blocks)
                .map(|block| block as f64 / blocks as f64)
                .collect::<Vec<_>>()
        );
        worker.join().unwrap();
    }
}

#[test]
fn firmware_never_advances_after_missing_or_rejected_data_ack() {
    for (length, ack) in [
        (32usize, None),
        (64, None),
        (32, Some(0x00)),
        (64, Some(0x00)),
    ] {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            master.write_all(&[0xA5]).unwrap();
            let mut packet = [0; 36];
            master.read_exact(&mut packet).unwrap();
            assert_eq!(packet[0], 0xA0);
            master.write_all(&[0xA3]).unwrap();
            master.read_exact(&mut packet).unwrap();
            assert_eq!(packet[0], if length == 32 { 0xA2 } else { 0xA1 });
            assert_eq!(&packet[1..3], &[0, 0]);
            if let Some(ack) = ack {
                master.write_all(&[ack]).unwrap();
            } else {
                for _ in 0..2 {
                    let mut retry = [0; 36];
                    master.read_exact(&mut retry).unwrap();
                    assert_eq!(retry, packet);
                }
            }
            master.set_timeout(Duration::from_millis(700)).unwrap();
            let error = master.read_exact(&mut packet[..1]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        });
        let mut progress = Vec::new();
        let error = flash_firmware(&port, &vec![0x42; length], |event| {
            if let nictui::device::ProgressEvent::Progress(value) = event {
                progress.push(value);
            }
        })
        .unwrap_err();
        assert!(
            error.to_string().contains(if ack.is_some() {
                "rejected block 0"
            } else {
                "Missing bootloader ACK for block 0"
            }),
            "{error:#}"
        );
        assert!(progress.is_empty());
        worker.join().unwrap();
    }
}
