#![cfg(unix)]

use std::io::{self, Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serialport::{SerialPort, TTYPort};

struct Process(Option<Child>);

impl Process {
    fn spawn(port: &str, held_key: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
        command.args([
            "remote",
            "capture",
            "--port",
            port,
            "--duration",
            "60",
            "--raw",
            "--recover-retries",
            "3",
        ]);
        if held_key {
            command.args(["--send", "menu", "--send", "exit"]);
        }
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

    fn assert_running(&mut self) {
        assert!(
            self.child().try_wait().unwrap().is_none(),
            "remote child exited before its stop ACK"
        );
    }

    fn output(mut self) -> Output {
        let started = Instant::now();
        while self.child().try_wait().unwrap().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "remote child did not exit"
            );
            thread::sleep(Duration::from_millis(5));
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            // Failure-only cleanup of this test's private fake-radio child.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Radio {
    master: TTYPort,
    _slave: TTYPort,
    port: String,
    transcript: Vec<u8>,
}

impl Radio {
    fn new() -> Self {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_millis(25)).unwrap();
        Self {
            port: slave.name().unwrap(),
            master,
            _slave: slave,
            transcript: Vec::new(),
        }
    }

    fn expect(&mut self, expected: u8, process: &mut Process) {
        let started = Instant::now();
        loop {
            process.assert_running();
            let mut byte = [0];
            match self.master.read(&mut byte) {
                Ok(1) => {
                    self.transcript.push(byte[0]);
                    assert_eq!(
                        byte[0], expected,
                        "wire transcript: {:02X?}",
                        self.transcript
                    );
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                other => panic!("PTY read failed: {other:?}"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "missing {expected:02X}; transcript: {:02X?}",
                self.transcript
            );
        }
    }

    fn bootstrap(&mut self, process: &mut Process) {
        for _ in 0..10 {
            self.expect(0x01, process);
            self.master.write_all(&[0x01]).unwrap();
        }
        self.expect(0x4B, process);
        self.master.write_all(&[0x4B]).unwrap();
        self.expect(0x4A, process);
        self.master.write_all(&[0x4A]).unwrap();
    }

    fn confirm_stop(&mut self, process: &mut Process, signal: &str) {
        self.expect(0x4B, process);
        // A repeated signal must not skip cleanup or restart the session.
        process.signal(signal);
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(100) {
            process.assert_running();
            let mut byte = [0];
            match self.master.read(&mut byte) {
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                other => panic!("unexpected command while stop ACK withheld: {other:?}"),
            }
        }
        self.master.write_all(&[0x4B]).unwrap();
    }

    fn assert_no_more_commands(&mut self) {
        let mut byte = [0];
        let error = self
            .master
            .read(&mut byte)
            .expect_err("command after shutdown");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}

fn assert_interrupted(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "child did not exit cooperatively: {output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Interrupted"),
        "{output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Remote mode OFF"),
        "{output:?}"
    );
}

#[test]
fn passive_capture_signals_confirm_stop_ack_before_exit_without_recovery() {
    for signal in ["INT", "TERM"] {
        let mut radio = Radio::new();
        let mut process = Process::spawn(&radio.port, false);
        radio.bootstrap(&mut process);
        process.signal(signal);
        radio.confirm_stop(&mut process, signal);
        assert_interrupted(&process.output());
        radio.assert_no_more_commands();
        let mut expected = vec![0x01; 10];
        expected.extend([0x4B, 0x4A, 0x4B]);
        assert_eq!(radio.transcript, expected);
    }
}

#[test]
fn held_key_signals_release_then_confirm_stop_without_sending_queued_key() {
    for signal in ["INT", "TERM"] {
        let mut radio = Radio::new();
        let mut process = Process::spawn(&radio.port, true);
        radio.bootstrap(&mut process);
        radio.expect(0x8A, &mut process); // Menu down, only to this private PTY.
        let interrupted_at = Instant::now();
        process.signal(signal);
        radio.expect(0xFF, &mut process);
        assert!(
            interrupted_at.elapsed() < Duration::from_millis(350),
            "cancellation waited for the normal 400ms hold"
        );
        radio.confirm_stop(&mut process, signal);
        assert_interrupted(&process.output());
        radio.assert_no_more_commands();
        let mut expected = vec![0x01; 10];
        expected.extend([0x4B, 0x4A, 0x8A, 0xFF, 0x4B]);
        assert_eq!(radio.transcript, expected, "queued Exit must not be sent");
    }
}
