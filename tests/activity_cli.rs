use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use clap::Parser;
use nictui::cli::{ActivityCommand, Cli, Commands, DemoView};
use serde_json::Value;

static PROCESS_SPAWN_LOCK: Mutex<()> = Mutex::new(());

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nictui-activity-cli-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn nictui(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
    command.args(args);
    command
}

fn command_output(mut command: Command) -> Output {
    let child = {
        let _guard = PROCESS_SPAWN_LOCK.lock().unwrap();
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    child.wait_with_output().unwrap()
}

fn hit(frequency: u32, signal: u8) -> Vec<u8> {
    let mut bytes = vec![0x2A];
    bytes.extend(frequency.to_le_bytes());
    bytes.push(signal);
    bytes
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("not a single JSON object: {error}; {output:?}"))
}

#[test]
fn legacy_remote_vm_is_rejected_before_opening_an_invalid_port() {
    let directory = Directory::new();
    let invalid_port = directory.path("not-a-radio-port");
    for args in [
        vec!["remote", "key", "--key", "vm"],
        vec!["remote", "capture", "--send", "vm"],
        vec![
            "remote", "capture", "--send", "menu", "--send", "vm", "--send", "ptt-b",
        ],
        vec!["remote", "probe", "--preset", "vm"],
        vec!["remote", "matrix", "--preset", "vm"],
    ] {
        let mut command = nictui(&args);
        command.arg("--port").arg(&invalid_port);
        let output = command_output(command);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("use exit for orange V/M button; use explicit ptt-b for transmission"),
            "{args:?}: {stderr}"
        );
        assert!(
            !stderr.contains(invalid_port.to_str().unwrap()),
            "port was opened before rejecting vm: {args:?}: {stderr}"
        );
    }
}

#[test]
fn clap_monitor_defaults_and_explicit_ble_targets() {
    for target in ["--port", "--ble-device", "--ble-name"] {
        let cli =
            Cli::try_parse_from(["nictui", "activity", "monitor", target, "example", "--json"])
                .unwrap();
        let Some(Commands::Activity {
            command: ActivityCommand::Monitor(args),
        }) = cli.command
        else {
            panic!("wrong command")
        };
        assert_eq!(args.duration, 30);
        assert!(args.json);
    }
    assert!(
        Cli::try_parse_from([
            "nictui",
            "activity",
            "monitor",
            "--port",
            "fake",
            "--duration",
            "1"
        ])
        .is_ok()
    );
}

#[test]
fn clap_rejects_zero_negative_fractional_duration_missing_target_and_conflicts() {
    for duration in ["0", "-1", "0.5", "nope"] {
        assert!(
            Cli::try_parse_from([
                "nictui",
                "activity",
                "monitor",
                "--port",
                "fake",
                "--duration",
                duration
            ])
            .is_err()
        );
    }
    assert!(Cli::try_parse_from(["nictui", "activity", "monitor"]).is_err());
    for targets in [
        ["--port", "--ble-name"],
        ["--port", "--ble-device"],
        ["--ble-name", "--ble-device"],
    ] {
        assert!(
            Cli::try_parse_from([
                "nictui", "activity", "monitor", targets[0], "a", targets[1], "b"
            ])
            .is_err()
        );
    }
}

#[test]
fn clap_replay_is_offline_and_requires_input() {
    assert!(Cli::try_parse_from(["nictui", "activity", "replay"]).is_err());
    assert!(
        Cli::try_parse_from([
            "nictui",
            "activity",
            "replay",
            "--input",
            "raw.bin",
            "--output",
            "activity.csv",
            "--json"
        ])
        .is_ok()
    );
    for option in ["--port", "--ble-name", "--ble-device", "--duration"] {
        assert!(
            Cli::try_parse_from([
                "nictui", "activity", "replay", "--input", "raw.bin", option, "fake"
            ])
            .is_err()
        );
    }
    let cli = Cli::try_parse_from(["nictui", "tui", "--demo", "--demo-view", "activity"]).unwrap();
    assert!(
        matches!(cli.command, Some(Commands::Tui(args)) if args.demo_view == DemoView::Activity)
    );
}

#[test]
fn public_help_explains_manual_setup_and_signal_units() {
    for command in [
        vec!["activity", "--help"],
        vec!["activity", "monitor", "--help"],
        vec!["activity", "replay", "--help"],
    ] {
        let output = command_output(nictui(&command));
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(
            help.contains("Start a VFO preset scan, then enable Activity Mode"),
            "{help}"
        );
        assert!(help.contains("receive-only"), "{help}");
        assert!(help.contains("0x02"), "{help}");
        assert!(help.contains("NOT dBm"), "{help}");
        assert!(!help.contains("auto-select"), "{help}");
        for private in ["PVOJH", "Debug", "ActivityMonitorOptions"] {
            assert!(!help.contains(private), "{help}");
        }
    }
}

