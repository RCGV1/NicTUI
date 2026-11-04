#![cfg(unix)]

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nictui::protocol::{BLOCK_SIZE, EEPROM_SIZE, RadioProtocol};
use serialport::{SerialPort, TTYPort};

struct Directory(PathBuf);

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nictui-cancellation-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn temporary_directories_are_unique_under_parallel_creation() {
    let workers: Vec<_> = (0..16).map(|_| thread::spawn(Directory::new)).collect();
    let directories: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    let paths: std::collections::HashSet<_> = directories.iter().map(|dir| &dir.0).collect();
    assert_eq!(paths.len(), directories.len());
}

struct Process(Option<Child>);

impl Process {
    fn spawn(mut command: Command) -> Self {
        Self(Some(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }

    fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }

    fn signal(&mut self, signal: &str) {
        assert!(
            Command::new("/bin/kill")
                .args(["-s", signal, &self.child().id().to_string()])
                .status()
                .unwrap()
                .success()
        );
    }

    fn assert_running(&mut self, message: &str) {
        if let Some(status) = self.child().try_wait().unwrap() {
            let mut stderr = String::new();
            self.child()
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            panic!("{message}: {status}; stderr: {stderr}");
        }
    }

    fn interrupt(&mut self, signal: &str) {
        self.signal(signal);
        // Allow the dedicated handler thread to observe the signal before replying.
        thread::sleep(Duration::from_millis(75));
        self.assert_running("signal abandoned in-flight I/O");
    }

    fn output(mut self) -> Output {
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            // Failure-only cleanup of this test's fake-radio child, never a real session.
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Ping,
    Pause,
    Resume,
    Read(u8),
    Write(u8),
}

#[derive(Clone, Copy)]
enum InterruptAt {
    PausedRead,
    Resume,
    PreservationRead,
    FirstWrite,
}

fn memory() -> Vec<u8> {
    let mut data = vec![0; EEPROM_SIZE];
    data[0x1900..0x1902].copy_from_slice(&0xD82Fu16.to_be_bytes());
    data[0x1A00..0x1A02].copy_from_slice(&0xA46Du16.to_be_bytes());
    data
}

fn nictui(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
    command.args(args);
    command
}

fn drive_memory(
    mut command: Command,
    signal: &str,
    interrupt_at: InterruptAt,
    fail_first_resume: bool,
) -> (Output, Vec<Request>, Vec<u8>) {
    let (mut master, slave) = TTYPort::pair().unwrap();
    master.set_timeout(Duration::from_millis(50)).unwrap();
    command.args(["--port", &slave.name().unwrap()]);
    let mut process = Process::spawn(command);
    let mut data = memory();
    let mut trace = Vec::new();
    let mut paused = false;
    let mut signalled = false;
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "child shutdown stalled: {trace:?}"
        );
        if process.child().try_wait().unwrap().is_some() {
            break;
        }
        let mut byte = [0];
        match master.read_exact(&mut byte) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            Err(error) => panic!("PTY receive failed: {error}"),
        }
        let mut response = byte.to_vec();
        let request = match byte[0] {
            0x01 => Request::Ping,
            0x45 => {
                paused = true;
                Request::Pause
            }
            0x46 => {
                if fail_first_resume && !trace.contains(&Request::Resume) {
                    response[0] = 0;
                } else {
                    paused = false;
                }
                Request::Resume
            }
            0x30 => {
                let mut block = [0];
                master.read_exact(&mut block).unwrap();
                let offset = usize::from(block[0]) * BLOCK_SIZE;
                let bytes = &data[offset..offset + BLOCK_SIZE];
                response.extend_from_slice(bytes);
                response.push(bytes.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
                Request::Read(block[0])
            }
            0x31 => {
                let mut body = [0; BLOCK_SIZE + 2];
                master.read_exact(&mut body).unwrap();
                assert_eq!(
                    body[BLOCK_SIZE + 1],
                    body[1..=BLOCK_SIZE]
                        .iter()
                        .fold(0u8, |sum, b| sum.wrapping_add(*b))
                );
                let offset = usize::from(body[0]) * BLOCK_SIZE;
                data[offset..offset + BLOCK_SIZE].copy_from_slice(&body[1..=BLOCK_SIZE]);
                Request::Write(body[0])
            }
            other => panic!("unexpected radio command {other:02X}"),
        };
        let should_interrupt = match interrupt_at {
            InterruptAt::PausedRead => paused && matches!(request, Request::Read(_)),
            InterruptAt::Resume => request == Request::Resume,
            InterruptAt::PreservationRead => {
                request == Request::Read(200)
                    && trace.iter().filter(|&&r| r == Request::Ping).count() == 2
                    && trace.ends_with(&[
                        Request::Ping,
                        Request::Read(200),
                        Request::Read(201),
                        Request::Read(202),
                        Request::Read(203),
                    ])
            }
            InterruptAt::FirstWrite => matches!(request, Request::Write(_)),
        };
        trace.push(request);
        if should_interrupt && !signalled {
            if matches!(interrupt_at, InterruptAt::PreservationRead) {
                let session_start = trace.iter().rposition(|&r| r == Request::Ping).unwrap();
                // The first connection is CLI probing. On the second connection,
                // identification has completed and preservation is now awaiting its first reply.
                assert_eq!(
                    trace[..session_start]
                        .iter()
                        .filter(|&&r| r == Request::Ping)
                        .count(),
                    1
                );
                assert_eq!(
                    &trace[session_start..],
                    &[
                        Request::Ping,
                        Request::Read(200),
                        Request::Read(201),
                        Request::Read(202),
                        Request::Read(203),
                        Request::Read(200),
                    ]
                );
                assert!(!trace.iter().any(|r| matches!(r, Request::Write(_))));
            }
            process.interrupt(signal);
            // Repeated signals are cooperative too, not an implicit hard-exit path.
            process.interrupt(signal);
            signalled = true;
        }
        if request == Request::Resume {
            thread::sleep(Duration::from_millis(50));
            process.assert_running(&format!("exit preceded resume ACK; trace: {trace:?}"));
        }
        master.write_all(&response).unwrap();
    }
    let output = process.output();
    assert!(
        signalled,
        "test never reached its interruption boundary: {trace:?}; {output:?}"
    );
    (output, trace, data)
}

