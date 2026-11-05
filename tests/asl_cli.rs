use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use nictui::cli::{AslCommand, Cli, Commands};
use serde_json::Value;

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nictui-asl-cli-{}-{}-{}",
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

    fn input(&self, bytes: &[u8]) -> PathBuf {
        let path = self.0.join("raw.bin");
        std::fs::write(&path, bytes).unwrap();
        path
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

fn bounded_output(mut command: Command, close_stdout: bool) -> Output {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if close_stdout {
        drop(child.stdout.take());
    }
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("ASL command did not stop within its bounded deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn replay(input: &Path, json: bool) -> Command {
    let mut command = nictui(&["asl", "replay"]);
    command.arg("--input").arg(input);
    if json {
        command.arg("--json");
    }
    command
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("not one final JSON object: {error}; {output:?}"))
}

#[test]
fn monitor_requires_explicit_native_port_and_defaults_to_thirty_seconds() {
    let cli =
        Cli::try_parse_from(["nictui", "asl", "monitor", "--port", "COM3", "--json"]).unwrap();
    let Some(Commands::Asl {
        command: AslCommand::Monitor(args),
    }) = cli.command
    else {
        panic!("wrong command");
    };
    assert_eq!(args.port, "COM3");
    assert_eq!(args.duration, 30);
    assert!(args.json);
    assert!(Cli::try_parse_from(["nictui", "asl", "monitor"]).is_err());
    for port in [
        "",
        " ",
        " COM3",
        "ble:device",
        "ble://device",
        "BLE://device",
        "serial://COM3",
    ] {
        assert!(Cli::try_parse_from(["nictui", "asl", "monitor", "--port", port]).is_err());
    }
    for duration in ["0", "-1", "0.5", "nope"] {
        assert!(
            Cli::try_parse_from([
                "nictui",
                "asl",
                "monitor",
                "--port",
                "COM3",
                "--duration",
                duration,
            ])
            .is_err()
        );
    }
    assert!(
        Cli::try_parse_from([
            "nictui",
            "asl",
            "monitor",
            "--port",
            "COM3",
            "--duration",
            "1",
        ])
        .is_ok()
    );
    for option in [
        "--ble-name",
        "--ble-device",
        "--ble-scan-time",
        "--baud",
        "--output",
        "--send",
    ] {
        assert!(
            Cli::try_parse_from([
                "nictui", "asl", "monitor", "--port", "COM3", option, "example",
            ])
            .is_err()
        );
    }
}

#[test]
fn replay_is_offline_and_asl_has_no_control_commands() {
    assert!(Cli::try_parse_from(["nictui", "asl", "replay"]).is_err());
    for option in [
        "--port",
        "--ble-name",
        "--ble-device",
        "--duration",
        "--output",
        "--send",
    ] {
        assert!(
            Cli::try_parse_from([
                "nictui", "asl", "replay", "--input", "raw.bin", option, "example",
            ])
            .is_err()
        );
    }
    for verb in [
        "ptt", "ptt-b", "key", "enable", "write", "audio", "transmit",
    ] {
        assert!(Cli::try_parse_from(["nictui", "asl", verb]).is_err());
    }
}

#[test]
fn help_explains_manual_setup_and_evidence_limits() {
    for args in [
        vec!["asl", "--help"],
        vec!["asl", "monitor", "--help"],
        vec!["asl", "replay", "--help"],
    ] {
        let output = bounded_output(nictui(&args), false);
        assert!(output.status.success(), "{output:?}");
        let help = String::from_utf8(output.stdout).unwrap();
        for text in [
            "Menu 45 ASL USB",
            "Stop Activity, Remote",
            "38400",
            "zero UART command bytes",
            "no PTT commands",
            "DTR/RTS",
            "no handshake",
            "not voice",
            "silent stream remains Unknown",
            "no resynchronization",
            "host replay processing times",
        ] {
            assert!(help.contains(text), "missing {text}: {help}");
        }
        assert!(!help.contains("auto-select"), "{help}");
        assert!(!help.contains("--ble-name"), "{help}");
    }
}

#[test]
fn replay_counts_duplicates_but_only_state_changes_are_transitions() {
    let dir = Directory::new();
    let input = dir.input(&[0, 0x10, 0x10, 0, 0x11, 0x11, 0x10]);
    let output = bounded_output(replay(&input, true), false);
    assert!(output.status.success(), "{output:?}");
    let json = report(&output);
    assert_eq!(json["state"], "closed");
    assert_eq!(json["open_reports"], 2);
    assert_eq!(json["closed_reports"], 3);
    assert_eq!(json["transitions"], 2);
    assert_eq!(json["stop_reason"], "end_of_input");
    assert_eq!(json["source"], "replay");
    assert_eq!(json["timestamp_source"], "host_replay");
    assert_eq!(json["complete"], true);
    assert!(json["error"].is_null());
    assert!(json["port_opened_at"].is_null());
    assert!(json["elapsed_seconds"].as_f64().unwrap() >= 0.0);
    for key in ["last_report_at", "last_transition_at"] {
        chrono::DateTime::parse_from_rfc3339(json[key].as_str().unwrap()).unwrap();
    }
    assert!(
        output.stderr.is_empty(),
        "offline replay must not discover/probe/setup a radio"
    );
}

#[test]
fn empty_or_nop_only_replay_remains_unknown_not_healthy() {
    let dir = Directory::new();
    for bytes in [vec![], vec![0; 9000]] {
        let input = dir.input(&bytes);
        let output = bounded_output(replay(&input, true), false);
        assert!(output.status.success(), "{output:?}");
        let json = report(&output);
        assert_eq!(json["state"], "unknown");
        assert_eq!(json["open_reports"], 0);
        assert_eq!(json["closed_reports"], 0);
        assert_eq!(json["transitions"], 0);
        assert!(json["last_report_at"].is_null());
        assert!(json["last_transition_at"].is_null());
        assert!(json.get("healthy").is_none());
    }
}

#[test]
fn first_known_report_is_not_a_transition_and_text_suppresses_duplicates() {
    let dir = Directory::new();
    let input = dir.input(&[0x11, 0x11, 0, 0x11]);
    let output = bounded_output(replay(&input, true), false);
    let json = report(&output);
    assert_eq!(json["state"], "open");
    assert_eq!(json["open_reports"], 3);
    assert_eq!(json["transitions"], 0);
    assert!(json["last_transition_at"].is_null());
    let input = dir.input(&[0x10, 0x10, 0, 0x11, 0x11, 0x10]);
    let output = bounded_output(replay(&input, false), false);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let states: Vec<_> = text
        .lines()
        .filter_map(|line| line.split_once(" | COS ").map(|(_, state)| state))
        .collect();
    assert_eq!(states, ["Unknown", "Closed", "Open", "Closed"]);
    assert!(text.contains("3 closed reports"), "{text}");
}

#[test]
fn unexpected_byte_is_terminal_with_partial_counts_unknown_state_and_nonzero_exit() {
    let dir = Directory::new();
    for byte in [0x01, 0x02, 0x12, 0x2a, 0xff] {
        let input = dir.input(&[0, 0x10, 0x11, byte, 0x10, 0x11]);
        let output = bounded_output(replay(&input, true), false);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let json = report(&output);
        assert_eq!(json["state"], "unknown");
        assert_eq!(json["open_reports"], 1);
        assert_eq!(json["closed_reports"], 1);
        assert_eq!(json["transitions"], 1);
        assert_eq!(json["complete"], false);
        assert_eq!(json["stop_reason"], "fault");
        assert!(!json["error"].as_str().unwrap().is_empty());
    }
}

#[test]
fn split_chunks_and_read_failure_have_final_json() {
    let dir = Directory::new();
    let mut bytes = vec![0; 8191];
    bytes.extend([0x10, 0x11, 0xff, 0x10]);
    let input = dir.input(&bytes);
    let output = bounded_output(replay(&input, true), false);
    assert!(!output.status.success());
    assert_eq!(report(&output)["transitions"], 1);
    let output = bounded_output(replay(&dir.0.join("missing.bin"), true), false);
    assert!(!output.status.success());
    let json = report(&output);
    assert_eq!(json["state"], "unknown");
    assert_eq!(json["stop_reason"], "fault");
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("Failed to open ASL recording")
    );
}

#[test]
fn unread_or_broken_output_stops_within_a_bounded_deadline() {
    let dir = Directory::new();
    let input = dir.input(&[0x10, 0x11].repeat(100_000));
    for close_stdout in [false, true] {
        let output = bounded_output(replay(&input, false), close_stdout);
        assert!(!output.status.success(), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
    let input = dir.input(&[0x11]);
    let output = bounded_output(replay(&input, true), true);
    assert!(!output.status.success(), "{output:?}");
}
