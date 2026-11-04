use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use nictui::activity::{
    ActivityDecoder, ActivityEvent, ActivityLog, format_frequency_mhz, write_activity_csv,
};
use nictui::activity_session::{ActivityMonitorEvent, ActivityMonitorOptions, monitor_activity};
use nictui::ble::parse_ble_device_uri;
use nictui::cancellation;
use nictui::device::{ProgressEvent, inspect_codeplug, read_codeplug_with_timing};
use nictui::protocol::radio::ReadTiming;
use nictui::protocol::{TOTAL_BLOCKS, save_codeplug};

#[derive(Debug, Parser)]
#[command(
    name = "radio_lab",
    about = "Experimental radio read and activity tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read all 256 EEPROM blocks and report decoded record counts.
    Read(ReadArgs),
    /// Collect passive activity without disabling radio operation.
    Activity(ActivityArgs),
    /// Decode a raw activity recording without connecting to hardware.
    Replay(ReplayArgs),
}

#[derive(Debug, Args)]
struct ReadArgs {
    /// Serial port path or ble:// device identifier.
    #[arg(long, value_parser = nonempty_port)]
    port: String,
    /// Use response-paced serial reads; temporarily pauses radio operation.
    #[arg(long)]
    fast: bool,
    /// Save the complete EEPROM snapshot as an .nfw file.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ActivityArgs {
    /// Serial port path or ble:// device identifier.
    #[arg(long, value_parser = nonempty_port)]
    port: String,
    /// Capture duration in seconds.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
    duration: u64,
    /// Write per-frequency hit counts and mean raw signal values to CSV.
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct ReplayArgs {
    /// Raw binary activity recording.
    #[arg(long)]
    input: PathBuf,
    /// Write per-frequency hit counts and mean raw signal values to CSV.
    #[arg(long)]
    output: PathBuf,
}

fn nonempty_port(value: &str) -> std::result::Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value == "ble://" {
        return Err("provide a serial port path or ble:// device identifier".to_string());
    }
    Ok(value.to_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    cancellation::install_handler()?;
    let cli = Cli::parse();
    check_interruption()?;
    match cli.command {
        Command::Read(args) => run_read(args),
        Command::Activity(args) => run_activity(args),
        Command::Replay(args) => run_replay(args),
    }?;
    check_interruption()
}

fn check_interruption() -> Result<()> {
    if cancellation::is_requested() {
        bail!("Interrupted: signal requested");
    }
    Ok(())
}

fn read_timing(fast: bool) -> ReadTiming {
    if fast {
        ReadTiming::Responsive
    } else {
        ReadTiming::Conservative
    }
}

fn run_read(args: ReadArgs) -> Result<()> {
    if args.fast && parse_ble_device_uri(&args.port).is_some() {
        bail!("--fast requires USB serial; omit --fast for BLE reads");
    }
    let timing = read_timing(args.fast);
    eprintln!("Reading {TOTAL_BLOCKS} blocks with {timing:?} timing");
    let started = Instant::now();
    let mut last_reported = 0;
    let (snapshot, _) = read_codeplug_with_timing(&args.port, timing, |event| match event {
        ProgressEvent::Status(message) => eprintln!("{message}"),
        ProgressEvent::Progress(fraction) => {
            let blocks = (fraction * TOTAL_BLOCKS as f64).round() as usize;
            if blocks >= last_reported + 32 || blocks == TOTAL_BLOCKS {
                eprintln!(
                    "Read {blocks}/{TOTAL_BLOCKS} blocks ({:.2}s elapsed)",
                    started.elapsed().as_secs_f64()
                );
                last_reported = blocks;
            }
        }
    })?;
    let elapsed = started.elapsed();
    let inspection = inspect_codeplug(&snapshot).context("Could not decode radio snapshot")?;
    println!(
        "Read {TOTAL_BLOCKS} blocks ({} bytes) in {:.3}s ({:.2} ms/block, end-to-end)",
        snapshot.len(),
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / TOTAL_BLOCKS as f64
    );
    println!(
        "Endian: {:?}; channel endian: {:?}",
        inspection.endian, inspection.channel_endian
    );
    println!(
        "Decoded: {} channels, {} VFO memories, {} settings block, {} scan presets, {} band plans, {} DTMF presets, {} named groups",
        inspection.channel_count,
        inspection.vfo_memory_count,
        usize::from(inspection.settings_present),
        inspection.scan_preset_count,
        inspection.band_plan_count,
        inspection.dtmf_preset_count,
        inspection
            .group_labels
            .iter()
            .filter(|label| !label.trim().is_empty())
            .count()
    );
    if let Some(path) = args.output {
        check_interruption()?;
        save_codeplug(&path, &snapshot)
            .with_context(|| format!("Could not save snapshot to {}", path.display()))?;
        println!("Saved snapshot to {}", path.display());
    }
    Ok(())
}

#[derive(Debug, Default)]
struct ActivityCapture {
    log: ActivityLog,
    hits: u64,
    heartbeats: u64,
}

impl ActivityCapture {
    fn record(&mut self, event: ActivityEvent) -> Result<()> {
        self.log.record(&event)?;
        match event {
            ActivityEvent::Hit { .. } => self.hits += 1,
            ActivityEvent::Heartbeat { .. } => self.heartbeats += 1,
        }
        Ok(())
    }