fn replay(input: &Path, output: &Path, json: bool) -> Output {
    let mut command = nictui(&["activity", "replay"]);
    command
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(output);
    if json {
        command.arg("--json");
    }
    command_output(command)
}

#[test]
fn replay_json_and_csv_aggregate_hits_and_payload_markers() {
    let dir = Directory::new();
    let input = dir.path("raw.bin");
    let output = dir.path("activity.csv");
    let marker_frequency = u32::from_le_bytes([0x2A, 0x2B, 0x02, 0]);
    let mut bytes = vec![0, 0x02, 0x2B];
    bytes.extend(hit(14_535_000, 40));
    bytes.extend(hit(marker_frequency, 0x2B));
    bytes.extend(hit(14_535_000, 61));
    bytes.push(0x2B);
    std::fs::write(&input, bytes).unwrap();
    let result = replay(&input, &output, true);
    assert!(result.status.success(), "{result:?}");
    let json = report(&result);
    assert_eq!(json["complete"], true);
    assert_eq!(json["hit_count"], 3);
    assert_eq!(json["heartbeat_count"], 2);
    assert_eq!(json["truncated_frames"], 0);
    let rows = json["frequencies"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let row = rows
        .iter()
        .find(|row| row["frequency_10hz"] == 14_535_000)
        .unwrap();
    assert_eq!(row["frequency_mhz"], "145.35000");
    assert_eq!(row["count"], 2);
    assert_eq!(row["mean_signal_raw"], 50.5);
    for key in ["first_seen", "last_seen"] {
        chrono::DateTime::parse_from_rfc3339(row[key].as_str().unwrap()).unwrap();
    }
    let csv = std::fs::read_to_string(output).unwrap();
    assert!(csv.starts_with("Frequency,Count,Signal\r\n"));
    assert!(csv.contains("145.35000,2,050.50\r\n"), "{csv}");
    assert!(!String::from_utf8_lossy(&result.stderr).contains("Discovering"));
}

#[test]
fn replay_split_read_chunks_and_plain_summary() {
    let dir = Directory::new();
    let input = dir.path("split.bin");
    let output = dir.path("activity.csv");
    let mut bytes = vec![0; 8191];
    bytes.extend(hit(14_535_000, 2));
    std::fs::write(&input, bytes).unwrap();
    let result = replay(&input, &output, false);
    assert!(result.status.success(), "{result:?}");
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("145.35000 MHz"), "{text}");
    assert!(text.contains("count 1"), "{text}");
    assert!(text.contains("raw, not dBm"), "{text}");
}

#[test]
fn replay_incomplete_frames_preserve_existing_output() {
    let dir = Directory::new();
    let input = dir.path("truncated.bin");
    let output = dir.path("activity.csv");
    for payload_bytes in 0..5 {
        let mut bytes = hit(14_535_000, 60);
        bytes.push(0x2A);
        bytes.extend(std::iter::repeat_n(0x02, payload_bytes));
        std::fs::write(&input, bytes).unwrap();
        std::fs::write(&output, "preserve me").unwrap();
        let result = replay(&input, &output, true);
        assert!(!result.status.success(), "{result:?}");
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Incomplete activity recording"));
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "preserve me");
    }
}

#[test]
fn replay_frequency_limit_preserves_existing_output() {
    let dir = Directory::new();
    let input = dir.path("many.bin");
    let output = dir.path("activity.csv");
    let bytes: Vec<_> = (0..=4096)
        .flat_map(|frequency| hit(frequency, 42))
        .collect();
    std::fs::write(&input, bytes).unwrap();
    std::fs::write(&output, "preserve me").unwrap();
    let result = replay(&input, &output, true);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("frequency limit"));
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "preserve me");
}

#[cfg(unix)]
mod pty {
    use super::*;
    use serialport::{SerialPort, TTYPort};
    use std::ffi::c_int;
    use std::io::{self, Read, Write};
    use std::os::fd::AsRawFd;
    use std::process::{Child, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
        fn kill(pid: c_int, signal: c_int) -> c_int;
    }

    struct Process(Option<Child>);
    impl Process {
        fn interrupt(&mut self) {
            let pid = c_int::try_from(self.0.as_ref().unwrap().id()).unwrap();
            assert!(pid > 0);
            // SAFETY: this unreaped positive PID identifies only our fixture child.
            assert_eq!(unsafe { kill(pid, 2) }, 0);
        }

