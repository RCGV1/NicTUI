#![cfg(unix)]

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use nictui::device::{self, ProgressEvent};
use nictui::protocol::codeplug::*;
use nictui::protocol::radio::ReadTiming;
use nictui::protocol::{
    BLOCK_SIZE, EEPROM_SIZE, Endianness, GROUP_LABELS_BLOCK_COUNT, GROUP_LABELS_BLOCK_START,
    TOTAL_BLOCKS,
};
use serialport::{SerialPort, TTYPort};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Ping,
    Read(u8),
    Pause,
    Resume,
}

type RequestTrace = Vec<(Request, Instant)>;
type WorkerResult = io::Result<RequestTrace>;

#[derive(Clone, Copy, Default)]
enum Fault {
    #[default]
    None,
    ChecksumOnce(u8),
    HeaderOnce(u8),
    TruncatedOnce(u8),
    ChecksumAlways(u8),
    PauseAck,
    PauseTimeout,
    ResumeAck,
    ResumeTimeout,
    ChecksumAndResume(u8),
}

#[derive(Default)]
struct Options {
    fault: Fault,
    response_delay: Duration,
}

struct Radio {
    // Keep the slave alive without acquiring a lock before the public API opens it.
    slave: TTYPort,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<WorkerResult>>,
}

impl Radio {
    fn new(memory: Vec<u8>, options: Options) -> Self {
        static PAIR_LOCK: Mutex<()> = Mutex::new(());
        let (mut master, slave) = {
            // On macOS, pair() uses non-thread-safe ptsname(); exclude other calls
            // until its shared buffer has been copied. Radio lifetimes stay parallel.
            let _guard = PAIR_LOCK.lock().unwrap();
            TTYPort::pair().unwrap()
        };
        master.set_timeout(Duration::from_millis(50)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            let mut paused = false;
            let mut injected = false;
            while !stopped.load(Ordering::Relaxed) {
                let mut command = [0];
                match master.read_exact(&mut command) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => return Err(error),
                }
                let mut response = command.to_vec();
                let request = match command[0] {
                    0x01 => Request::Ping,
                    0x45 => {
                        paused = true;
                        match options.fault {
                            Fault::PauseAck => response[0] = 0,
                            Fault::PauseTimeout => response.clear(),
                            _ => {}
                        }
                        Request::Pause
                    }
                    0x46 => {
                        paused = false;
                        match options.fault {
                            Fault::ResumeAck | Fault::ChecksumAndResume(_) => response[0] = 0,
                            Fault::ResumeTimeout => response.clear(),
                            _ => {}
                        }
                        Request::Resume
                    }
                    0x30 => {
                        let mut block = [0];
                        master.read_exact(&mut block)?;
                        let start = usize::from(block[0]) * BLOCK_SIZE;
                        let data = &memory[start..start + BLOCK_SIZE];
                        response.extend_from_slice(data);
                        response.push(data.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
                        if paused {
                            match options.fault {
                                Fault::ChecksumAlways(b) | Fault::ChecksumAndResume(b)
                                    if b == block[0] =>
                                {
                                    response[BLOCK_SIZE + 1] ^= 1
                                }
                                Fault::ChecksumOnce(b) if b == block[0] && !injected => {
                                    response[BLOCK_SIZE + 1] ^= 1;
                                    injected = true;
                                }
                                Fault::HeaderOnce(b) if b == block[0] && !injected => {
                                    response[0] = 0;
                                    injected = true;
                                }
                                Fault::TruncatedOnce(b) if b == block[0] && !injected => {
                                    response.truncate(7);
                                    injected = true;
                                }
                                _ => {}
                            }
                        }
                        Request::Read(block[0])
                    }
                    other => {
                        return Err(io::Error::other(format!("unexpected command {other:02X}")));
                    }
                };
                requests.push((request, Instant::now()));
                thread::sleep(options.response_delay);
                // Split the frame to exercise partial serial receives as well.
                for chunk in response.chunks(7) {
                    master.write_all(chunk)?;
                }
            }
            Ok(requests)
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

    fn finish(mut self) -> RequestTrace {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap().unwrap()
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

#[derive(Clone, Copy, Debug)]
enum Operation {
    Codeplug,
    Channels,
    Settings,
    Scans,
    Bands,
    Dtmf,
    Labels,
}

const OPERATIONS: [Operation; 7] = [
    Operation::Codeplug,
    Operation::Channels,
    Operation::Settings,
    Operation::Scans,
    Operation::Bands,
    Operation::Dtmf,
    Operation::Labels,
];

impl Operation {
    fn blocks(self) -> std::ops::Range<usize> {
        match self {
            Self::Codeplug => 0..TOTAL_BLOCKS,
            Self::Channels => 2..200,
            Self::Settings => 200..204,
            Self::Scans => 216..229,
            Self::Bands => 208..215,
            Self::Dtmf => 231..239,
            Self::Labels => {
                GROUP_LABELS_BLOCK_START..GROUP_LABELS_BLOCK_START + GROUP_LABELS_BLOCK_COUNT
            }
        }
    }

    fn read(self, port: &str, report: impl FnMut(ProgressEvent)) -> Result<serde_json::Value> {
        Ok(match self {
            Self::Codeplug => serde_json::to_value(device::read_codeplug(port, report)?)?,
            Self::Channels => serde_json::to_value(device::read_channels(port, report)?)?,
            Self::Settings => serde_json::to_value(device::read_settings(port, report)?)?,
            Self::Scans => serde_json::to_value(device::read_scan_presets(port, report)?)?,
            Self::Bands => serde_json::to_value(device::read_band_plans(port, report)?)?,
            Self::Dtmf => serde_json::to_value(device::read_dtmf_presets(port, report)?)?,
            Self::Labels => serde_json::to_value(device::read_group_labels(port, report)?)?,
        })
    }

    fn expected(self, memory: &[u8]) -> serde_json::Value {
        let endian = Endianness::Big;
        match self {
            Self::Codeplug => serde_json::to_value((memory, endian)).unwrap(),
            Self::Channels => {
                serde_json::to_value((extract_channels_from_codeplug(memory, endian), endian))
                    .unwrap()
            }
            Self::Settings => serde_json::to_value((
                extract_settings_from_codeplug(memory, endian).unwrap(),
                endian,
            ))
            .unwrap(),
            Self::Scans => {
                serde_json::to_value((extract_scan_presets_from_codeplug(memory, endian), endian))
                    .unwrap()
            }
            Self::Bands => {
                serde_json::to_value((extract_band_plans_from_codeplug(memory, endian), endian))
                    .unwrap()
            }
            Self::Dtmf => serde_json::to_value(extract_dtmf_presets_from_codeplug(memory)).unwrap(),
            Self::Labels => {
                serde_json::to_value(extract_group_labels_from_codeplug(memory)).unwrap()
            }
        }
    }
}

fn memory() -> Vec<u8> {
    let mut memory = vec![0; EEPROM_SIZE];
    memory[0x1900..0x1902].copy_from_slice(&0xD82Fu16.to_be_bytes());
    memory[0x1A00..0x1A02].copy_from_slice(&0xA46Du16.to_be_bytes());
    memory[0x1C90..0x1C96].copy_from_slice(b"GROUP\0");
    memory[64..68].copy_from_slice(&14_535_000u32.to_be_bytes());
    memory[68..72].copy_from_slice(&14_535_000u32.to_be_bytes());
    memory
}

fn opening() -> Vec<Request> {
    let mut requests = vec![Request::Ping];
    requests.extend((200..204).map(Request::Read));
    requests.push(Request::Pause);
    requests
}

fn progress(events: &[ProgressEvent]) -> Vec<f64> {
    events
        .iter()
        .filter_map(|event| match event {
            ProgressEvent::Progress(value) => Some(*value),
            _ => None,
        })
        .collect()
}

fn read_bundle(
    primary: Operation,
    port: &str,
    report: impl FnMut(ProgressEvent),
) -> Result<(serde_json::Value, Result<Vec<String>>)> {
    match primary {
        Operation::Channels => {
            let (channels, endian, labels) = device::read_channels_with_group_labels(port, report)?;
            Ok((serde_json::to_value((channels, endian))?, labels))
        }
        Operation::Scans => {
            let (presets, endian, labels) =
                device::read_scan_presets_with_group_labels(port, report)?;
            Ok((serde_json::to_value((presets, endian))?, labels))
        }
        _ => unreachable!("only channels and scans have bundled read APIs"),
    }
}

fn bundle_blocks(primary: Operation) -> Vec<usize> {
    let mut blocks: Vec<_> = primary.blocks().collect();
    for block in Operation::Labels.blocks() {
        if !blocks.contains(&block) {
            blocks.push(block);
        }
    }
    blocks
}

fn assert_bundle_progress(events: &[ProgressEvent], completed: usize, total: usize) {
    let values = progress(events);
    assert_eq!(
        values,
        (1..=completed)
            .map(|i| i as f64 / total as f64)
            .collect::<Vec<_>>()
    );
    assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn bundled_reads_use_one_session_with_unique_block_requests_and_monotonic_progress() {
    let memory = memory();
    for primary in [Operation::Channels, Operation::Scans] {
        let radio = Radio::new(memory.clone(), Options::default());
        let mut events = Vec::new();
        let (actual, labels) =
            read_bundle(primary, &radio.port(), |event| events.push(event)).unwrap();
        let requests = radio.finish();
        assert_eq!(actual, primary.expected(&memory));
        assert_eq!(labels.unwrap(), extract_group_labels_from_codeplug(&memory));

        let blocks = bundle_blocks(primary);
        let mut expected = opening();
        expected.extend(blocks.iter().map(|block| Request::Read(*block as u8)));
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected
        );
        let reads: Vec<_> = requests
            .iter()
            .filter_map(|(r, _)| match r {
                Request::Read(block) => Some(*block),
                _ => None,
            })
            .collect();
        let expected_count = match primary {
            Operation::Channels => 206,
            Operation::Scans => 20,
            _ => unreachable!(),
        };
        assert_eq!(reads.len(), expected_count);
        assert_eq!(
            reads.iter().collect::<std::collections::HashSet<_>>().len(),
            expected_count
        );
        assert_eq!(reads.iter().filter(|block| **block == 228).count(), 1);
        assert_bundle_progress(&events, blocks.len(), blocks.len());
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
            ProgressEvent::Status(status) if status.starts_with("Opening ")))
                .count(),
            1
        );
    }
}

#[test]
fn bundled_label_failures_preserve_primary_data_and_resume_without_false_completion() {
    const BAD_BLOCK: u8 = 230;
    let memory = memory();
    for primary in [Operation::Channels, Operation::Scans] {
        let radio = Radio::new(
            memory.clone(),
            Options {
                fault: Fault::ChecksumAlways(BAD_BLOCK),
                ..Options::default()
            },
        );
        let mut events = Vec::new();
        let (actual, labels) = read_bundle(primary, &radio.port(), |event| events.push(event))
            .expect("label failure must not discard successfully read primary data");
        let error = format!("{:#}", labels.unwrap_err());
        let requests = radio.finish();
        assert_eq!(actual, primary.expected(&memory));
        assert!(
            error.contains("Failed to read group names block 230"),
            "{error}"
        );
        assert!(error.contains("after retries"), "{error}");

        let blocks = bundle_blocks(primary);
        let completed = blocks
            .iter()
            .position(|block| *block == usize::from(BAD_BLOCK))
            .unwrap();
        let mut expected = opening();
        expected.extend(
            blocks[..completed]
                .iter()
                .map(|block| Request::Read(*block as u8)),
        );
        expected.extend([Request::Read(BAD_BLOCK); 6]);
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected
        );
        assert_bundle_progress(&events, completed, blocks.len());
        assert!(progress(&events).last().unwrap() < &1.0);
        assert!(!events.iter().any(|event| matches!(event,
            ProgressEvent::Status(status) if status.contains("Loaded") && status.contains("group"))));
    }
}