    fn record_live(&mut self, event: ActivityEvent) -> Result<()> {
        if let ActivityEvent::Hit {
            timestamp,
            frequency_10hz,
            signal_raw,
        } = &event
        {
            println!(
                "{} {} MHz signal_raw={signal_raw}",
                timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
                format_frequency_mhz(*frequency_10hz)
            );
        }
        self.record(event)
    }

    fn save(&self, path: &std::path::Path) -> Result<()> {
        write_activity_csv(path, &self.log)?;
        println!(
            "Captured {} hits across {} frequencies and {} heartbeats; saved {}",
            self.hits,
            self.log.rows().count(),
            self.heartbeats,
            path.display()
        );
        println!("Signal values are raw radio values, not dBm.");
        Ok(())
    }
}

fn run_activity(args: ActivityArgs) -> Result<()> {
    let duration = Duration::from_secs(args.duration);
    run_activity_for(args, duration)
}

fn run_activity_for(args: ActivityArgs, duration: Duration) -> Result<()> {
    check_interruption()?;
    eprintln!("Start VFO preset scan and enable Activity Mode on radio");
    let mut capture = ActivityCapture::default();
    let stop = Arc::new(AtomicBool::new(false));
    let options = ActivityMonitorOptions {
        duration: Some(duration),
        stop_signal: Some(stop.clone()),
    };
    let mut ready = false;
    let mut record_error = None;
    let capture_result = monitor_activity(&args.port, &options, |event| match event {
        ActivityMonitorEvent::Ready => {
            ready = true;
            eprintln!(
                "Receiving activity for {:.3} seconds",
                duration.as_secs_f64()
            );
        }
        ActivityMonitorEvent::Event(event) => {
            if let Err(error) = capture.record_live(event) {
                record_error = Some(error);
                stop.store(true, Ordering::SeqCst);
            }
        }
        ActivityMonitorEvent::Warning(message) => eprintln!("warning: {message}"),
    })
    .map(|_| ())
    .and_then(|_| record_error.map_or(Ok(()), Err));
    if !ready {
        return capture_result;
    }
    check_interruption()?;
    let save_result = capture.save(&args.output);
    match (capture_result, save_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(save_error)) => {
            Err(error.context(format!("Could not save collected activity: {save_error:#}")))
        }
    }
}

fn decode_recording(mut reader: impl Read) -> Result<ActivityCapture> {
    let mut decoder = ActivityDecoder::new(Duration::MAX);
    let mut capture = ActivityCapture::default();
    let mut buffer = [0u8; 4096];
    // Raw recordings carry no timestamps; disk read delays must not expire frames.
    let timestamp = chrono::Utc::now();
    loop {
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.context("Could not read activity recording")?,
        };
        if count == 0 {
            break;
        }
        for event in decoder.push_at(&buffer[..count], timestamp) {
            capture.record(event)?;
        }
    }
    decoder
        .finish()
        .context("Activity recording ends with an incomplete frame")?;
    Ok(capture)
}