        fn output(mut self) -> Output {
            let child = self.0.as_mut().unwrap();
            let started = Instant::now();
            while child.try_wait().unwrap().is_none() {
                assert!(
                    started.elapsed() < Duration::from_secs(8),
                    "CLI did not stop"
                );
                thread::sleep(Duration::from_millis(10));
            }
            self.0.take().unwrap().wait_with_output().unwrap()
        }
    }
    impl Drop for Process {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn spawn(output: &Path, json: bool) -> (TTYPort, TTYPort, Process) {
        spawn_with_duration(output, json, "1")
    }

    fn spawn_with_duration(
        output: &Path,
        json: bool,
        duration: &str,
    ) -> (TTYPort, TTYPort, Process) {
        // pair() does not set CLOEXEC atomically. Serialize every child spawn
        // against this window so sibling fake radios cannot inherit the master.
        let _guard = PROCESS_SPAWN_LOCK.lock().unwrap();
        let (mut master, slave) = TTYPort::pair().unwrap();
        for port in [&master, &slave] {
            // TTYPort::pair does not set CLOEXEC. Do not let the child retain
            // its own fake-radio master, which would mask a transport failure.
            // SAFETY: F_SETFD (2) and FD_CLOEXEC (1) operate on owned valid FDs.
            assert_eq!(unsafe { fcntl(port.as_raw_fd(), 2, 1) }, 0);
        }
        master.set_timeout(Duration::from_millis(50)).unwrap();
        let mut command = nictui(&["activity", "monitor", "--duration", duration]);
        command
            .arg("--port")
            .arg(slave.name().unwrap())
            .arg("--output")
            .arg(output);
        if json {
            command.arg("--json");
        }
        let process = Process(Some(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        (master, slave, process)
    }

    fn ping(master: &mut TTYPort) {
        let start = Instant::now();
        loop {
            let mut byte = [0];
            match master.read_exact(&mut byte) {
                Ok(()) => {
                    assert_eq!(byte, [0x02], "activity must only transmit 02");
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                    assert!(start.elapsed() < Duration::from_secs(3))
                }
                Err(error) => panic!("PTY read: {error}"),
            }
        }
    }

    fn assert_no_writes(master: &mut TTYPort) {
        let mut byte = [0];
        match master.read(&mut byte) {
            Ok(0) => (),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => (),
            other => panic!("unexpected radio write after ping: {byte:?}; {other:?}"),
        }
    }

    #[test]
    fn monitor_only_pings_and_decodes_split_frames_including_pre_ack_payload_02() {
        let dir = Directory::new();
        let csv = dir.path("capture.csv");
        let (mut master, _slave, process) = spawn(&csv, true);
        ping(&mut master);
        master.write_all(&[0x2B, 0x2A, 0x2A]).unwrap();
        thread::sleep(Duration::from_millis(20));
        master.write_all(&[0x2B, 0x02, 0, 0x02]).unwrap();
        thread::sleep(Duration::from_millis(20));
        master.write_all(&[0x02]).unwrap();
        let frame = hit(14_535_000, 40);
        master.write_all(&frame[..3]).unwrap();
        thread::sleep(Duration::from_millis(20));
        master.write_all(&frame[3..]).unwrap();
        master.write_all(&hit(14_535_000, 60)).unwrap();
        master.write_all(&[0x2B, 0x2A, 1, 2]).unwrap();
        let result = process.output();
        assert!(result.status.success(), "{result:?}");
        assert_no_writes(&mut master);
        let json = report(&result);
        assert_eq!(json["hit_count"], 3);
        assert_eq!(json["heartbeat_count"], 2);
        assert_eq!(json["truncated_frames"], 1);
        assert_eq!(json["frequencies"].as_array().unwrap().len(), 2);
        assert!(
            std::fs::read_to_string(csv)
                .unwrap()
                .contains("145.35000,2,050.00\r\n")
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains("Warning:"));
    }

    #[test]
    fn capacity_failure_reports_and_outputs_only_retained_hits() {
        use std::io::BufRead;
        use std::sync::mpsc;

        let capacity = nictui::activity::DEFAULT_MAX_FREQUENCIES;
        for json in [true, false] {
            let accepted_hits = capacity as u64 + u64::from(!json);
            let dir = Directory::new();
            let csv = dir.path("capacity.csv");
            let (mut master, _slave, mut process) = spawn_with_duration(&csv, json, "30");
            let stdout = process.0.as_mut().unwrap().stdout.take().unwrap();
            let (line_tx, line_rx) = mpsc::channel();
            let reader = thread::spawn(move || {
                let mut reader = io::BufReader::new(stdout);
                let mut output = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        break;
                    }
                    output.push_str(&line);
                    let _ = line_tx.send(line);
                }
                output.into_bytes()
            });
            ping(&mut master);
            master.write_all(&[0x02, 0x2B]).unwrap();
            let accepted_frames =
                (0..capacity).map(|index| hit(14_000_000 + u32::try_from(index).unwrap(), 42));
            if json {
                // Burst one more distinct frequency than the log can retain.
                let burst: Vec<_> = accepted_frames
                    .flatten()
                    .chain(hit(44_650_000, 255))
                    .collect();
                master.write_all(&burst).unwrap();
            } else {
                // Pace live output and include a repeated frequency to distinguish hits from rows.
                for frame in accepted_frames.chain(std::iter::once(hit(14_000_000, 42))) {
                    master.write_all(&frame).unwrap();
                    let line = line_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                    assert!(line.contains("signal (raw, not dBm) 42"), "{line}");
                }
                master.write_all(&hit(44_650_000, 255)).unwrap();
            }
            let mut result = process.output();
            result.stdout = reader.join().unwrap();
            assert_eq!(result.status.code(), Some(1), "{result:?}");
            assert_no_writes(&mut master);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.contains("frequency limit"), "{stderr}");
            assert!(stderr.contains("bounded partial data retained"), "{stderr}");
            let mut csv_reader = csv::Reader::from_path(&csv).unwrap();
            let rows: Vec<_> = csv_reader.records().map(Result::unwrap).collect();
            assert_eq!(rows.len(), capacity);
            let retained_hits: u64 = rows.iter().map(|row| row[1].parse::<u64>().unwrap()).sum();
            assert_eq!(retained_hits, accepted_hits);
            assert!(rows.iter().all(|row| &row[0] != "446.50000"));
            if json {
                let report = report(&result);
                assert_eq!(report["complete"], false);
                assert_eq!(report["hit_count"], retained_hits);
                assert_eq!(report["heartbeat_count"], 1);
                let frequencies = report["frequencies"].as_array().unwrap();
                assert_eq!(frequencies.len(), capacity);
                assert_eq!(
                    frequencies
                        .iter()
                        .map(|row| row["count"].as_u64().unwrap())
                        .sum::<u64>(),
                    retained_hits
                );
            } else {
                let stdout = String::from_utf8(result.stdout).unwrap();
                assert!(!stdout.contains("446.50000"), "rejected hit was emitted");
                assert_eq!(
                    stdout
                        .lines()
                        .filter(|line| line.contains(" MHz | signal (raw, not dBm) "))
                        .count() as u64,
                    retained_hits
                );
                assert!(
                    stderr.contains(&format!("Activity: {retained_hits} hits, 1 heartbeats")),
                    "{stderr}"
                );
            }
        }
    }