#[test]
fn bundled_resume_failures_are_fatal_even_when_primary_and_labels_were_read() {
    for primary in [Operation::Channels, Operation::Scans] {
        for fault in [
            Fault::ResumeAck,
            Fault::ResumeTimeout,
            Fault::ChecksumAndResume(230),
        ] {
            let radio = Radio::new(
                memory(),
                Options {
                    fault,
                    ..Options::default()
                },
            );
            let mut events = Vec::new();
            let error = read_bundle(primary, &radio.port(), |event| events.push(event))
                .expect_err("unverified radio resume must fail the outer result");
            let requests = radio.finish();
            assert!(
                format!("{error:#}").to_lowercase().contains("resume"),
                "{error:#}"
            );
            assert_eq!(
                requests
                    .iter()
                    .filter(|(r, _)| *r == Request::Resume)
                    .count(),
                2
            );
            assert_eq!(
                requests.iter().filter(|(r, _)| *r == Request::Ping).count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, ProgressEvent::Status(_)))
                    .count(),
                1
            );
            let blocks = bundle_blocks(primary);
            let completed = if matches!(fault, Fault::ChecksumAndResume(_)) {
                blocks.iter().position(|block| *block == 230).unwrap()
            } else {
                blocks.len()
            };
            assert_bundle_progress(&events, completed, blocks.len());
        }
    }
}

