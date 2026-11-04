#![cfg(unix)]

use std::ffi::{c_int, c_short};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serialport::{SerialPort, TTYPort};

const POLL: Duration = Duration::from_millis(20);
const OUTPUT_LIMIT: usize = 64 * 1024;
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nictui-activity-safety-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type Captured = JoinHandle<io::Result<(Vec<u8>, usize)>>;

fn capture(mut pipe: impl Read + Send + 'static) -> Captured {
    thread::spawn(move || {
        let mut saved = Vec::new();
        let mut total = 0usize;
        let mut buffer = [0; 4096];
        loop {
            let count = match pipe.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                return Ok((saved, total));
            }
            total = total.saturating_add(count);
            let keep = count.min(OUTPUT_LIMIT - saved.len());
            saved.extend_from_slice(&buffer[..keep]);
            // Continue draining after the cap so a noisy regression cannot block shutdown.
        }
    })
}

struct Process {
    child: Option<Child>,
    stdout: Option<Captured>,
    unread_stdout: Option<(io::PipeReader, io::PipeWriter)>,
    stderr: Option<Captured>,
}

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
type PollCount = usize;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
type PollCount = u32;

unsafe extern "C" {
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn poll(fds: *mut PollFd, count: PollCount, timeout: c_int) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
}

impl Process {
    fn spawn(directory: &Directory, port: &str, output: &Path, json: bool) -> Self {
        Self::spawn_with_stdout(directory, port, output, json, false)
    }

    fn spawn_with_stdout(
        directory: &Directory,
        port: &str,
        output: &Path,
        json: bool,
        unread: bool,
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
        command
            .current_dir(&directory.0)
            .args([
                "activity",
                "monitor",
                "--port",
                port,
                "--duration",
                "30",
                "--output",
            ])
            .arg(output)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if json {
            command.arg("--json");
        }
        let unread_stdout = unread.then(|| {
            let (reader, writer) = io::pipe().unwrap();
            command.stdout(writer.try_clone().unwrap());
            (reader, writer)
        });
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().map(capture);
        let stderr = capture(child.stderr.take().unwrap());
        Self {
            child: Some(child),
            stdout,
            unread_stdout,
            stderr: Some(stderr),
        }
    }

    fn child(&mut self) -> &mut Child {
        self.child.as_mut().unwrap()
    }

    fn signal(&mut self, signal: c_int) {
        assert!(self.signal_if_running(signal), "child exited before signal");
    }

    fn signal_if_running(&mut self, signal: c_int) -> bool {
        if self.child().try_wait().unwrap().is_some() {
            return false;
        }
        assert!([2, 15].contains(&signal));
        let pid = c_int::try_from(self.child().id()).unwrap();
        assert!(
            pid > 0,
            "only signal this test's positive, unreaped child PID"
        );
        // SAFETY: the positive PID belongs to our unreaped child; no process group is targeted.
        let result = unsafe { kill(pid, signal) };
        if result != 0 && self.child().try_wait().unwrap().is_some() {
            return false;
        }
        assert_eq!(result, 0, "{}", io::Error::last_os_error());
        true
    }

    fn stdout_backpressured(&self) -> bool {
        let (_, writer) = self.unread_stdout.as_ref().unwrap();
        let mut fd = PollFd {
            fd: writer.as_raw_fd(),
            events: 0x0004, // POLLOUT
            revents: 0,
        };
        // SAFETY: poll receives one initialized entry for our live pipe, with no wait.
        let ready = unsafe { poll(&mut fd, 1, 0) };
        if ready == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            return false;
        }
        assert!(
            ready >= 0,
            "pipe poll failed: {}",
            io::Error::last_os_error()
        );
        assert_eq!(fd.revents & !0x0004, 0, "unexpected pipe events");
        ready == 0
    }

    fn output(mut self) -> Output {
        let status = self.child.take().unwrap().wait().unwrap();
        let (stdout, stdout_count) = if let Some((reader, writer)) = self.unread_stdout.take() {
            // Read only after exit. Keep at most the normal diagnostic cap even under flood.
            drop(writer);
            let mut saved = Vec::new();
            reader
                .take(OUTPUT_LIMIT as u64)
                .read_to_end(&mut saved)
                .unwrap();
            let count = saved.len();
            (saved, count)
        } else {
            self.stdout.take().unwrap().join().unwrap().unwrap()
        };
        let (stderr, stderr_count) = self.stderr.take().unwrap().join().unwrap().unwrap();
        assert!(
            stdout_count <= OUTPUT_LIMIT,
            "activity flooded stdout: {stdout_count} bytes"
        );
        assert!(
            stderr_count <= OUTPUT_LIMIT,
            "activity flooded stderr: {stderr_count} bytes"
        );
        Output {
            status,
            stdout,
            stderr,
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        for reader in [&mut self.stdout, &mut self.stderr] {
            if let Some(reader) = reader.take() {
                let _ = reader.join();
            }
        }
    }
}