fn run_replay(args: ReplayArgs) -> Result<()> {
    let file = File::open(&args.input)
        .with_context(|| format!("Could not open recording {}", args.input.display()))?;
    let capture = decode_recording(BufReader::new(file))?;
    capture.save(&args.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let directory = Self(std::env::temp_dir().join(format!(
                "nictui-radio-lab-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            )));
            std::fs::create_dir(&directory.0).unwrap();
            directory
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn read_defaults_to_conservative_full_snapshot_without_output() {
        let cli = Cli::try_parse_from(["radio_lab", "read", "--port", "ble://radio"])
            .expect("valid read command");
        let Command::Read(args) = cli.command else {
            panic!("expected read command");
        };
        assert_eq!(args.port, "ble://radio");
        assert_eq!(read_timing(args.fast), ReadTiming::Conservative);
        assert!(args.output.is_none());
    }

    #[test]
    fn read_accepts_fast_snapshot_output() {
        let cli = Cli::try_parse_from([
            "radio_lab",
            "read",
            "--port",
            "/dev/ttyUSB0",
            "--fast",
            "--output",
            "snapshot.nfw",
        ])
        .expect("valid fast read command");
        let Command::Read(args) = cli.command else {
            panic!("expected read command");
        };
        assert_eq!(read_timing(args.fast), ReadTiming::Responsive);
        assert_eq!(args.output, Some(PathBuf::from("snapshot.nfw")));
    }

    #[test]
    fn read_rejects_missing_or_empty_ports_and_unknown_flags() {
        for args in [
            vec!["radio_lab", "read"],
            vec!["radio_lab", "read", "--port", " "],
            vec!["radio_lab", "read", "--port", "ble://"],
            vec!["radio_lab", "read", "--port", "radio", "--write"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn fast_ble_is_rejected_before_opening_a_device() {
        let error = run_read(ReadArgs {
            port: "ble://radio".to_string(),
            fast: true,
            output: None,
        })
        .expect_err("fast BLE is not supported");
        assert!(error.to_string().contains("--fast requires USB serial"));
    }

    #[test]
    fn activity_accepts_default_and_explicit_duration() {
        for (extra, expected) in [(vec![], 30), (vec!["--duration", "7"], 7)] {
            let mut args = vec![
                "radio_lab",
                "activity",
                "--port",
                "ble://radio",
                "--output",
                "hits.csv",
            ];
            args.extend(extra);
            let Command::Activity(args) = Cli::try_parse_from(args).unwrap().command else {
                panic!("expected activity command");
            };
            assert_eq!(args.duration, expected);
        }
        assert!(
            Cli::try_parse_from([
                "radio_lab",
                "activity",
                "--port",
                "radio",
                "--output",
                "hits.csv",
                "--duration",
                "0",
            ])
            .is_err()
        );
    }

    #[test]
    fn replay_requires_only_input_and_output() {
        let Command::Replay(args) = Cli::try_parse_from([
            "radio_lab",
            "replay",
            "--input",
            "packet.bin",
            "--output",
            "hits.csv",
        ])
        .unwrap()
        .command
        else {
            panic!("expected replay command");
        };
        assert_eq!(args.input, PathBuf::from("packet.bin"));
        assert_eq!(args.output, PathBuf::from("hits.csv"));
    }

    fn hit(frequency: u32, signal: u8) -> Vec<u8> {
        let mut bytes = vec![0x2A];
        bytes.extend_from_slice(&frequency.to_le_bytes());
        bytes.push(signal);
        bytes
    }

    #[test]
    fn replay_aggregates_hits_and_exports_raw_signal_csv() {
        let mut bytes = vec![0x2B];
        bytes.extend(hit(14_550_000, 20));
        bytes.extend(hit(14_550_000, 40));
        bytes.extend(hit(43_350_000, 10));
        let capture = decode_recording(bytes.as_slice()).unwrap();
        assert_eq!(capture.hits, 3);
        assert_eq!(capture.heartbeats, 1);
        assert_eq!(capture.log.rows().count(), 2);
        let mut csv = Vec::new();
        capture.log.write_csv(&mut csv).unwrap();
        let csv = String::from_utf8(csv).unwrap();
        assert!(csv.starts_with("Frequency,Count,Signal\r\n"));
        assert!(csv.contains("145.50000,2,030.00\r\n"));
        assert!(csv.contains("433.50000,1,010.00\r\n"));
    }

    #[test]
    fn replay_rejects_incomplete_final_frame() {
        assert!(decode_recording([0x2A, 0x01].as_slice()).is_err());
    }

    #[test]
    fn replay_handles_split_frames_and_interrupted_reads() {
        struct FragmentedReader<'a> {
            bytes: &'a [u8],
            interrupt: bool,
        }
        impl Read for FragmentedReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.interrupt {
                    self.interrupt = false;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                let count = buffer.len().min(1);
                self.bytes.read(&mut buffer[..count])
            }
        }
        let mut bytes = hit(0x002B_2A02, 0x2B);
        bytes.push(0x2B);
        let capture = decode_recording(FragmentedReader {
            bytes: &bytes,
            interrupt: true,
        })
        .unwrap();
        assert_eq!(capture.hits, 1);
        assert_eq!(capture.heartbeats, 1);
        let row = capture.log.rows().next().unwrap();
        assert_eq!(row.frequency_10hz, 0x002B_2A02);
        assert_eq!(row.mean_signal_raw(), 43.0);
    }

    #[test]
    fn empty_replay_produces_header_only() {
        let capture = decode_recording([].as_slice()).unwrap();
        let mut csv = Vec::new();
        capture.log.write_csv(&mut csv).unwrap();
        assert_eq!(csv, b"Frequency,Count,Signal\r\n");
    }

    #[test]
    fn replay_writes_csv_and_preserves_output_on_truncation() {
        let directory = TestDirectory::new();
        let input = directory.0.join("packet.bin");
        let output = directory.0.join("hits.csv");
        std::fs::write(&input, hit(14_550_000, 42)).unwrap();
        run_replay(ReplayArgs {
            input: input.clone(),
            output: output.clone(),
        })
        .unwrap();
        let csv = std::fs::read(&output).unwrap();
        assert_eq!(csv, b"Frequency,Count,Signal\r\n145.50000,1,042.00\r\n");
        std::fs::write(&input, [0x2A, 0x01]).unwrap();
        let error = run_replay(ReplayArgs {
            input,
            output: output.clone(),
        })
        .expect_err("truncated replay must fail");
        assert!(error.to_string().contains("incomplete frame"));
        assert_eq!(std::fs::read(output).unwrap(), csv);
    }

    #[cfg(unix)]
    #[test]
    fn live_activity_pty_uses_only_ping_and_exports_fragmented_hits() {
        use nictui::activity::ACTIVITY_PING;
        use serialport::SerialPort;
        use std::io::Write;
        use std::sync::mpsc;

        for echo in [true, false] {
            let directory = TestDirectory::new();
            let output = directory.0.join("hits.csv");
            let (mut emulator, slave) = serialport::TTYPort::pair().unwrap();
            let port = slave.name().unwrap();
            // Keep the slave alive until the CLI opens it, avoiding a master-side EOF race.
            emulator.set_timeout(Duration::from_secs(2)).unwrap();
            let (stop, stopped) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let mut command = [0u8; 1];
                emulator.read_exact(&mut command).unwrap();
                let mut commands = command.to_vec();
                let mut stream = Vec::new();
                if echo {
                    stream.push(ACTIVITY_PING);
                }
                stream.push(0x2B);
                stream.extend(hit(14_550_000, if echo { 20 } else { ACTIVITY_PING }));
                if echo {
                    stream.extend(hit(14_550_000, 40));
                }
                for chunk in stream.chunks(1) {
                    emulator.write_all(chunk).unwrap();
                    std::thread::sleep(Duration::from_millis(2));
                }
                emulator.set_timeout(Duration::from_millis(10)).unwrap();
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(2) && stopped.try_recv().is_err() {
                    let mut buffer = [0u8; 32];
                    match emulator.read(&mut buffer) {
                        Ok(count) => commands.extend_from_slice(&buffer[..count]),
                        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(_) => break,
                    }
                }
                commands
            });
            let started = Instant::now();
            let result = run_activity_for(
                ActivityArgs {
                    port,
                    duration: 1,
                    output: output.clone(),
                },
                Duration::from_millis(160),
            );
            let elapsed = started.elapsed();
            let _ = stop.send(());
            let commands = worker.join().unwrap();
            assert_eq!(
                commands,
                [ACTIVITY_PING],
                "no disable, enable, remote or EEPROM commands"
            );
            assert!(
                elapsed < Duration::from_secs(1),
                "activity must remain bounded"
            );
            if echo {
                result.unwrap();
                assert!(elapsed >= Duration::from_millis(160));
                assert_eq!(
                    std::fs::read(output).unwrap(),
                    b"Frequency,Count,Signal\r\n145.50000,2,030.00\r\n"
                );
            } else {
                assert!(result.unwrap_err().to_string().contains("response 02"));
                assert!(!output.exists(), "payload 02 must not acknowledge the ping");
            }
        }
    }
}