#[test]
fn bundled_primary_failures_are_fatal_and_resume_before_any_label_only_reads() {
    for (primary, bad) in [
        (Operation::Channels, 2),
        (Operation::Scans, 216),
        (Operation::Scans, 228),
    ] {
        let radio = Radio::new(
            memory(),
            Options {
                fault: Fault::ChecksumAlways(bad),
                ..Options::default()
            },
        );
        let mut events = Vec::new();
        let error = read_bundle(primary, &radio.port(), |event| events.push(event)).unwrap_err();
        let requests = radio.finish();
        assert!(format!("{error:#}").contains(&format!("Failed to read block {bad}")));
        let mut expected = opening();
        let completed = usize::from(bad) - primary.blocks().start;
        expected.extend(
            primary
                .blocks()
                .take(completed)
                .map(|block| Request::Read(block as u8)),
        );
        expected.extend([Request::Read(bad); 6]);
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected
        );
        assert_bundle_progress(&events, completed, bundle_blocks(primary).len());
    }
}

#[test]
fn bundled_reads_resume_on_unwind_in_both_primary_and_label_progress_callbacks() {
    for primary in [Operation::Channels, Operation::Scans] {
        let blocks = bundle_blocks(primary);
        for cancel_after in [1, primary.blocks().len() + 1] {
            let radio = Radio::new(memory(), Options::default());
            let mut events = Vec::new();
            let mut completed = 0;
            let result = catch_unwind(AssertUnwindSafe(|| {
                read_bundle(primary, &radio.port(), |event| {
                    if matches!(event, ProgressEvent::Progress(_)) {
                        completed += 1;
                    }
                    events.push(event);
                    if completed == cancel_after {
                        panic!("bundled read callback cancelled");
                    }
                })
            }));
            let requests = radio.finish();
            assert!(result.is_err());
            let mut expected = opening();
            expected.extend(
                blocks[..cancel_after]
                    .iter()
                    .map(|block| Request::Read(*block as u8)),
            );
            expected.push(Request::Resume);
            assert_eq!(
                requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
                expected
            );
            assert_bundle_progress(&events, cancel_after, blocks.len());
        }
    }
}

