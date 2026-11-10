use super::*;
use serialport::SerialPort;
use std::cell::Cell;
use std::io::{Read, Write};

fn in_bounded_pty_child(test_name: &str) -> bool {
    const CHILD_TEST: &str = "NICTUI_REMOTE_HOLD_CHILD_TEST";
    if std::env::var(CHILD_TEST).as_deref() == Ok(test_name) {
        return true;
    }
    // macOS tcdrain can block beyond the serial timeout. Keep that syscall out
    // of the test runner so a broken PTY exchange fails instead of hanging CI.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(CHILD_TEST, test_name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "{test_name} failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return false;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "{test_name} exceeded the 10s PTY deadline:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn expect_bytes(peer: &mut serialport::TTYPort, expected: &[u8]) {
    let previous_timeout = peer.timeout();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::with_capacity(expected.len());
    // Complete each one-byte read before the sender's next write/flush pair.
    while bytes.len() < expected.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "PTY read deadline exceeded: expected {expected:02X?}, received {bytes:02X?}"
        );
        peer.set_timeout(remaining.min(Duration::from_millis(50)))
            .unwrap();
        let mut byte = [0];
        match peer.read(&mut byte) {
            Ok(1) => bytes.push(byte[0]),
            Ok(_) => panic!("PTY closed before expected bytes: {bytes:02X?}"),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => panic!("PTY read failed after {bytes:02X?}: {error}"),
        }
    }
    peer.set_timeout(previous_timeout).unwrap();
    assert_eq!(bytes, expected);
}

fn pair() -> (serialport::TTYPort, serialport::TTYPort, String) {
    let (mut peer, slave) = serialport::TTYPort::pair().unwrap();
    peer.set_timeout(Duration::from_secs(3)).unwrap();
    let path = slave.name().unwrap();
    (peer, slave, path)
}

fn bootstrap(peer: &mut serialport::TTYPort) {
    for _ in 0..NICFW_REMOTE_SYNC_COUNT {
        expect_bytes(peer, &[0x01]);
        peer.write_all(&[0x01]).unwrap();
    }
    expect_bytes(peer, &[0x4B]);
    peer.write_all(&[0x4B]).unwrap();
    expect_bytes(peer, &[0x4A]);
    peer.write_all(&[0x4A]).unwrap();
}

