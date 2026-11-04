#![cfg(unix)]

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nictui::device::{ProgressEvent, read_codeplug, read_codeplug_with_timing};
use nictui::protocol::radio::ReadTiming;
use nictui::protocol::{BLOCK_SIZE, EEPROM_SIZE, Endianness, TOTAL_BLOCKS};
use serialport::{SerialPort, TTYPort};

const PING: u8 = 0x01;
const READ: u8 = 0x30;
const PAUSE: u8 = 0x45;
const RESUME: u8 = 0x46;

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Ping,
    Read(u8),
    Pause,
    Resume,
}

struct EmulatedRadio {
    // Keep the slave open while the device API reopens it by name.
    slave: TTYPort,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<Vec<Request>>>>,
}

impl EmulatedRadio {
    fn new(snapshot: Vec<u8>, corrupt_block: Option<u8>) -> Self {
        // pair() does not lock the slave; set_exclusive(false) would acquire a
        // shared flock and prevent the device API's initial exclusive open.
        let (mut master, slave) = TTYPort::pair().expect("create radio PTY pair");
        master
            .set_timeout(Duration::from_millis(50))
            .expect("set emulator timeout");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            while !worker_stop.load(Ordering::Relaxed) {
                let mut command = [0];
                match master.read_exact(&mut command) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => return Err(error),
                }
                match command[0] {
                    PING | PAUSE | RESUME => {
                        requests.push(match command[0] {
                            PING => Request::Ping,
                            PAUSE => Request::Pause,
                            _ => Request::Resume,
                        });
                        master.write_all(&command)?;
                    }
                    READ => {
                        let mut block = [0];
                        master.read_exact(&mut block)?;
                        requests.push(Request::Read(block[0]));
                        let start = usize::from(block[0]) * BLOCK_SIZE;
                        let data = &snapshot[start..start + BLOCK_SIZE];
                        let mut checksum =
                            data.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
                        if corrupt_block == Some(block[0]) {
                            checksum = checksum.wrapping_add(1);
                        }
                        let mut response = Vec::with_capacity(BLOCK_SIZE + 2);
                        response.push(READ);
                        response.extend_from_slice(data);
                        response.push(checksum);
                        master.write_all(&response)?;
                    }
                    command => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("unexpected radio command {command:#04x}"),
                        ));
                    }
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

    fn port_name(&self) -> String {
        self.slave.name().expect("slave PTY has a path")
    }

    fn finish(mut self) -> Vec<Request> {
        self.stop.store(true, Ordering::Relaxed);
        self.worker
            .take()
            .unwrap()
            .join()
            .expect("radio emulator thread panicked")
            .expect("radio emulator I/O failed")
    }
}

impl Drop for EmulatedRadio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn snapshot_bytes(endian: Endianness) -> Vec<u8> {
    let mut snapshot: Vec<u8> = (0..EEPROM_SIZE)
        .map(|offset| ((offset * 37) ^ (offset >> 8) ^ (offset >> 3)) as u8)
        .collect();
    snapshot[0x1900..0x1902].copy_from_slice(&[0xD8, 0x2F]);
    snapshot[240 * BLOCK_SIZE] = match endian {
        Endianness::Little => 0x57,
        Endianness::Big => 0x00,
    };
    snapshot
}

fn opening_requests() -> Vec<Request> {
    let mut requests = vec![Request::Ping];
    requests.extend((200..=203).map(Request::Read));
    requests.push(Request::Pause);
    requests
}

fn progress_values(events: &[ProgressEvent]) -> Vec<f64> {
    events
        .iter()
        .filter_map(|event| match event {
            ProgressEvent::Progress(value) => Some(*value),
            ProgressEvent::Status(_) => None,
        })
        .collect()
}