#[test]
fn all_public_reads_pause_read_only_the_requested_blocks_and_verify_resume() {
    let memory = memory();
    for operation in OPERATIONS {
        let radio = Radio::new(memory.clone(), Options::default());
        let mut events = Vec::new();
        let actual = operation
            .read(&radio.port(), |event| events.push(event))
            .unwrap();
        let requests = radio.finish();

        assert_eq!(actual, operation.expected(&memory), "{operation:?}");
        let mut expected = opening();
        expected.extend(operation.blocks().map(|block| Request::Read(block as u8)));
        expected.push(Request::Resume);
        assert_eq!(
            requests
                .iter()
                .map(|(request, _)| *request)
                .collect::<Vec<_>>(),
            expected,
            "{operation:?}"
        );
        let count = operation.blocks().len();
        assert_eq!(
            progress(&events),
            (1..=count)
                .map(|i| i as f64 / count as f64)
                .collect::<Vec<_>>()
        );
        assert!(matches!(events.last(), Some(ProgressEvent::Status(_))));
    }
}

#[test]
fn public_usb_snapshot_has_no_per_block_conservative_delay_budget() {
    let expected = memory();
    let radio = Radio::new(expected.clone(), Options::default());
    let start = Instant::now();
    let (actual, _) = device::read_codeplug(&radio.port(), |_| {}).unwrap();
    let elapsed = start.elapsed();
    radio.finish();
    assert_eq!(actual, expected);
    // The default used to require >5.98 s of mandatory sleeps for 260 blocks.
    // Allow several seconds of scheduling slack, not a tight throughput target.
    assert!(
        elapsed < Duration::from_secs(5),
        "public USB read still has conservative pacing: {elapsed:?}"
    );
}