fn stop(peer: &mut serialport::TTYPort) {
    expect_bytes(peer, &[0x4B]);
    peer.write_all(&[0x4B]).unwrap();
    peer.set_timeout(Duration::from_millis(100)).unwrap();
    let error = peer
        .read(&mut [0])
        .expect_err("unexpected command after stop");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn remote_hold_lasts_until_explicit_release_not_tap_timer() {
    if !in_bounded_pty_child(
        "remote::hold_tests::remote_hold_lasts_until_explicit_release_not_tap_timer",
    ) {
        return;
    }
    let (mut peer, _slave, path) = pair();
    let worker = thread::spawn(move || {
        bootstrap(&mut peer);
        expect_bytes(&mut peer, &[0x90]);
        let pressed_at = Instant::now();
        expect_bytes(&mut peer, &[0xFF]);
        let held = pressed_at.elapsed();
        stop(&mut peer);
        held
    });
    let pressed_at = Cell::new(None::<Instant>);
    let released = Cell::new(false);
    let reports = Cell::new(0);
    let result = run_remote_session(
        &path,
        &RemoteSessionOptions {
            command_reaction_window: Duration::from_secs(5),
            ..RemoteSessionOptions::default()
        },
        |elapsed| reports.get() == 2 || elapsed > Duration::from_secs(2),
        |_| match pressed_at.get() {
            None => {
                pressed_at.set(Some(Instant::now()));
                Some(RemoteControlCommand::press_key("PTT", 0x13))
            }
            Some(started) if started.elapsed() >= Duration::from_millis(650) => {
                (!released.replace(true)).then(RemoteControlCommand::release_key)
            }
            _ => None,
        },
        |event| {
            if let RemoteCaptureEvent::Control(report) = event {
                assert!(report.success);
                assert!(report.reaction.is_none());
                reports.set(reports.get() + 1);
            }
        },
    );
    let held = worker.join().unwrap();
    result.unwrap();
    assert_eq!(reports.get(), 2);
    assert!(held >= Duration::from_millis(600), "hold was {held:?}");
    assert!(
        held < Duration::from_secs(2),
        "release was delayed: {held:?}"
    );
}

#[test]
fn remote_hold_queued_release_skips_reaction_window() {
    if !in_bounded_pty_child("remote::hold_tests::remote_hold_queued_release_skips_reaction_window")
    {
        return;
    }
    let (mut peer, _slave, path) = pair();
    let worker = thread::spawn(move || {
        bootstrap(&mut peer);
        expect_bytes(&mut peer, &[0x90]);
        let pressed_at = Instant::now();
        expect_bytes(&mut peer, &[0xFF]);
        let release_latency = pressed_at.elapsed();
        stop(&mut peer);
        release_latency
    });
    let mut commands = [
        RemoteControlCommand::press_key("PTT", 0x13),
        RemoteControlCommand::release_key(),
    ]
    .into_iter();
    let reports = Cell::new(0);
    let result = run_remote_session(
        &path,
        &RemoteSessionOptions {
            command_reaction_window: Duration::from_secs(5),
            ..RemoteSessionOptions::default()
        },
        |elapsed| reports.get() == 2 || elapsed > Duration::from_secs(2),
        |_| commands.next(),
        |event| {
            if let RemoteCaptureEvent::Control(report) = event {
                assert!(report.success);
                reports.set(reports.get() + 1);
            }
        },
    );
    let latency = worker.join().unwrap();
    result.unwrap();
    assert_eq!(reports.get(), 2);
    assert!(
        latency < Duration::from_millis(200),
        "release latency {latency:?}"
    );
}

#[test]
fn remote_hold_stop_releases_before_stop_command() {
    if !in_bounded_pty_child("remote::hold_tests::remote_hold_stop_releases_before_stop_command") {
        return;
    }
    let (mut peer, _slave, path) = pair();
    let worker = thread::spawn(move || {
        bootstrap(&mut peer);
        expect_bytes(&mut peer, &[0x90, 0xFF]);
        stop(&mut peer);
    });
    let sent = Cell::new(false);
    let done = Cell::new(false);
    let result = run_remote_session(
        &path,
        &RemoteSessionOptions::default(),
        |elapsed| done.get() || elapsed > Duration::from_secs(2),
        |_| (!sent.replace(true)).then(|| RemoteControlCommand::press_key("PTT", 0x13)),
        |event| {
            if matches!(event, RemoteCaptureEvent::Control(_)) {
                done.set(true);
            }
        },
    );
    worker.join().unwrap();
    result.unwrap();
}

#[test]
fn remote_hold_unwind_releases_before_cleanup() {
    if !in_bounded_pty_child("remote::hold_tests::remote_hold_unwind_releases_before_cleanup") {
        return;
    }
    let (mut peer, _slave, path) = pair();
    let worker = thread::spawn(move || {
        bootstrap(&mut peer);
        expect_bytes(&mut peer, &[0x90, 0xFF]);
        stop(&mut peer);
    });
    let result = std::panic::catch_unwind(|| {
        let sent = Cell::new(false);
        let _ = run_remote_session(
            &path,
            &RemoteSessionOptions::default(),
            |elapsed| elapsed > Duration::from_secs(2),
            |_| (!sent.replace(true)).then(|| RemoteControlCommand::press_key("PTT", 0x13)),
            |event| {
                assert!(
                    !matches!(event, RemoteCaptureEvent::Control(_)),
                    "simulated unwind"
                );
            },
        );
    });
    worker.join().unwrap();
    assert!(result.is_err());
}

#[test]
fn remote_hold_safety_deadline_releases_once_and_heartbeat_stays_released() {
    let mut writes = Vec::new();
    let mut current_key = 0x90;
    let mut deadline = Some(MAX_REMOTE_KEY_HOLD);
    assert!(
        !release_expired_remote_key(
            &mut current_key,
            &mut deadline,
            MAX_REMOTE_KEY_HOLD - Duration::from_millis(1),
            |bytes| {
                writes.extend_from_slice(bytes);
                Ok(())
            },
        )
        .unwrap()
    );
    assert!(
        release_expired_remote_key(
            &mut current_key,
            &mut deadline,
            MAX_REMOTE_KEY_HOLD,
            |bytes| {
                writes.extend_from_slice(bytes);
                Ok(())
            },
        )
        .unwrap()
    );
    assert_eq!(current_key, NICFW_REMOTE_RELEASE);
    assert_eq!(deadline, None);
    assert!(
        !release_expired_remote_key(
            &mut current_key,
            &mut deadline,
            MAX_REMOTE_KEY_HOLD + Duration::from_secs(1),
            |bytes| {
                writes.extend_from_slice(bytes);
                Ok(())
            },
        )
        .unwrap()
    );
    let mut heartbeat = NICFW_REMOTE_HEARTBEAT_INTERVAL;
    service_remote_heartbeat_with_writer(
        current_key,
        &mut heartbeat,
        NICFW_REMOTE_HEARTBEAT_INTERVAL,
        |bytes| {
            writes.extend_from_slice(bytes);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(writes, [0xFF, 0xFF]);
}

#[test]
fn remote_hold_heartbeat_preserves_physical_ptt_code() {
    let command = RemoteControlCommand::press_key("PTT", 0x13);
    let mut writes = command.steps[0].bytes.clone();
    let mut key = heartbeat_key_after_step(&command, &command.steps[0]).unwrap();
    let mut heartbeat = NICFW_REMOTE_HEARTBEAT_INTERVAL;
    service_remote_heartbeat_with_writer(
        key,
        &mut heartbeat,
        NICFW_REMOTE_HEARTBEAT_INTERVAL,
        |bytes| {
            writes.extend_from_slice(bytes);
            Ok(())
        },
    )
    .unwrap();
    release_remote_key_with_writer(&mut key, |bytes| {
        writes.extend_from_slice(bytes);
        Ok(())
    })
    .unwrap();
    assert_eq!(writes, [0x90, 0x90, 0xFF]);
}

#[test]
fn remote_hold_failed_safety_release_retains_cleanup_ownership() {
    let mut key = 0x90;
    let mut deadline = Some(MAX_REMOTE_KEY_HOLD);
    let error = release_expired_remote_key(&mut key, &mut deadline, MAX_REMOTE_KEY_HOLD, |_| {
        anyhow::bail!("mock write failure")
    })
    .unwrap_err();
    assert!(error.detail.contains("mock write failure"));
    assert_eq!(key, 0x90);
    assert_eq!(deadline, Some(MAX_REMOTE_KEY_HOLD));
    let mut writes = Vec::new();
    assert!(
        release_expired_remote_key(&mut key, &mut deadline, MAX_REMOTE_KEY_HOLD, |bytes| {
            writes.extend_from_slice(bytes);
            Ok(())
        },)
        .unwrap()
    );
    assert_eq!(writes, [0xFF]);
    assert_eq!(key, NICFW_REMOTE_RELEASE);
    assert_eq!(deadline, None);
}

#[test]
fn remote_hold_failed_heartbeat_does_not_advance_schedule() {
    let mut heartbeat = NICFW_REMOTE_HEARTBEAT_INTERVAL;
    assert!(
        service_remote_heartbeat_with_writer(
            0x90,
            &mut heartbeat,
            NICFW_REMOTE_HEARTBEAT_INTERVAL,
            |_| anyhow::bail!("mock write failure"),
        )
        .is_err()
    );
    assert_eq!(heartbeat, NICFW_REMOTE_HEARTBEAT_INTERVAL);
}

#[test]
fn remote_hold_stream_failure_releases_and_does_not_resume_after_recovery() {
    if !in_bounded_pty_child(
        "remote::hold_tests::remote_hold_stream_failure_releases_and_does_not_resume_after_recovery",
    ) {
        return;
    }
    let (mut peer, _slave, path) = pair();
    let worker = thread::spawn(move || {
        bootstrap(&mut peer);
        expect_bytes(&mut peer, &[0x90]);
        // Truncate a display packet so the next stream read fails.
        peer.write_all(&[0x64, 0]).unwrap();
        expect_bytes(&mut peer, &[0xFF, 0x4B]);
        // The failed decoder cannot validate a stop ACK. The new session starts neutral.
        bootstrap(&mut peer);
        stop(&mut peer);
    });
    let sent = Cell::new(false);
    let sessions = Cell::new(0);
    let result = run_remote_session(
        &path,
        &RemoteSessionOptions {
            recover_retries: 1,
            ..RemoteSessionOptions::default()
        },
        |elapsed| sessions.get() == 2 || elapsed > Duration::from_secs(2),
        |_| (!sent.replace(true)).then(|| RemoteControlCommand::press_key("PTT", 0x13)),
        |event| {
            if matches!(event, RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed)) {
                sessions.set(sessions.get() + 1);
            }
        },
    );
    worker.join().unwrap();
    assert_eq!(result.unwrap().recovery_count, 1);
    assert_eq!(sessions.get(), 2);
}