    #[test]
    fn handshake_failure_preserves_existing_output() {
        let dir = Directory::new();
        let csv = dir.path("capture.csv");
        std::fs::write(&csv, "preserve me").unwrap();
        let (mut master, _slave, process) = spawn(&csv, true);
        ping(&mut master);
        let result = process.output();
        assert!(!result.status.success(), "{result:?}");
        assert_no_writes(&mut master);
        assert!(result.stdout.is_empty());
        assert_eq!(std::fs::read_to_string(csv).unwrap(), "preserve me");
    }

    #[test]
    fn transport_failure_after_ready_saves_partial_csv_and_json() {
        let dir = Directory::new();
        let csv = dir.path("capture.csv");
        let (mut master, _slave, process) = spawn(&csv, true);
        ping(&mut master);
        master.write_all(&[0x02]).unwrap();
        master.write_all(&hit(14_535_000, 42)).unwrap();
        thread::sleep(Duration::from_millis(100));
        assert_no_writes(&mut master);
        drop(master);
        let result = process.output();
        assert!(!result.status.success(), "{result:?}");
        assert_eq!(report(&result)["complete"], false);
        assert_eq!(report(&result)["hit_count"], 1);
        assert!(
            std::fs::read_to_string(csv)
                .unwrap()
                .contains("145.35000,1,042.00")
        );
    }

    #[test]
    fn cancellation_before_ready_is_interrupted_and_preserves_existing_output() {
        let dir = Directory::new();
        let csv = dir.path("capture.csv");
        std::fs::write(&csv, "preserve me").unwrap();
        let (mut master, _slave, mut process) = spawn(&csv, true);
        ping(&mut master);
        process.interrupt();
        let result = process.output();
        assert!(!result.status.success(), "{result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("Interrupted"),
            "{result:?}"
        );
        assert!(result.stdout.is_empty());
        assert_eq!(std::fs::read_to_string(csv).unwrap(), "preserve me");
        assert_no_writes(&mut master);
    }