#[test]
fn public_reads_retry_failed_checksums_and_latch_conservative_fallback() {
    for operation in OPERATIONS {
        let bad = operation.blocks().start as u8;
        let radio = Radio::new(
            memory(),
            Options {
                fault: Fault::ChecksumOnce(bad),
                ..Options::default()
            },
        );
        let mut events = Vec::new();
        operation
            .read(&radio.port(), |event| events.push(event))
            .unwrap();
        let requests = radio.finish();
        let mut expected = opening();
        expected.push(Request::Read(bad));
        expected.extend(operation.blocks().map(|block| Request::Read(block as u8)));
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected,
            "{operation:?}"
        );
        let first = opening().len();
        // Lower bounds cannot fail because of a slow scheduler. The retry and
        // next block must retain protocol recovery and conservative success waits.
        assert!(
            requests[first + 1].1.duration_since(requests[first].1) >= Duration::from_millis(135)
        );
        assert!(
            requests[first + 2].1.duration_since(requests[first + 1].1)
                >= Duration::from_millis(23)
        );
        assert_eq!(progress(&events).len(), operation.blocks().len());
    }
}

#[test]
fn public_read_recovers_from_bad_headers_and_partial_frames() {
    for fault in [Fault::HeaderOnce(200), Fault::TruncatedOnce(200)] {
        let radio = Radio::new(
            memory(),
            Options {
                fault,
                ..Options::default()
            },
        );
        let result = device::read_settings(&radio.port(), |_| {});
        let requests = radio.finish();
        result.unwrap();
        let mut expected = opening();
        expected.push(Request::Read(200));
        expected.extend((200..204).map(Request::Read));
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn every_public_read_resumes_after_callback_unwind() {
    for operation in OPERATIONS {
        let radio = Radio::new(memory(), Options::default());
        let result = catch_unwind(AssertUnwindSafe(|| {
            operation.read(&radio.port(), |event| {
                if matches!(event, ProgressEvent::Progress(_)) {
                    panic!("cancel read callback");
                }
            })
        }));
        let requests = radio.finish();
        assert!(result.is_err());
        let mut expected = opening();
        expected.push(Request::Read(operation.blocks().start as u8));
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected,
            "{operation:?}"
        );
    }
}

#[test]
fn every_public_read_rejects_pause_and_resume_ack_failures_without_success_status() {
    for operation in OPERATIONS {
        for fault in [Fault::PauseAck, Fault::ResumeAck] {
            let radio = Radio::new(
                memory(),
                Options {
                    fault,
                    ..Options::default()
                },
            );
            let mut events = Vec::new();
            let error = operation
                .read(&radio.port(), |event| events.push(event))
                .unwrap_err();
            let requests = radio.finish();
            let mut expected = opening();
            if matches!(fault, Fault::ResumeAck) {
                expected.extend(operation.blocks().map(|block| Request::Read(block as u8)));
                // Explicit ACK failure leaves the Drop guard armed for one retry.
                expected.push(Request::Resume);
                assert!(error.to_string().contains("read resume"), "{error:#}");
            } else {
                assert!(error.to_string().contains("read pause"), "{error:#}");
                assert!(progress(&events).is_empty());
            }
            expected.push(Request::Resume);
            assert_eq!(
                requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
                expected,
                "{operation:?}"
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, ProgressEvent::Status(_)))
                    .count(),
                1
            );
        }
    }
}

