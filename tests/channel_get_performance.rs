#![cfg(unix)]

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nictui::channel_file::{ChannelFileFormat, save_channels_to_writer};
use nictui::device::{ProgressEvent, read_channel};
use nictui::protocol::{BLOCK_SIZE, Channel, EEPROM_SIZE, Endianness, RadioProtocol};
use serialport::{SerialPort, TTYPort};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Ping,
    Read(u8),
    Pause,
    Resume,
}

#[derive(Default)]
struct Fault {
    checksum_block: Option<u8>,
    bad_pause: bool,
    bad_resume_once: bool,
    interrupt: Option<(u8, Arc<AtomicU32>)>,
}

struct Radio {
    slave: TTYPort,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Vec<Request>>>,
}

impl Radio {
    fn new(memory: Vec<u8>, fault: Fault) -> Self {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_millis(50)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            let mut paused = false;
            let mut resumed = false;
            while !stopped.load(Ordering::Relaxed) {
                let mut command = [0];
                match master.read_exact(&mut command) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("PTY receive failed: {error}"),
                }
                let mut response = command.to_vec();
                let request = match command[0] {
                    0x01 => Request::Ping,
                    0x45 => {
                        paused = true;
                        if fault.bad_pause {
                            response[0] = 0;
                        }
                        Request::Pause
                    }
                    0x46 => {
                        paused = false;
                        if fault.bad_resume_once && !resumed {
                            response[0] = 0;
                        }
                        resumed = true;
                        Request::Resume
                    }
                    0x30 => {
                        let mut block = [0];
                        master.read_exact(&mut block).unwrap();
                        let offset = usize::from(block[0]) * BLOCK_SIZE;
                        let data = &memory[offset..offset + BLOCK_SIZE];
                        response.extend_from_slice(data);
                        let mut checksum = data.iter().fold(0u8, |sum, b| sum.wrapping_add(*b));
                        if paused && fault.checksum_block == Some(block[0]) {
                            checksum ^= 1;
                        }
                        response.push(checksum);
                        if let Some((trigger, pid)) = &fault.interrupt
                            && paused
                            && *trigger == block[0]
                        {
                            let pid = i32::try_from(pid.load(Ordering::SeqCst)).unwrap();
                            assert!(pid > 0, "signal only the spawned fake-radio CLI child");
                            // SAFETY: the positive PID belongs to our unreaped child.
                            assert_eq!(unsafe { kill(pid, 15) }, 0);
                            thread::sleep(Duration::from_millis(75));
                        }
                        Request::Read(block[0])
                    }
                    other => panic!("forbidden write/reboot/remote command: {other:02X}"),
                };
                requests.push(request);
                for chunk in response.chunks(7) {
                    master.write_all(chunk).unwrap();
                }
            }
            requests
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

    fn finish(mut self) -> Vec<Request> {
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

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

struct Process(Option<Child>);

impl Process {
    fn spawn(port: &str, channel: u16, args: &[&str]) -> Self {
        Self(Some(
            Command::new(env!("CARGO_BIN_EXE_nictui"))
                .args([
                    "channels",
                    "get",
                    "--channel",
                    &channel.to_string(),
                    "--port",
                    port,
                ])
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }

    fn output(mut self) -> Output {
        let start = Instant::now();
        while self.0.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(15), "CLI stalled");
            thread::sleep(Duration::from_millis(10));
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                }
            }
        }
    }
}

fn memory(settings: Endianness) -> Vec<u8> {
    let mut data = vec![0; EEPROM_SIZE];
    let marker = match settings {
        Endianness::Big => 0xD82Fu16.to_be_bytes(),
        Endianness::Little => 0xD82Fu16.to_le_bytes(),
    };
    data[0x1900..0x1902].copy_from_slice(&marker);
    // The live-radio regression has little-endian firmware and big-endian channels.
    data[240 * BLOCK_SIZE] = 0x57;
    data
}

