#![cfg(unix)]

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use nictui::remote::{
    RemoteCaptureEvent, RemoteCaptureSummary, RemoteControlCommand, RemoteSessionFailureKind,
    RemoteSessionOptions, RemoteSessionPhase, RemoteSessionResult, run_remote_session,
};
use serialport::{SerialPort, TTYPort};

struct Outcome {
    result: RemoteSessionResult<RemoteCaptureSummary>,
    phases: Vec<RemoteSessionPhase>,
    reports: Vec<nictui::remote::RemoteControlReport>,
    command_polls: usize,
}

struct LocalSession {
    master: TTYPort,
    _slave: TTYPort,
    stop: Arc<AtomicBool>,
    packet_seen: mpsc::Receiver<()>,
    completed: mpsc::Receiver<Outcome>,
    worker: Option<thread::JoinHandle<()>>,
    transcript: Vec<u8>,
}

impl LocalSession {
    fn new() -> Self {
        Self::with_options(RemoteSessionOptions {
            recover_retries: 3,
            command_reaction_window: Duration::from_secs(4),
            ..RemoteSessionOptions::default()
        })
    }

    fn with_options(options: RemoteSessionOptions) -> Self {
        Self::with_commands(
            options,
            VecDeque::from([
                RemoteControlCommand::raw_key("Menu", 0x0B),
                RemoteControlCommand::raw_key("Exit", 0x0E),
            ]),
        )
    }