fn receive_ping(master: &mut TTYPort, process: &mut Process) {
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "activity startup stalled"
        );
        let mut byte = [0];
        match master.read(&mut byte) {
            Ok(1) => {
                assert_eq!(byte, [0x02], "monitor sent a non-activity command");
                return;
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            result => panic!("PTY ping read failed: {result:?}"),
        }
        assert!(
            process.child().try_wait().unwrap().is_none(),
            "child exited before ping"
        );
    }
}

fn assert_wire_quiet(master: &mut TTYPort) {
    let mut bytes = [0; 64];
    match master.read(&mut bytes) {
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
        result => panic!(
            "monitor must send exactly one 02, including shutdown; extra wire data: {result:?}; {bytes:02X?}"
        ),
    }
}

fn wait_for_exit(master: &mut TTYPort, process: &mut Process, budget: Duration) {
    let started = Instant::now();
    while process.child().try_wait().unwrap().is_none() {
        assert!(
            started.elapsed() < budget,
            "activity child did not exit within {budget:?}"
        );
        assert_wire_quiet(master);
    }
    // Keep the test-owned slave open until this final drain, including bytes sent just before exit.
    assert_wire_quiet(master);
}

fn hit(frequency_10hz: u32, signal: u8) -> Vec<u8> {
    let mut frame = vec![0x2A];
    frame.extend_from_slice(&frequency_10hz.to_le_bytes());
    frame.push(signal);
    frame
}

fn assert_csv(path: &Path) {
    let mut reader = csv::Reader::from_path(path).unwrap();
    assert_eq!(
        reader.headers().unwrap(),
        &csv::StringRecord::from(vec!["Frequency", "Count", "Signal"])
    );
    let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
    assert_eq!(rows.len(), 2, "partial frames must not become CSV hits");
    for (row, frequency, count, mean) in [
        (&rows[0], "145.35000", 2u64, 42.5),
        (&rows[1], "433.50000", 1u64, 2.0),
    ] {
        assert_eq!(&row[0], frequency);
        assert_eq!(row[1].parse::<u64>().unwrap(), count);
        assert_eq!(row[2].parse::<f64>().unwrap(), mean);
    }
}

fn assert_port_released(slave: TTYPort) {
    let name = slave.name().unwrap();
    drop(slave);
    // macOS PTYs reject the custom-baud ioctl used for 38400 by serialport.
    #[cfg(target_os = "macos")]
    let baud = 0;
    #[cfg(not(target_os = "macos"))]
    let baud = 38_400;
    let reopened = serialport::new(name, baud)
        .timeout(POLL)
        .open_native()
        .expect("activity must release its private port on exit");
    drop(reopened);
}

struct Feeder {
    stop: Arc<AtomicBool>,
    sent: Arc<AtomicU64>,
    handle: Option<JoinHandle<io::Result<()>>>,
}