fn assert_interrupted(output: &Output) {
    assert!(!output.status.success());
    assert_eq!(
        output.status.code(),
        Some(1),
        "child died from a signal: {output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Interrupted"),
        "{output:?}"
    );
}

#[test]
fn signals_during_paused_cli_reads_resume_before_exit_without_backup() {
    for signal in ["INT", "TERM"] {
        for fail_resume in [false, true] {
            let directory = Directory::new();
            let path = directory.0.join("radio.nfw");
            let mut command = nictui(&["codeplug", "read"]);
            command.arg("--output").arg(&path);
            let (output, trace, after) =
                drive_memory(command, signal, InterruptAt::PausedRead, fail_resume);
            assert_interrupted(&output);
            assert!(!path.exists(), "interrupted read created a backup");
            assert_eq!(after, memory());
            assert_eq!(trace.last(), Some(&Request::Resume));
            assert_eq!(
                trace.iter().filter(|&&r| r == Request::Resume).count(),
                if fail_resume { 2 } else { 1 }
            );
            assert_eq!(
                trace
                    .iter()
                    .filter(|r| matches!(r, Request::Read(_)))
                    .count(),
                5,
                "read did not stop at the next block boundary"
            );
        }
    }
}

#[test]
fn signal_during_resume_does_not_overwrite_existing_backup() {
    for signal in ["INT", "TERM"] {
        let directory = Directory::new();
        let path = directory.0.join("existing.nfw");
        std::fs::write(&path, b"existing backup").unwrap();
        let mut command = nictui(&["codeplug", "read"]);
        command.arg("--output").arg(&path);
        let (output, trace, after) = drive_memory(command, signal, InterruptAt::Resume, false);
        assert_interrupted(&output);
        assert_eq!(trace.last(), Some(&Request::Resume));
        assert_eq!(std::fs::read(path).unwrap(), b"existing backup");
        assert_eq!(after, memory());
    }
}

#[test]
fn radio_lab_fast_read_also_resumes_before_interrupted_exit() {
    for signal in ["INT", "TERM"] {
        let directory = Directory::new();
        let path = directory.0.join("radio.nfw");
        let mut command = Command::new(env!("CARGO_BIN_EXE_radio_lab"));
        command.args(["read", "--fast", "--output"]).arg(&path);
        let (output, trace, after) = drive_memory(command, signal, InterruptAt::PausedRead, false);
        assert_interrupted(&output);
        assert_eq!(trace.last(), Some(&Request::Resume));
        assert!(!path.exists());
        assert_eq!(after, memory());
    }
}