fn channel(slot: u16) -> Channel {
    Channel {
        channel_num: slot,
        name: format!("SLOT{slot}"),
        rx_freq: "145.35000".into(),
        tx_freq: "145.35000".into(),
        rx_tone: "D023n".into(),
        tx_tone: "88.5".into(),
        bandwidth: "Wide".into(),
        modulation: "FM".into(),
        groups: [1, 3, 5, 7],
        ..Channel::default()
    }
}

fn put(data: &mut [u8], block: usize, rx: u32, tx: u32, endian: Endianness) {
    let mut raw = RadioProtocol::pack_channel(&channel((block.max(2) - 1) as u16), endian);
    let bytes = |value: u32| match endian {
        Endianness::Big => value.to_be_bytes(),
        Endianness::Little => value.to_le_bytes(),
    };
    raw[..4].copy_from_slice(&bytes(rx));
    raw[4..8].copy_from_slice(&bytes(tx));
    data[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE].copy_from_slice(&raw);
}

fn expected(data: &[u8], slot: u16, endian: Endianness) -> Channel {
    let offset = (usize::from(slot) + 1) * BLOCK_SIZE;
    RadioProtocol::parse_channel(&data[offset..offset + BLOCK_SIZE], slot, endian).unwrap()
}

fn opening() -> Vec<Request> {
    let mut requests = vec![Request::Ping];
    requests.extend((200..204).map(Request::Read));
    requests.push(Request::Pause);
    requests
}

fn assert_session(trace: &[Request], reads: impl IntoIterator<Item = u8>, resumes: usize) {
    let mut expected = opening();
    expected.extend(reads.into_iter().map(Request::Read));
    expected.extend(std::iter::repeat_n(Request::Resume, resumes));
    assert_eq!(trace, expected);
}

fn fast_reads(slot: u16) -> Vec<u8> {
    let target = (slot + 1) as u8;
    let mut blocks = vec![target];
    blocks.extend((2..10).filter(|b| *b != target));
    blocks
}

fn fallback_reads(slot: u16) -> Vec<u8> {
    let target = (slot + 1) as u8;
    let mut blocks = fast_reads(slot);
    blocks.extend([0, 1].into_iter().chain(10..200).filter(|b| *b != target));
    blocks
}

#[test]
fn first_and_last_populated_slots_use_bounded_unique_reads_in_both_byte_orders() {
    for endian in [Endianness::Big, Endianness::Little] {
        for settings in [Endianness::Big, Endianness::Little] {
            for slot in [1, 8, 9, 197, 198] {
                let mut data = memory(settings);
                put(&mut data, 2, 14_535_000, 14_535_000, endian);
                put(&mut data, usize::from(slot) + 1, 14_535_000, 0, endian);
                let want = expected(&data, slot, endian);
                let radio = Radio::new(data, Fault::default());
                let actual = read_channel(&radio.port(), slot, |_| {}).unwrap().unwrap();
                assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    serde_json::to_value(want).unwrap()
                );
                let blocks = fast_reads(slot);
                assert!(blocks.len() <= if slot <= 8 { 8 } else { 9 });
                assert_session(&radio.finish(), blocks, 1);
            }
        }
    }
}

#[test]
fn empty_targets_short_circuit_on_empty_and_populated_radios() {
    for slot in [1, 198] {
        for erased in [0, 0xFF] {
            for populated in [false, true] {
                let mut data = memory(Endianness::Little);
                if populated {
                    put(&mut data, 3, 14_535_000, 14_535_000, Endianness::Big);
                }
                let offset = (usize::from(slot) + 1) * BLOCK_SIZE;
                data[offset..offset + BLOCK_SIZE].fill(erased);
                let radio = Radio::new(data, Fault::default());
                assert!(read_channel(&radio.port(), slot, |_| {}).unwrap().is_none());
                assert_session(&radio.finish(), [(slot + 1) as u8], 1);
            }
        }
    }
}