impl Feeder {
    fn start(master: &TTYPort) -> Self {
        let mut writer = master.try_clone_native().unwrap();
        writer.set_timeout(POLL).unwrap();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let nonblock = 0x800;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let nonblock = 0x4;
        // SAFETY: F_GETFL/F_SETFL operate only on this test's owned fake-radio FD.
        let flags = unsafe { fcntl(writer.as_raw_fd(), 3) };
        assert!(flags >= 0, "{}", io::Error::last_os_error());
        // Poll readiness alone does not bound a blocking PTY write on macOS.
        assert_eq!(
            unsafe { fcntl(writer.as_raw_fd(), 4, flags | nonblock) },
            0,
            "{}",
            io::Error::last_os_error()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicU64::new(0));
        let worker_stop = Arc::clone(&stop);
        let worker_sent = Arc::clone(&sent);
        let handle = thread::spawn(move || {
            let frame = hit(14_535_000, 42);
            let mut offset = 0;
            let started = Instant::now();
            while !worker_stop.load(Ordering::SeqCst)
                && started.elapsed() < Duration::from_secs(15)
                && worker_sent.load(Ordering::SeqCst) < 65_536
            {
                // TTYPort::write polls with POLL timeout; never write_all or tcdrain a flood.
                match writer.write(&frame[offset..]) {
                    Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                    Ok(count) => {
                        offset += count;
                        if offset == frame.len() {
                            worker_sent.fetch_add(1, Ordering::SeqCst);
                            offset = 0;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::TimedOut
                                | io::ErrorKind::WouldBlock
                                | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        });
        Self {
            stop,
            sent,
            handle: Some(handle),
        }
    }

    fn finish(&mut self) -> u64 {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.take().unwrap().join().unwrap().unwrap();
        self.sent.load(Ordering::SeqCst)
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn unread_stdout_flood(signal: c_int) {
    let directory = Directory::new();
    let path = directory.0.join("backpressure.csv");
    let (mut master, slave) = TTYPort::pair().unwrap();
    master.set_timeout(POLL).unwrap();
    let mut process =
        Process::spawn_with_stdout(&directory, &slave.name().unwrap(), &path, false, true);
    receive_ping(&mut master, &mut process);
    master.write_all(&[0x02, 0x2B]).unwrap();
    let mut feeder = Feeder::start(&master);
    let started = Instant::now();
    let mut backpressured = false;
    loop {
        if process.child().try_wait().unwrap().is_some() {
            break;
        }
        if process.stdout_backpressured() {
            backpressured = true;
            // Let a currently executing write encounter the full pipe before signalling.
            let settle = Instant::now();
            while settle.elapsed() < Duration::from_millis(150) {
                assert_wire_quiet(&mut master);
            }
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "stdout never filled and capture did not fail safely"
        );
        assert_wire_quiet(&mut master);
    }
    let signalled = process.signal_if_running(signal);
    wait_for_exit(&mut master, &mut process, Duration::from_secs(3));
    let sent = feeder.finish();
    let output = process.output();
    assert_eq!(
        output.status.signal(),
        None,
        "backpressure caused signal death"
    );
    assert!(
        !output.status.success(),
        "backpressure or interruption reported success"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lower = stderr.to_ascii_lowercase();
    let safe_output_failure = lower.contains("backpressure")
        || ((lower.contains("output") || lower.contains("stdout"))
            && (lower.contains("full")
                || lower.contains("blocked")
                || lower.contains("timed out")));
    assert!(
        (signalled && stderr.contains("Interrupted")) || safe_output_failure,
        "neither cooperative interruption nor explicit backpressure failure: {stderr}"
    );
    assert!(
        backpressured || safe_output_failure,
        "test never exercised output pressure: {stderr}"
    );
    assert!(
        !output.stdout.is_empty(),
        "no live output preceded backpressure"
    );
    let mut csv = csv::Reader::from_path(&path).expect("backpressure must retain partial CSV");
    assert_eq!(
        csv.headers().unwrap(),
        &csv::StringRecord::from(vec!["Frequency", "Count", "Signal"])
    );
    let rows: Vec<_> = csv.records().map(Result::unwrap).collect();
    assert_eq!(
        rows.len(),
        1,
        "bounded one-frequency flood produced unexpected rows"
    );
    assert_eq!(&rows[0][0], "145.35000");
    let retained = rows[0][1].parse::<u64>().unwrap();
    assert!(
        retained > 0 && retained <= sent,
        "invalid retained count {retained}, sent {sent}"
    );
    let emitted_hits = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(" | 145.35000 MHz | ") && line.ends_with("raw, not dBm) 42"))
        .count() as u64;
    assert!(
        emitted_hits > 0,
        "unread stdout contained no complete live hits"
    );
    assert!(
        retained >= emitted_hits,
        "CSV lost emitted hits: retained {retained}, emitted {emitted_hits}"
    );
    assert_eq!(rows[0][2].parse::<f64>().unwrap(), 42.0);
    eprintln!(
        "signal={signal}, sent_signal={signalled}, pipe_full={backpressured}, sent_hits={sent}, retained_hits={retained}, emitted_hits={emitted_hits}"
    );
    assert_port_released(slave);
}

#[test]
fn int_with_unread_stdout_retains_csv_and_only_sends_activity_ping() {
    unread_stdout_flood(2);
}

#[test]
fn term_with_unread_stdout_retains_csv_and_only_sends_activity_ping() {
    unread_stdout_flood(15);
}

#[test]
fn signals_retain_fragmented_activity_without_sending_stop_commands() {
    for signal in [2, 15] {
        for json in [false, true] {
            let directory = Directory::new();
            let path = directory.0.join("partial.csv");
            let (mut master, slave) = TTYPort::pair().unwrap();
            master.set_timeout(POLL).unwrap();
            let mut process = Process::spawn(&directory, &slave.name().unwrap(), &path, json);
            receive_ping(&mut master, &mut process);
            master.write_all(&[0x02, 0x2B]).unwrap();
            for frame in [
                hit(14_535_000, 0x2A),
                hit(14_535_000, 0x2B),
                hit(43_350_000, 2),
            ] {
                for fragment in frame.chunks(2) {
                    master.write_all(fragment).unwrap();
                    thread::sleep(Duration::from_millis(10));
                }
            }
            // An unfinished hit must not lose earlier hits or count its embedded heartbeat byte.
            master.write_all(&[0x2B, 0x2A, 0x2B, 0x02]).unwrap();
            let settle = Instant::now();
            while settle.elapsed() < Duration::from_millis(250) {
                assert_wire_quiet(&mut master);
            }
            process.signal(signal);
            wait_for_exit(&mut master, &mut process, Duration::from_secs(3));
            let output = process.output();
            assert_eq!(output.status.signal(), None, "child died from a signal");
            assert!(
                !output.status.success(),
                "interruption must not report success"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("Interrupted"),
                "missing cooperative interruption: {stderr}"
            );
            assert_csv(&path);
            if json {
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["complete"], false);
                assert_eq!(report["hit_count"], 3);
                assert_eq!(report["heartbeat_count"], 2);
                assert_eq!(report["truncated_frames"], 1);
                let rows = report["frequencies"].as_array().unwrap();
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0]["frequency_10hz"], 14_535_000);
                assert_eq!(rows[0]["count"], 2);
                assert_eq!(rows[0]["mean_signal_raw"], 42.5);
                assert_eq!(rows[1]["frequency_10hz"], 43_350_000);
                assert_eq!(rows[1]["count"], 1);
                assert_eq!(rows[1]["mean_signal_raw"], 2.0);
            } else {
                assert!(stderr.contains("partial capture"), "{stderr}");
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(stdout.contains("145.35000"), "{stdout}");
            }
            assert_port_released(slave);
        }
    }
}

#[test]
fn failed_handshake_preserves_existing_csv_and_releases_private_port() {
    let directory = Directory::new();
    let path = directory.0.join("existing.csv");
    let sentinel = b"existing recording must survive a failed connection\n";
    std::fs::write(&path, sentinel).unwrap();
    let (mut master, slave) = TTYPort::pair().unwrap();
    master.set_timeout(POLL).unwrap();
    let mut process = Process::spawn(&directory, &slave.name().unwrap(), &path, true);
    receive_ping(&mut master, &mut process);
    // Telemetry, including 02 inside a hit payload, is not a successful handshake.
    master.write_all(&hit(14_535_000, 0x02)).unwrap();
    master.write_all(&[0x2B]).unwrap();
    wait_for_exit(&mut master, &mut process, Duration::from_secs(3));
    let output = process.output();
    assert_eq!(
        output.status.signal(),
        None,
        "handshake failure caused signal death"
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Timed out waiting for activity ping response 02"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("Interrupted"),
        "handshake failure was mislabeled: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "failed connection emitted a capture report"
    );
    assert_eq!(std::fs::read(&path).unwrap(), sentinel);
    assert_eq!(
        std::fs::read_dir(&directory.0).unwrap().count(),
        1,
        "failed connection left extra files"
    );
    assert_port_released(slave);
}