#[test]
fn pause_and_resume_timeouts_fail_and_attempt_cleanup() {
    for fault in [Fault::PauseTimeout, Fault::ResumeTimeout] {
        let radio = Radio::new(
            memory(),
            Options {
                fault,
                ..Options::default()
            },
        );
        let error = device::read_settings(&radio.port(), |_| {}).unwrap_err();
        let requests = radio.finish();
        assert!(format!("{error:#}").contains("Timeout waiting for response"));
        let mut expected = opening();
        if matches!(fault, Fault::ResumeTimeout) {
            expected.extend((200..204).map(Request::Read));
            expected.push(Request::Resume);
        }
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn exhausted_retries_resume_and_preserve_both_read_and_resume_errors() {
    for fault in [Fault::ChecksumAlways(201), Fault::ChecksumAndResume(201)] {
        let radio = Radio::new(
            memory(),
            Options {
                fault,
                ..Options::default()
            },
        );
        let mut events = Vec::new();
        let error = device::read_settings(&radio.port(), |event| events.push(event)).unwrap_err();
        let requests = radio.finish();
        let error = format!("{error:#}");
        assert!(
            error.contains("Failed to read block 201 after retries"),
            "{error}"
        );
        let mut expected = opening();
        expected.push(Request::Read(200));
        expected.extend([Request::Read(201); 6]);
        if matches!(fault, Fault::ChecksumAndResume(_)) {
            expected.push(Request::Resume);
            assert!(error.contains("Radio resume also failed"), "{error}");
        }
        expected.push(Request::Resume);
        assert_eq!(
            requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(progress(&events), vec![0.25]);
        assert!(!matches!(events.last(), Some(ProgressEvent::Status(_))));
    }
}

#[test]
fn unsupported_firmware_never_enters_responsive_session() {
    let mut memory = memory();
    memory[0x1900..0x1902].fill(0);
    let radio = Radio::new(memory, Options::default());
    let error = device::read_codeplug(&radio.port(), |_| {}).unwrap_err();
    assert!(error.to_string().contains("unsupported image"));
    let requests = radio.finish();
    assert_eq!(
        requests.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        &opening()[..5]
    );
}

#[test]
#[ignore = "emulated public-API benchmark; no physical radio access"]
fn benchmark_public_snapshot_against_explicit_conservative_timing() {
    let expected = memory();
    let mut results = Vec::new();
    for responsive in [false, true] {
        let radio = Radio::new(
            expected.clone(),
            Options {
                response_delay: Duration::from_millis(9),
                ..Options::default()
            },
        );
        let start = Instant::now();
        let (bytes, endian) = if responsive {
            device::read_codeplug(&radio.port(), |_| {}).unwrap()
        } else {
            device::read_codeplug_with_timing(&radio.port(), ReadTiming::Conservative, |_| {})
                .unwrap()
        };
        let elapsed = start.elapsed();
        let requests = radio.finish();
        assert_eq!(bytes.len(), EEPROM_SIZE);
        assert_eq!(bytes, expected);
        assert_eq!(endian, Endianness::Big);
        assert_eq!(
            requests
                .iter()
                .filter(|(r, _)| matches!(r, Request::Read(_)))
                .count(),
            TOTAL_BLOCKS + 4
        );
        println!(
            "emulated public snapshot {}: {elapsed:.3?}, {} identical bytes",
            if responsive {
                "default USB"
            } else {
                "explicit conservative"
            },
            bytes.len()
        );
        results.push(bytes);
    }
    assert_eq!(results[0], results[1]);
}