    #[test]
    fn cancellation_after_ready_retains_captured_csv_and_partial_json() {
        let dir = Directory::new();
        let csv = dir.path("capture.csv");
        let (mut master, _slave, mut process) = spawn(&csv, true);
        ping(&mut master);
        master.write_all(&[0x02]).unwrap();
        master.write_all(&hit(14_535_000, 42)).unwrap();
        thread::sleep(Duration::from_millis(75));
        process.interrupt();
        let result = process.output();
        assert!(!result.status.success(), "{result:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("Interrupted"));
        assert_eq!(report(&result)["complete"], false);
        assert_eq!(report(&result)["hit_count"], 1);
        assert!(
            std::fs::read_to_string(csv)
                .unwrap()
                .contains("145.35000,1,042.00")
        );
        assert_no_writes(&mut master);
    }

    #[test]
    fn closed_stdout_stops_cleanly_and_saves_partial_capture() {
        let dir = Directory::new();
        let csv = dir.path("capture.csv");
        let (mut master, _slave, mut process) = spawn(&csv, false);
        drop(process.0.as_mut().unwrap().stdout.take());
        ping(&mut master);
        master.write_all(&[0x02]).unwrap();
        master.write_all(&hit(14_535_000, 42)).unwrap();
        let result = process.output();
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        assert!(!String::from_utf8_lossy(&result.stderr).contains("panicked"));
        assert!(
            std::fs::read_to_string(csv)
                .unwrap()
                .contains("145.35000,1,042.00")
        );
        assert_no_writes(&mut master);
    }

    #[test]
    fn unread_final_json_has_bounded_drain_and_retains_csv() {
        let dir = Directory::new();
        let input = dir.path("many.bin");
        let csv = dir.path("capture.csv");
        let bytes: Vec<_> = (0..4096).flat_map(|frequency| hit(frequency, 42)).collect();
        std::fs::write(&input, bytes).unwrap();
        let child = {
            let _guard = PROCESS_SPAWN_LOCK.lock().unwrap();
            nictui(&["activity", "replay", "--json"])
                .arg("--input")
                .arg(&input)
                .arg("--output")
                .arg(&csv)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        };
        // Process::output deliberately waits for exit before reading either pipe.
        let started = Instant::now();
        let result = Process(Some(child)).output();
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("output backpressure"),
            "{result:?}"
        );
        let mut reader = csv::Reader::from_path(csv).unwrap();
        assert_eq!(reader.records().map(Result::unwrap).count(), 4096);
    }

    #[test]
    fn live_text_arrives_before_capture_finishes_without_heartbeat_spam() {
        let dir = Directory::new();
        let csv = dir.path("capture.csv");
        let (mut master, _slave, mut process) = spawn(&csv, false);
        ping(&mut master);
        master.write_all(&[0x02, 0x2B, 0x2B]).unwrap();
        master.write_all(&hit(14_535_000, 42)).unwrap();
        let stdout = process.0.as_mut().unwrap().stdout.take().unwrap();
        let reader = thread::spawn(move || {
            use std::io::BufRead;
            let mut line = String::new();
            let mut reader = io::BufReader::new(stdout);
            reader.read_line(&mut line).unwrap();
            (line, reader)
        });
        let start = Instant::now();
        while !reader.is_finished() {
            assert!(
                start.elapsed() < Duration::from_millis(800),
                "live hit was not flushed"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let (line, mut reader) = reader.join().unwrap();
        assert!(line.contains("145.35000 MHz"), "{line}");
        assert!(line.contains("raw, not dBm) 42"), "{line}");
        chrono::DateTime::parse_from_rfc3339(line.split(" | ").next().unwrap()).unwrap();
        assert!(process.0.as_mut().unwrap().try_wait().unwrap().is_none());
        let result = process.output();
        assert!(result.status.success(), "{result:?}");
        let mut remaining = String::new();
        reader.read_to_string(&mut remaining).unwrap();
        assert_eq!(
            remaining.lines().count(),
            1,
            "unexpected stdout spam: {remaining}"
        );
        assert!(remaining.contains("count 1"), "{remaining}");
        assert!(
            remaining.contains("mean signal (raw, not dBm) 42.00"),
            "{remaining}"
        );
        assert!(!remaining.contains("heartbeat"), "{remaining}");
        assert_no_writes(&mut master);
    }
}