#[test]
fn rx_and_tx_evidence_not_rx_alone_determine_byte_order() {
    for endian in [Endianness::Big, Endianness::Little] {
        let mut data = memory(Endianness::Big);
        put(&mut data, 2, 0x013A3701, 14_535_000, endian);
        let want = expected(&data, 1, endian);
        let radio = Radio::new(data, Fault::default());
        let actual = read_channel(&radio.port(), 1, |_| {}).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(want).unwrap()
        );
        assert_session(&radio.finish(), fast_reads(1), 1);
    }
}

#[test]
fn ambiguous_initial_window_uses_vfo_or_later_evidence_without_duplicate_reads() {
    for evidence in [0, 1, 10, 199] {
        for endian in [Endianness::Big, Endianness::Little] {
            let mut data = memory(Endianness::Little);
            put(&mut data, 2, 0x013A3701, 0x013A3701, endian);
            put(&mut data, evidence, 14_535_000, 14_535_000, endian);
            let want = expected(&data, 1, endian);
            let radio = Radio::new(data, Fault::default());
            let actual = read_channel(&radio.port(), 1, |_| {}).unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(want).unwrap()
            );
            assert_session(&radio.finish(), fallback_reads(1), 1);
        }
    }
}

#[test]
fn nonempty_ambiguity_never_falls_back_to_settings_or_firmware_marker() {
    for settings in [Endianness::Big, Endianness::Little] {
        let mut data = memory(settings);
        put(&mut data, 199, 0x013A3701, 0x013A3701, Endianness::Big);
        let radio = Radio::new(data, Fault::default());
        let error = read_channel(&radio.port(), 198, |_| {}).unwrap_err();
        assert!(format!("{error:#}").contains("ambiguous"));
        assert_session(&radio.finish(), fallback_reads(198), 1);
    }
}

#[test]
fn conflicting_window_target_and_fallback_evidence_fail_closed() {
    for (slot, conflicting_block) in [(1, 3), (198, 2), (1, 199)] {
        let mut data = memory(Endianness::Little);
        if conflicting_block == 199 {
            put(&mut data, 2, 0x013A3701, 0x013A3701, Endianness::Big);
            put(&mut data, 0, 14_535_000, 14_535_000, Endianness::Big);
        } else {
            put(
                &mut data,
                usize::from(slot) + 1,
                14_535_000,
                14_535_000,
                Endianness::Big,
            );
        }
        put(
            &mut data,
            conflicting_block,
            14_535_000,
            14_535_000,
            Endianness::Little,
        );
        let radio = Radio::new(data, Fault::default());
        let error = read_channel(&radio.port(), slot, |_| {}).unwrap_err();
        assert!(format!("{error:#}").contains("conflicting"));
        let blocks = if conflicting_block == 199 {
            fallback_reads(slot)
        } else {
            fast_reads(slot)
        };
        assert_session(&radio.finish(), blocks, 1);
    }
}

#[test]
fn target_sample_and_fallback_read_failures_always_resume() {
    for bad in [2, 5, 199] {
        let mut data = memory(Endianness::Big);
        let rx = if bad == 199 { 0x013A3701 } else { 14_535_000 };
        put(&mut data, 2, rx, rx, Endianness::Big);
        let radio = Radio::new(
            data,
            Fault {
                checksum_block: Some(bad),
                ..Fault::default()
            },
        );
        let error = read_channel(&radio.port(), 1, |_| {}).unwrap_err();
        assert!(format!("{error:#}").contains(&format!("Failed to read block {bad}")));
        let blocks = if bad == 199 {
            fallback_reads(1)
        } else {
            fast_reads(1)
        };
        let mut reads: Vec<_> = blocks.into_iter().take_while(|b| *b != bad).collect();
        reads.extend([bad; 6]);
        assert_session(&radio.finish(), reads, 1);
    }
}

#[test]
fn pause_failure_and_bad_resume_ack_fail_and_cleanup_retries_resume() {
    for pause_failure in [false, true] {
        let mut data = memory(Endianness::Big);
        put(&mut data, 2, 14_535_000, 14_535_000, Endianness::Big);
        let radio = Radio::new(
            data,
            Fault {
                bad_pause: pause_failure,
                bad_resume_once: !pause_failure,
                ..Fault::default()
            },
        );
        let error = read_channel(&radio.port(), 1, |_| {}).unwrap_err();
        let text = format!("{error:#}");
        if pause_failure {
            assert!(text.contains("pause"), "{text}");
            assert_session(&radio.finish(), [], 1);
        } else {
            assert!(text.contains("resume"), "{text}");
            assert_session(&radio.finish(), fast_reads(1), 2);
        }
    }
}