    fn with_commands(
        options: RemoteSessionOptions,
        mut commands: VecDeque<RemoteControlCommand>,
    ) -> Self {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_millis(20)).unwrap();
        let port = slave.name().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requested = stop.clone();
        let (packet_tx, packet_seen) = mpsc::channel();
        let (completed_tx, completed) = mpsc::channel();
        let worker = thread::spawn(move || {
            let started = Instant::now();
            let mut command_polls = 0;
            let mut phases = Vec::new();
            let mut reports = Vec::new();
            let result = run_remote_session(
                &port,
                &options,
                // A one-shot local request must remain latched across cleanup.
                |_| {
                    requested.swap(false, Ordering::SeqCst)
                        || started.elapsed() > Duration::from_secs(8)
                },
                |_| {
                    command_polls += 1;
                    commands.pop_front()
                },
                |event| match event {
                    RemoteCaptureEvent::Phase(phase) => phases.push(phase),
                    RemoteCaptureEvent::Control(report) => reports.push(report),
                    RemoteCaptureEvent::Packet(_) => {
                        let _ = packet_tx.send(());
                    }
                    _ => {}
                },
            );
            let _ = completed_tx.send(Outcome {
                result,
                phases,
                reports,
                command_polls,
            });
        });
        Self {
            master,
            _slave: slave,
            stop,
            packet_seen,
            completed,
            worker: Some(worker),
            transcript: Vec::new(),
        }
    }

    fn expect(&mut self, expected: u8) {
        let started = Instant::now();
        loop {
            let mut byte = [0];
            match self.master.read(&mut byte) {
                Ok(1) => {
                    self.transcript.push(byte[0]);
                    assert_eq!(byte[0], expected, "wire: {:02X?}", self.transcript);
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                other => panic!("PTY read failed: {other:?}"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "missing {expected:02X}"
            );
        }
    }

    fn bootstrap(&mut self) {
        for byte in [0x01; 10].into_iter().chain([0x4B, 0x4A]) {
            self.expect(byte);
            self.master.write_all(&[byte]).unwrap();
        }
    }

    fn assert_ack_pending(&mut self) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(100) {
            assert!(!self.worker.as_ref().unwrap().is_finished());
            assert!(matches!(
                self.completed.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
            let mut byte = [0];
            match self.master.read(&mut byte) {
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                other => panic!("command while stop ACK withheld: {other:?}"),
            }
        }
    }

    fn finish_wire(&mut self) -> Outcome {
        let outcome = self.completed.recv_timeout(Duration::from_secs(2)).unwrap();
        self.worker.take().unwrap().join().unwrap();
        let mut byte = [0];
        let error = self
            .master
            .read(&mut byte)
            .expect_err("command after shutdown");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        outcome
    }

    fn finish(&mut self) -> Outcome {
        let outcome = self.finish_wire();
        let mut expected = vec![0x01; 10];
        expected.extend([0x4B, 0x4A, 0x8A, 0xFF, 0x4B]);
        assert_eq!(self.transcript, expected, "queued Exit must never be sent");
        assert_eq!(outcome.command_polls, 1);
        assert_eq!(
            outcome
                .phases
                .iter()
                .filter(|p| **p == RemoteSessionPhase::Opening)
                .count(),
            1
        );
        assert!(!outcome.phases.contains(&RemoteSessionPhase::Recovering));
        assert_eq!(outcome.reports.len(), 1);
        assert!(outcome.reports[0].reaction.is_none());
        outcome
    }
}

impl Drop for LocalSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn stop_at(reaction: bool, acknowledge: bool) {
    let mut session = LocalSession::new();
    session.bootstrap();
    session.expect(0x8A);
    if reaction {
        session.expect(0xFF);
        session.master.write_all(&[0x70]).unwrap();
        // Seeing a packet proves execution entered the long reaction window.
        session
            .packet_seen
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }
    let requested_at = Instant::now();
    session.stop.store(true, Ordering::SeqCst);
    if !reaction {
        session.expect(0xFF);
    }
    session.expect(0x4B);
    assert!(requested_at.elapsed() < Duration::from_millis(350));
    session.assert_ack_pending();
    if acknowledge {
        session.master.write_all(&[0x4B]).unwrap();
    }
    let outcome = session.finish();
    if acknowledge {
        assert_eq!(outcome.result.unwrap().recovery_count, 0);
        assert_eq!(outcome.phases.last(), Some(&RemoteSessionPhase::Stopped));
    } else {
        let failure = outcome.result.unwrap_err();
        assert_eq!(failure.kind, RemoteSessionFailureKind::StreamLost);
        assert_eq!(failure.summary, "Remote shutdown was not confirmed.");
        assert!(!outcome.phases.contains(&RemoteSessionPhase::Stopped));
    }
}

#[test]
fn local_stop_during_tap_releases_and_waits_for_ack_without_recovery() {
    stop_at(false, true);
}

#[test]
fn local_stop_during_reaction_skips_the_rest_of_the_window_and_queued_key() {
    stop_at(true, true);
}

#[test]
fn local_stop_during_tap_ack_timeout_never_reopens() {
    stop_at(false, false);
}

#[test]
fn local_stop_during_reaction_ack_timeout_never_reopens() {
    stop_at(true, false);
}

#[test]
fn preopen_local_stop_never_attempts_an_open() {
    let port = format!("/nictui-private-nonexistent-{}", std::process::id());
    let mut phases = Vec::new();
    let result = run_remote_session(
        &port,
        &RemoteSessionOptions {
            recover_retries: 3,
            ..RemoteSessionOptions::default()
        },
        |elapsed| {
            assert_eq!(elapsed, Duration::ZERO);
            true
        },
        |_| panic!("polled commands before arming"),
        |event| {
            if let RemoteCaptureEvent::Phase(phase) = event {
                phases.push(phase);
            }
        },
    );
    assert_eq!(result.unwrap().recovery_count, 0);
    assert_eq!(phases, [RemoteSessionPhase::Stopped]);
}

#[test]
fn local_stop_on_opening_status_prevents_the_open() {
    let stop = AtomicBool::new(false);
    let result = run_remote_session(
        "/nictui-private-nonexistent-opening-stop",
        &RemoteSessionOptions {
            recover_retries: 3,
            ..RemoteSessionOptions::default()
        },
        |elapsed| {
            assert_eq!(elapsed, Duration::ZERO);
            stop.load(Ordering::SeqCst)
        },
        |_| panic!("polled commands before arming"),
        |event| {
            if matches!(
                event,
                RemoteCaptureEvent::Phase(RemoteSessionPhase::Opening)
            ) {
                stop.store(true, Ordering::SeqCst);
            }
        },
    );
    assert_eq!(result.unwrap().recovery_count, 0);
}

#[test]
fn local_stop_after_open_failure_preserves_error_without_retrying() {
    let mut checks = 0;
    let mut phases = Vec::new();
    let result = run_remote_session(
        "/nictui-private-nonexistent-open-failure",
        &RemoteSessionOptions {
            recover_retries: 3,
            ..RemoteSessionOptions::default()
        },
        |elapsed| {
            assert_eq!(elapsed, Duration::ZERO);
            checks += 1;
            checks == 3
        },
        |_| panic!("polled commands before arming"),
        |event| {
            if let RemoteCaptureEvent::Phase(phase) = event {
                phases.push(phase);
            }
        },
    );
    assert_eq!(
        result.unwrap_err().kind,
        RemoteSessionFailureKind::OpenFailed
    );
    assert_eq!(phases, [RemoteSessionPhase::Opening]);
}

#[test]
fn local_stop_after_failed_bootstrap_waits_for_cleanup_ack_and_never_reopens() {
    for cleanup_ack in [true, false] {
        let mut session = LocalSession::new();
        for _ in 0..10 {
            session.expect(0x01);
            session.master.write_all(&[0x01]).unwrap();
        }
        session.expect(0x4B);
        session.stop.store(true, Ordering::SeqCst);
        // Withhold the bootstrap ACK, then independently confirm cleanup.
        session.expect(0x4B);
        session.assert_ack_pending();
        if cleanup_ack {
            session.master.write_all(&[0x4B]).unwrap();
        }
        let outcome = session.finish_wire();
        let failure = outcome.result.unwrap_err();
        assert_eq!(
            failure.kind,
            RemoteSessionFailureKind::BootstrapHandshakeFailed
        );
        assert_eq!(
            failure.detail.contains("remote cleanup failed"),
            !cleanup_ack
        );
        assert_eq!(outcome.phases, [RemoteSessionPhase::Opening]);
        assert_eq!(outcome.command_polls, 0);
        let mut expected = vec![0x01; 10];
        expected.extend([0x4B, 0x4B]);
        assert_eq!(session.transcript, expected);
    }
}

#[test]
fn local_stop_after_failed_disable_waits_for_enable_ack_and_never_bootstraps() {
    for cleanup_ack in [true, false] {
        let mut session = LocalSession::with_options(RemoteSessionOptions {
            disable_radio_before_remote: true,
            recover_retries: 3,
            ..RemoteSessionOptions::default()
        });
        session.expect(0x01);
        session.master.write_all(&[0x01]).unwrap();
        session.expect(0x45);
        session.stop.store(true, Ordering::SeqCst);
        session.master.write_all(&[0]).unwrap();
        session.expect(0x46);
        session.assert_ack_pending();
        if cleanup_ack {
            session.master.write_all(&[0x46]).unwrap();
        }
        let outcome = session.finish_wire();
        let failure = outcome.result.unwrap_err();
        assert_eq!(
            failure.kind,
            RemoteSessionFailureKind::BootstrapHandshakeFailed
        );
        assert_eq!(
            failure.detail.contains("remote cleanup failed"),
            !cleanup_ack
        );
        assert_eq!(outcome.phases, [RemoteSessionPhase::Opening]);
        assert_eq!(outcome.command_polls, 0);
        assert_eq!(session.transcript, [0x01, 0x45, 0x46]);
    }
}

#[test]
fn local_stop_with_successful_bootstrap_ack_never_sends_start() {
    let mut session = LocalSession::new();
    for _ in 0..10 {
        session.expect(0x01);
        session.master.write_all(&[0x01]).unwrap();
    }
    session.expect(0x4B);
    session.stop.store(true, Ordering::SeqCst);
    session.master.write_all(&[0x4B]).unwrap();
    session.expect(0x4B);
    session.assert_ack_pending();
    session.master.write_all(&[0x4B]).unwrap();
    let outcome = session.finish_wire();
    let failure = outcome.result.unwrap_err();
    assert_eq!(failure.kind, RemoteSessionFailureKind::StreamLost);
    assert_eq!(failure.summary, "Remote session stopped during bootstrap.");
    assert_eq!(outcome.command_polls, 0);
    assert_eq!(outcome.phases, [RemoteSessionPhase::Opening]);
    let mut expected = vec![0x01; 10];
    expected.extend([0x4B, 0x4B]);
    assert_eq!(session.transcript, expected);
}

#[test]
fn unexpected_start_ack_frame_payload_cannot_confirm_cleanup() {
    let mut session = LocalSession::new();
    for byte in [0x01; 10].into_iter().chain([0x4B]) {
        session.expect(byte);
        session.master.write_all(&[byte]).unwrap();
    }
    session.expect(0x4A);
    session.stop.store(true, Ordering::SeqCst);
    // A rectangle opcode is not a start ACK; its first payload byte is not a stop ACK.
    session
        .master
        .write_all(&[0x65, 0x4B, 0, 0, 0, 0, 0])
        .unwrap();
    session.expect(0x4B);
    let outcome = session.finish_wire();
    let failure = outcome.result.unwrap_err();
    assert_eq!(failure.kind, RemoteSessionFailureKind::RemoteOnAckFailed);
    assert!(
        failure.detail.contains("expected 4A, got 65"),
        "{failure:?}"
    );
    assert!(
        failure.detail.contains("remote cleanup failed"),
        "{failure:?}"
    );
    assert!(
        failure.detail.contains("decoder frame boundary is unknown"),
        "{failure:?}"
    );
    assert_eq!(outcome.phases, [RemoteSessionPhase::Opening]);
    assert_eq!(outcome.command_polls, 0);
    assert!(outcome.reports.is_empty());
    let mut expected = vec![0x01; 10];
    expected.extend([0x4B, 0x4A, 0x4B]);
    assert_eq!(session.transcript, expected);
}

#[derive(Clone, Copy)]
enum PartialFrameAt {
    Bootstrap,
    Stream,
    Reaction,
}

fn partial_payload_cannot_ack_stop(at: PartialFrameAt) {
    let options = RemoteSessionOptions {
        recover_retries: 3,
        command_reaction_window: Duration::from_secs(4),
        ..RemoteSessionOptions::default()
    };
    let mut session = if matches!(at, PartialFrameAt::Reaction) {
        LocalSession::with_options(options)
    } else {
        LocalSession::with_commands(options, VecDeque::new())
    };
    if matches!(at, PartialFrameAt::Bootstrap) {
        for _ in 0..10 {
            session.expect(0x01);
            session.master.write_all(&[0x01]).unwrap();
        }
        session.expect(0x4B);
    } else {
        session.bootstrap();
        if matches!(at, PartialFrameAt::Reaction) {
            session.expect(0x8A);
            session.expect(0xFF);
        }
        session.master.write_all(&[0x70]).unwrap();
        session
            .packet_seen
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }
    // The decoder consumes two of six rectangle payload bytes, then blocks.
    session.master.write_all(&[0x65, 0, 0]).unwrap();
    thread::sleep(Duration::from_millis(100));
    session.stop.store(true, Ordering::SeqCst);
    session.expect(0x4B);
    // This 4B is a late payload byte, never a standalone stop acknowledgment.
    session.master.write_all(&[0x4B, 0, 0, 0]).unwrap();
    let outcome = session.finish_wire();
    let failure = outcome.result.unwrap_err();
    assert!(
        failure.detail.contains("decoder frame boundary is unknown"),
        "{failure:?}"
    );
    assert!(failure.detail.contains("remote cleanup failed"));
    match at {
        PartialFrameAt::Bootstrap => assert_eq!(
            failure.kind,
            RemoteSessionFailureKind::BootstrapHandshakeFailed
        ),
        PartialFrameAt::Stream | PartialFrameAt::Reaction => {
            assert_eq!(failure.kind, RemoteSessionFailureKind::StreamLost);
            assert!(failure.detail.contains("got 2/6 bytes"), "{failure:?}");
            assert!(
                failure.summary.contains("packet stream failed"),
                "{failure:?}"
            );
        }
    }
    assert!(!outcome.phases.contains(&RemoteSessionPhase::Stopped));
    assert!(!outcome.phases.contains(&RemoteSessionPhase::Recovering));
    let mut expected = vec![0x01; 10];
    expected.push(0x4B);
    if !matches!(at, PartialFrameAt::Bootstrap) {
        expected.push(0x4A);
    }
    if matches!(at, PartialFrameAt::Reaction) {
        expected.extend([0x8A, 0xFF]);
    }
    expected.push(0x4B);
    assert_eq!(session.transcript, expected);
}

#[test]
fn partial_stream_payload_stop_byte_cannot_confirm_cleanup() {
    partial_payload_cannot_ack_stop(PartialFrameAt::Stream);
}

#[test]
fn partial_reaction_payload_stop_byte_cannot_confirm_cleanup() {
    partial_payload_cannot_ack_stop(PartialFrameAt::Reaction);
}

#[test]
fn partial_bootstrap_payload_stop_byte_cannot_confirm_cleanup() {
    partial_payload_cannot_ack_stop(PartialFrameAt::Bootstrap);
}
