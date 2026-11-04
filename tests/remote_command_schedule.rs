#![cfg(unix)]

use std::io::{self, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use nictui::device::{RemoteMonitorEvent, RemoteMonitorOptions, monitor_remote_with_summary};
use nictui::remote::{RemoteControlCommand, RemoteEvidenceKind, RemoteSessionPhase};
use serialport::{SerialPort, TTYPort};

struct SilentRemote {
    slave: TTYPort,
    injector: Box<dyn SerialPort>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Vec<(u8, Instant)>>>,
}

impl SilentRemote {
    fn new() -> Self {
        let (mut master, slave) = TTYPort::pair().expect("create isolated remote PTY");
        master.set_timeout(Duration::from_millis(20)).unwrap();
        let injector = master.try_clone().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut keys = Vec::new();
            while !stopped.load(Ordering::Relaxed) {
                let mut byte = [0];
                match master.read_exact(&mut byte) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(error) => panic!("remote PTY read failed: {error}"),
                }
                match byte[0] {
                    // Echo bootstrap bytes only; never send display/telemetry packets.
                    0x01 | 0x4A | 0x4B => master.write_all(&byte).unwrap(),
                    0xFF => {}
                    key => keys.push((key, Instant::now())),
                }
            }
            keys
        });
        Self {
            slave,
            injector,
            stop,
            worker: Some(worker),
        }
    }

    fn port(&self) -> String {
        self.slave.name().unwrap()
    }

    fn finish(mut self) -> Vec<(u8, Instant)> {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for SilentRemote {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn menu_exit_options() -> RemoteMonitorOptions {
    RemoteMonitorOptions {
        duration: Duration::from_millis(2300),
        scripted_commands: vec![
            RemoteControlCommand::raw_key("menu", 0x0B),
            RemoteControlCommand::raw_key("exit", 0x0E),
        ],
        command_start_delay: Duration::from_millis(200),
        // Raw-key holds plus the reaction window exceed 200ms, so use 1s
        // to distinguish an incorrectly immediate second command from a due one.
        key_interval: Duration::from_secs(1),
        ..RemoteMonitorOptions::default()
    }
}

#[test]
fn post_command_armed_without_telemetry_preserves_scripted_interval() {
    let radio = SilentRemote::new();
    let options = menu_exit_options();
    let mut phases = Vec::new();
    let mut reports = Vec::new();
    let summary = monitor_remote_with_summary(&radio.port(), &options, |event| match event {
        RemoteMonitorEvent::Phase(phase) => phases.push(phase),
        RemoteMonitorEvent::Control(report) => reports.push(report),
        _ => {}
    })
    .unwrap();
    let keys = radio.finish();

    assert_eq!(summary.packet_count, 0);
    assert_eq!(summary.recovery_count, 0);
    assert_eq!(
        keys.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        [0x8A, 0x8D]
    );
    assert_eq!(reports.len(), 2);
    for report in reports {
        assert!(report.success);
        assert_eq!(report.evidence, RemoteEvidenceKind::NoTelemetry);
    }
    assert_eq!(
        phases,
        [
            RemoteSessionPhase::Opening,
            RemoteSessionPhase::Armed,
            RemoteSessionPhase::Probing,
            RemoteSessionPhase::Armed,
            RemoteSessionPhase::Probing,
            RemoteSessionPhase::Armed,
            RemoteSessionPhase::Stopped,
        ]
    );
    let spacing = keys[1].1.duration_since(keys[0].1);
    assert!(
        spacing >= options.key_interval - Duration::from_millis(10),
        "scripted keys were only {spacing:?} apart"
    );
}

#[test]
fn recovery_restarts_delay_without_replaying_successful_commands() {
    let mut radio = SilentRemote::new();
    let options = RemoteMonitorOptions {
        duration: Duration::from_millis(1500),
        key_interval: Duration::from_secs(5),
        recover_retries: 1,
        ..menu_exit_options()
    };
    let mut phases = Vec::new();
    let mut reports = Vec::new();
    let summary = monitor_remote_with_summary(&radio.port(), &options, |event| match event {
        RemoteMonitorEvent::Phase(phase) => phases.push((phase, Instant::now())),
        RemoteMonitorEvent::Control(report) => {
            assert!(report.success);
            if reports.is_empty() {
                // A rectangle opcode without its body forces stream recovery,
                // but only after Menu's successful report has advanced the index.
                radio.injector.write_all(&[0x65]).unwrap();
            }
            reports.push(report.label);
        }
        _ => {}
    })
    .unwrap();
    let keys = radio.finish();

    assert_eq!(summary.recovery_count, 1);
    assert_eq!(summary.packet_count, 0);
    assert_eq!(reports, ["menu", "exit"]);
    assert_eq!(
        keys.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        [0x8A, 0x8D]
    );
    let recovering = phases
        .iter()
        .position(|(phase, _)| *phase == RemoteSessionPhase::Recovering)
        .expect("stream fault must trigger recovery");
    assert_eq!(phases[recovering + 1].0, RemoteSessionPhase::Opening);
    assert_eq!(phases[recovering + 2].0, RemoteSessionPhase::Armed);
    assert_eq!(phases[recovering + 3].0, RemoteSessionPhase::Probing);
    let recovered_delay = phases[recovering + 3].1 - phases[recovering + 2].1;
    assert!(recovered_delay >= options.command_start_delay);
    assert!(
        recovered_delay < Duration::from_secs(1),
        "recovered command retained the previous session's deadline: {recovered_delay:?}"
    );
}