#[test]
fn callback_panics_resume_on_empty_success_and_ambiguous_fallback() {
    for mode in [0, 1, 2] {
        let mut data = memory(Endianness::Big);
        if mode != 0 {
            let rx = if mode == 1 { 14_535_000 } else { 0x013A3701 };
            put(&mut data, 2, rx, rx, Endianness::Big);
        }
        let radio = Radio::new(data, Fault::default());
        let result = catch_unwind(AssertUnwindSafe(|| {
            read_channel(&radio.port(), 1, |event| {
                if matches!(event, ProgressEvent::Progress(_))
                    || matches!(event, ProgressEvent::Status(ref text) if text.starts_with("Checking"))
                {
                    panic!("simulated callback cancellation");
                }
            })
        }));
        assert!(result.is_err());
        let reads = if mode == 0 { vec![2] } else { fast_reads(1) };
        assert_session(&radio.finish(), reads, 1);
    }
}

#[test]
fn invalid_slots_are_rejected_before_open_by_api_and_cli() {
    for slot in [0, 199, u16::MAX] {
        let radio = Radio::new(memory(Endianness::Big), Fault::default());
        let error = read_channel(&radio.port(), slot, |_| {}).unwrap_err();
        assert!(format!("{error:#}").contains("between 1 and 198"));
        let output = Process::spawn(&radio.port(), slot, &[]).output();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("1-198"));
        assert!(radio.finish().is_empty());
    }
}

#[test]
fn cli_get_keeps_json_object_csv_and_empty_slot_error_compatibility() {
    for format in ["json", "csv"] {
        let mut data = memory(Endianness::Big);
        put(&mut data, 2, 14_535_000, 14_535_000, Endianness::Big);
        let want = expected(&data, 1, Endianness::Big);
        let radio = Radio::new(data, Fault::default());
        let output = Process::spawn(&radio.port(), 1, &["--format", format]).output();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if format == "json" {
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(value.is_object());
            assert_eq!(value, serde_json::to_value(want).unwrap());
        } else {
            let mut csv = Vec::new();
            save_channels_to_writer(&mut csv, &[want], ChannelFileFormat::Csv).unwrap();
            assert_eq!(output.stdout, csv);
        }
        assert_session(&radio.finish(), fast_reads(1), 1);
    }
    let mut data = memory(Endianness::Big);
    put(&mut data, 2, 14_535_000, 14_535_000, Endianness::Big);
    let radio = Radio::new(data, Fault::default());
    let output = Process::spawn(&radio.port(), 198, &[]).output();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Channel 198 is empty or not programmed")
    );
    assert_session(&radio.finish(), [199], 1);
}

#[test]
fn cli_signal_during_target_window_and_fallback_reads_resumes_before_exit() {
    for block in [2, 5, 10] {
        let mut data = memory(Endianness::Big);
        let rx = if block == 10 { 0x013A3701 } else { 14_535_000 };
        put(&mut data, 2, rx, rx, Endianness::Big);
        let pid = Arc::new(AtomicU32::new(0));
        let radio = Radio::new(
            data,
            Fault {
                interrupt: Some((block, Arc::clone(&pid))),
                ..Fault::default()
            },
        );
        let process = Process::spawn(&radio.port(), 1, &[]);
        pid.store(process.0.as_ref().unwrap().id(), Ordering::SeqCst);
        let output = process.output();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Interrupted"));
        let blocks = if block == 10 {
            fallback_reads(1)
        } else {
            fast_reads(1)
        };
        let reads: Vec<_> = blocks
            .into_iter()
            .take_while(|b| *b != block)
            .chain([block])
            .collect();
        assert_session(&radio.finish(), reads, 1);
    }
}