#[test]
fn public_snapshot_reads_every_byte_and_resumes_radio() {
    for expected_endian in [Endianness::Big, Endianness::Little] {
        let expected = snapshot_bytes(expected_endian);
        let radio = EmulatedRadio::new(expected.clone(), None);
        let mut events = Vec::new();

        let result = read_codeplug(&radio.port_name(), |event| events.push(event));
        let requests = radio.finish();
        let (snapshot, endian) = result.expect("read full emulated snapshot");

        assert_eq!(snapshot, expected);
        assert_eq!(endian, expected_endian);
        let mut expected_requests = opening_requests();
        expected_requests.extend((0..TOTAL_BLOCKS).map(|block| Request::Read(block as u8)));
        expected_requests.push(Request::Resume);
        assert_eq!(requests, expected_requests);
        assert_eq!(
            progress_values(&events),
            (1..=TOTAL_BLOCKS)
                .map(|block| block as f64 / TOTAL_BLOCKS as f64)
                .collect::<Vec<_>>()
        );
        assert!(matches!(events.last(), Some(ProgressEvent::Status(message))
            if message == &format!("Read {EEPROM_SIZE} bytes from radio")));
    }
}

#[test]
fn explicit_conservative_snapshot_remains_available_without_pause() {
    let expected = snapshot_bytes(Endianness::Little);
    let radio = EmulatedRadio::new(expected.clone(), None);
    let (actual, endian) =
        read_codeplug_with_timing(&radio.port_name(), ReadTiming::Conservative, |_| {})
            .expect("conservative snapshot remains supported");
    let requests = radio.finish();

    assert_eq!(actual, expected);
    assert_eq!(endian, Endianness::Little);
    let mut expected_requests = vec![Request::Ping];
    expected_requests.extend((200..=203).map(Request::Read));
    expected_requests.extend((0..TOTAL_BLOCKS).map(|block| Request::Read(block as u8)));
    assert_eq!(requests, expected_requests);
}

#[test]
fn responsive_snapshot_resumes_radio_after_checksum_retries_are_exhausted() {
    const BAD_BLOCK: u8 = 7;
    let radio = EmulatedRadio::new(snapshot_bytes(Endianness::Big), Some(BAD_BLOCK));
    let mut events = Vec::new();

    let result = read_codeplug_with_timing(&radio.port_name(), ReadTiming::Responsive, |event| {
        events.push(event)
    });
    let requests = radio.finish();
    let error = format!(
        "{:#}",
        result.expect_err("bad checksums must fail the snapshot")
    );

    assert!(error.contains("Failed to read block 7"), "{error}");
    assert!(error.contains("after retries"), "{error}");
    let mut expected_requests = opening_requests();
    expected_requests.extend((0..BAD_BLOCK).map(Request::Read));
    expected_requests.extend((0..6).map(|_| Request::Read(BAD_BLOCK)));
    expected_requests.push(Request::Resume);
    assert_eq!(requests, expected_requests);
    assert_eq!(
        progress_values(&events),
        (1..=BAD_BLOCK)
            .map(|block| f64::from(block) / TOTAL_BLOCKS as f64)
            .collect::<Vec<_>>()
    );
    assert!(!events.iter().any(|event| matches!(event,
        ProgressEvent::Status(message) if message.starts_with("Read "))));
}

#[test]
fn responsive_snapshot_resumes_radio_when_progress_callback_panics() {
    let radio = EmulatedRadio::new(snapshot_bytes(Endianness::Big), None);
    let port = radio.port_name();
    let mut progress = Vec::new();

    let result = catch_unwind(AssertUnwindSafe(|| {
        read_codeplug_with_timing(&port, ReadTiming::Responsive, |event| {
            if let ProgressEvent::Progress(value) = event {
                progress.push(value);
                panic!("snapshot progress callback panic");
            }
        })
    }));
    let requests = radio.finish();
    let panic = result.expect_err("progress callback panic must propagate");

    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"snapshot progress callback panic")
    );
    assert_eq!(progress, vec![1.0 / TOTAL_BLOCKS as f64]);
    let mut expected_requests = opening_requests();
    expected_requests.push(Request::Read(0));
    expected_requests.push(Request::Resume);
    assert_eq!(requests, expected_requests);
}

#[test]
fn responsive_ble_snapshot_is_rejected_before_any_progress_or_connection() {
    for port in [
        "ble://snapshot-test-no-device",
        "ble:snapshot-test-no-device",
    ] {
        let error = read_codeplug_with_timing(port, ReadTiming::Responsive, |_| {
            panic!("BLE rejection must precede progress callbacks and connection setup");
        })
        .expect_err("responsive timing is serial-only");

        assert_eq!(
            error.to_string(),
            "Response-paced reads currently require USB serial; use conservative timing for BLE"
        );
    }
}
