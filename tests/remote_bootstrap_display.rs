#![cfg(unix)]

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use nictui::protocol::RemotePacket;
use nictui::remote::{
    RemoteCaptureEvent, RemoteCaptureSummary, RemoteSessionFailure, RemoteSessionFailureKind,
    RemoteSessionOptions, RemoteSessionPhase, run_remote_session,
};
use serialport::{SerialPort, TTYPort};

#[derive(Clone, Copy)]
enum Delivery {
    Coalesced,
    Fragmented,
}

struct MockRadio {
    port: TTYPort,
    writes: Vec<u8>,
}

impl MockRadio {
    fn expect(&mut self, expected: u8) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let mut byte = [0];
            match self.port.read(&mut byte) {
                Ok(1) => {
                    self.writes.push(byte[0]);
                    assert_eq!(byte[0], expected, "host writes: {:02X?}", self.writes);
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                other => panic!("mock PTY read: {other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "missing host byte {expected:02X}"
            );
        }
    }

    fn quiet(&mut self) {
        let mut byte = [0];
        let error = self.port.read(&mut byte).expect_err(&format!(
            "host advanced without a boundary ACK: {:02X?}",
            byte
        ));
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    fn send(&mut self, bytes: &[u8], delivery: Delivery) {
        match delivery {
            Delivery::Coalesced => self.port.write_all(bytes).unwrap(),
            Delivery::Fragmented => {
                for byte in bytes {
                    self.port.write_all(&[*byte]).unwrap();
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn bootstrap(&mut self) {
        for byte in [0x01; 10].into_iter().chain([0x4B]) {
            self.expect(byte);
            self.port.write_all(&[byte]).unwrap();
        }
        self.expect(0x4A);
    }
}

struct Capture {
    result: Result<RemoteCaptureSummary, RemoteSessionFailure>,
    events: Vec<RemoteCaptureEvent>,
}

fn capture(stop_after: usize, script: impl FnOnce(&mut MockRadio) + Send + 'static) -> Capture {
    // Only a newly allocated PTY is opened; no device discovery or physical port.
    let (mut master, slave) = TTYPort::pair().unwrap();
    master.set_timeout(Duration::from_millis(30)).unwrap();
    let path = slave.name().unwrap();
    let worker = thread::spawn(move || {
        let mut radio = MockRadio {
            port: master,
            writes: Vec::new(),
        };
        script(&mut radio);
        radio.expect(0x4B);
        radio.port.write_all(&[0x4B]).unwrap();
        radio.quiet();
        radio.writes
    });
    let received = Cell::new(0);
    let mut events = Vec::new();
    let result = run_remote_session(
        &path,
        &RemoteSessionOptions::default(),
        // Include bootstrap and deliberately paced frame delivery in the safety budget.
        |elapsed| received.get() >= stop_after || elapsed > Duration::from_secs(8),
        |_| None,
        |event| {
            if matches!(event, RemoteCaptureEvent::Packet(_)) {
                received.set(received.get() + 1);
            }
            events.push(event);
        },
    );
    let writes = worker.join().expect("mock radio failed");
    let mut allowed = vec![0x01; 10];
    allowed.extend([0x4B, 0x4A, 0x4B]);
    assert_eq!(
        writes, allowed,
        "no keys, keepalive, recovery or write commands"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RemoteCaptureEvent::Control(_)))
    );
    Capture { result, events }
}

fn redraw(label: &[u8]) -> Vec<u8> {
    let mut bytes = vec![
        0x65, 0, 0, 128, 128, 0, 0x80, // Full background.
        0x65, 1, 1, 0x4A, 0x4B, 0x01, 0x4B, // Sync/ACK bytes are payload.
        0x64, 2, 4, 40, 0x4A, 0x4B, 0, 0x80,
    ];
    bytes.extend_from_slice(label);
    bytes.extend_from_slice(&[
        0, 0, 0, // Text terminator and firmware NOP padding.
        0x66, 0, 0, 0, 0xFF, 7, 0, 0x80,
    ]);
    bytes
}

fn expected_redraw(label: &str) -> Vec<RemotePacket> {
    vec![
        RemotePacket::DrawRectangle {
            x: 0,
            y: 0,
            width: 128,
            height: 128,
            color: 0x8000,
        },
        RemotePacket::DrawRectangle {
            x: 1,
            y: 1,
            width: 0x4A,
            height: 0x4B,
            color: 0x4B01,
        },
        RemotePacket::DisplayText {
            font_size: 2,
            x: 4,
            y: 40,
            fg_color: 0x4B4A,
            bg_color: 0x8000,
            text: label.to_string(),
        },
        RemotePacket::DrawSymbol {
            symbol_id: 0,
            x: 0,
            y: 0,
            fg_color: 0x07FF,
            bg_color: 0x8000,
        },
    ]
}

fn assert_capture(capture: Capture, expected: &[RemotePacket]) {
    let summary = capture.result.expect("valid start/stream/stop");
    assert_eq!(summary.packet_count, expected.len());
    assert_eq!(summary.unknown_packet_count, 0);
    assert_eq!(summary.recovery_count, 0);
    let actual: Vec<_> = capture
        .events
        .iter()
        .filter_map(|event| match event {
            RemoteCaptureEvent::Packet(packet) => Some(format!("{packet:?}")),
            _ => None,
        })
        .collect();
    let expected: Vec<_> = expected
        .iter()
        .map(|packet| format!("{packet:?}"))
        .collect();
    assert_eq!(
        actual, expected,
        "bootstrap history must not become a fresh display"
    );
    let phases: Vec<_> = capture
        .events
        .iter()
        .filter_map(|event| match event {
            RemoteCaptureEvent::Phase(phase) => Some(*phase),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        [
            RemoteSessionPhase::Opening,
            RemoteSessionPhase::Armed,
            RemoteSessionPhase::Live,
            RemoteSessionPhase::Stopped
        ]
    );
    let armed = capture
        .events
        .iter()
        .position(|event| matches!(event, RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed)))
        .unwrap();
    let first_packet = capture
        .events
        .iter()
        .position(|event| matches!(event, RemoteCaptureEvent::Packet(_)))
        .unwrap();
    assert!(armed < first_packet);
}

fn interleaved_bootstrap(delivery: Delivery) {
    let capture = capture(4, move |radio| {
        for _ in 0..10 {
            radio.expect(0x01);
            // A previous remote session can still mirror while sync is pending.
            let mut reply = redraw(b"OLD\x01JK");
            reply.push(0x01);
            radio.send(&reply, delivery);
        }
        radio.expect(0x4B);
        let mut reply = redraw(b"OLD-STOP-JK");
        reply.push(0x4B);
        radio.send(&reply, delivery);
        radio.expect(0x4A);
        let mut reply = vec![0x4A];
        reply.extend(redraw(b"146.520"));
        radio.send(&reply, delivery);
    });
    assert_capture(capture, &expected_redraw("146.520"));
}

#[test]
fn coalesced_bootstrap_history_is_discarded_but_new_redraw_is_preserved() {
    interleaved_bootstrap(Delivery::Coalesced);
}

#[test]
fn fragmented_bootstrap_history_is_discarded_but_new_redraw_is_preserved() {
    interleaved_bootstrap(Delivery::Fragmented);
}

#[test]
fn ack_bytes_inside_complete_frames_cannot_advance_bootstrap() {
    let capture = capture(4, |radio| {
        for index in 0..10 {
            radio.expect(0x01);
            if index == 0 {
                radio.send(&redraw(b"OLD\x01JK"), Delivery::Coalesced);
                radio.quiet(); // Includes a 01 payload, but no sync echo yet.
            }
            radio.port.write_all(&[0x01]).unwrap();
        }
        radio.expect(0x4B);
        radio.send(&redraw(b"OLD-JK"), Delivery::Fragmented);
        radio.quiet(); // Includes several 4B payloads, but no stop echo yet.
        radio.port.write_all(&[0x4B]).unwrap();
        radio.expect(0x4A);
        radio.port.write_all(&[0x4A]).unwrap();
        radio.send(&redraw(b"146.520"), Delivery::Coalesced);
    });
    assert_capture(capture, &expected_redraw("146.520"));
}

#[test]
fn bootstrap_full_redraw_followed_by_only_battery_does_not_resurrect_old_screen() {
    let capture = capture(1, |radio| {
        for index in 0..10 {
            radio.expect(0x01);
            if index == 9 {
                radio.send(&redraw(b"OLD-146.520"), Delivery::Coalesced);
            }
            radio.port.write_all(&[0x01]).unwrap();
        }
        radio.expect(0x4B);
        radio.port.write_all(&[0x4B]).unwrap();
        radio.expect(0x4A);
        radio
            .port
            .write_all(&[
                0x4A, 0x64, 0, 96, 120, 0xFF, 0xFF, 0, 0x80, b'7', b'.', b'2', b'V', 0,
            ])
            .unwrap();
    });
    assert_capture(
        capture,
        &[RemotePacket::DisplayText {
            font_size: 0,
            x: 96,
            y: 120,
            fg_color: 0xFFFF,
            bg_color: 0x8000,
            text: "7.2V".to_string(),
        }],
    );
}

fn assert_rejected(capture: Capture) {
    let failure = capture
        .result
        .expect_err("draw bytes are not an immediate start ACK");
    assert_eq!(failure.kind, RemoteSessionFailureKind::RemoteOnAckFailed);
    assert!(
        failure.detail.contains("expected 4A, got 65"),
        "{failure:?}"
    );
    assert!(
        failure.detail.contains("decoder frame boundary is unknown"),
        "{failure:?}"
    );
    assert!(!capture.events.iter().any(|event| matches!(
        event,
        RemoteCaptureEvent::Packet(_)
            | RemoteCaptureEvent::Received(_)
            | RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed | RemoteSessionPhase::Live)
    )));
}

#[test]
fn queued_old_frame_after_stop_echo_cannot_be_promoted_by_driver_buffering() {
    let capture = capture(usize::MAX, |radio| {
        for _ in 0..10 {
            radio.expect(0x01);
            radio.port.write_all(&[0x01]).unwrap();
        }
        radio.expect(0x4B);
        // Adversarial ordering, not a firmware-conforming new redraw: these
        // bytes are already queued before the host has sent the new start.
        let mut reply = vec![0x4B];
        reply.extend(redraw(b"QUEUED-OLD-JK"));
        radio.send(&reply, Delivery::Coalesced);
        radio.expect(0x4A);
        radio.port.write_all(&[0x4A]).unwrap();
    });
    assert_rejected(capture);
}

#[test]
fn redraw_before_start_ack_is_rejected_even_with_4a_and_4b_in_payload() {
    let capture = capture(usize::MAX, |radio| {
        radio.bootstrap();
        let mut reply = redraw(b"PRE-ACK-JK");
        reply.push(0x4A);
        radio.send(&reply, Delivery::Coalesced);
    });
    assert_rejected(capture);
}

#[test]
fn valid_post_ack_redraw_survives_header_and_text_dribbling_beyond_500ms() {
    let capture = capture(4, |radio| {
        radio.bootstrap();
        let bytes = redraw(b"146.520");
        // Coalesce the ACK with the first opcode, then make both a fixed header
        // and a text frame exceed 500ms total without a 500ms inactivity gap.
        radio.port.write_all(&[0x4A, bytes[0]]).unwrap();
        for byte in &bytes[1..7] {
            thread::sleep(Duration::from_millis(110));
            radio.port.write_all(&[*byte]).unwrap();
        }
        radio.port.write_all(&bytes[7..22]).unwrap();
        for byte in &bytes[22..30] {
            thread::sleep(Duration::from_millis(110));
            radio.port.write_all(&[*byte]).unwrap();
        }
        radio.port.write_all(&bytes[30..]).unwrap();
    });
    assert_capture(capture, &expected_redraw("146.520"));
}