#[test]
fn signals_do_not_interrupt_write_preservation_reads_or_writes() {
    for signal in ["INT", "TERM"] {
        for at in [InterruptAt::PreservationRead, InterruptAt::FirstWrite] {
            let directory = Directory::new();
            let path = directory.0.join("settings.json");
            let before = memory();
            let mut settings = RadioProtocol::parse_settings_block(
                &before[0x1900..0x1980],
                nictui::protocol::Endianness::Big,
            );
            for (index, meta) in nictui::protocol::SETTINGS_METADATA.iter().enumerate() {
                if let nictui::protocol::SettingType::Numeric { min, .. } = meta.setting_type {
                    settings.set_value(index, u32::try_from(min).unwrap());
                }
            }
            std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
            let mut command = nictui(&["settings", "write", "--no-reboot", "--input"]);
            command.arg(path);
            let (output, trace, _) = drive_memory(command, signal, at, false);
            assert_interrupted(&output);
            let written: Vec<_> = trace
                .iter()
                .filter_map(|r| match r {
                    Request::Write(b) => Some(*b),
                    _ => None,
                })
                .collect();
            assert_eq!(
                written,
                [200, 201, 202, 203],
                "write was abandoned: {trace:?}"
            );
        }
    }
}

#[test]
fn signals_during_flash_wait_for_final_packet_ack() {
    for signal in ["INT", "TERM"] {
        let directory = Directory::new();
        let path = directory.0.join("firmware.bin");
        std::fs::write(&path, [0x42; 64]).unwrap();
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_secs(3)).unwrap();
        let mut command = nictui(&["firmware", "flash", "--input"]);
        command.arg(&path).args(["--port", &slave.name().unwrap()]);
        let mut process = Process::spawn(command);
        let mut packet = [0; 36];
        master.set_timeout(Duration::from_millis(50)).unwrap();
        let handshake_started = Instant::now();
        loop {
            assert!(
                handshake_started.elapsed() < Duration::from_secs(3),
                "flash initialization stalled"
            );
            process.assert_running("flash child exited before initialization");
            // A bootloader repeats A5 until the programmer's initialization burst.
            // A one-shot A5 can be discarded by the child's initial port clear.
            master.write_all(&[0xA5]).unwrap();
            match master.read_exact(&mut packet[..1]) {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => panic!("flash initialization receive failed: {error}"),
            }
        }
        master.set_timeout(Duration::from_secs(3)).unwrap();
        master.read_exact(&mut packet[1..]).unwrap();
        assert_eq!(packet[0], 0xA0);
        if signal == "INT" {
            master.write_all(&[0xA3]).unwrap();
        }
        for index in 0..2u16 {
            master.read_exact(&mut packet).unwrap();
            assert_eq!(packet[0], if index == 0 { 0xA1 } else { 0xA2 });
            assert_eq!(&packet[1..3], &index.to_be_bytes());
            if index == 0 {
                process.interrupt(signal);
            }
            assert!(
                process.child().try_wait().unwrap().is_none(),
                "flash exited before ACK"
            );
            master.write_all(&[0xA3]).unwrap();
        }
        let started = Instant::now();
        while process.child().try_wait().unwrap().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "flash child did not exit"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_interrupted(&process.output());
    }
}

#[test]
fn radio_lab_activity_signal_closes_port_without_backup_or_mutations() {
    for signal in ["INT", "TERM"] {
        let directory = Directory::new();
        let path = directory.0.join("activity.csv");
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_secs(3)).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_radio_lab"));
        command
            .args(["activity", "--duration", "60", "--output"])
            .arg(&path)
            .args(["--port", &slave.name().unwrap()]);
        let mut process = Process::spawn(command);
        let mut byte = [0];
        master.read_exact(&mut byte).unwrap();
        assert_eq!(byte, [0x02]);
        // Passive activity has no paused state or cleanup ACK to wait for.
        process.signal(signal);
        master.write_all(&byte).unwrap();
        master.set_timeout(Duration::from_millis(50)).unwrap();
        let started = Instant::now();
        while process.child().try_wait().unwrap().is_none() {
            assert!(started.elapsed() < Duration::from_secs(3));
            match master.read(&mut byte) {
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                other => panic!("activity emitted another command: {other:?}"),
            }
        }
        assert_interrupted(&process.output());
        assert!(!path.exists());
    }
}
