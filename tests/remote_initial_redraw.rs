#![cfg(unix)]

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use nictui::app::{App, AppEvent};
use nictui::protocol::RemotePacket;
use nictui::remote::{
    RemoteCaptureEvent, RemoteSessionOptions, RemoteSessionPhase, run_remote_session,
};
use nictui::remote_display::RadioDisplay;
use ratatui::style::Color;
use serialport::{SerialPort, TTYPort};

#[derive(Clone, Copy)]
enum Delivery {
    Coalesced,
    Fragmented,
}

// Independent wire bytes and decoded expectations catch field loss/alignment errors.
fn redraw_bytes() -> Vec<u8> {
    vec![
        0x65, 0, 0, 128, 128, 0, 0x80, // Full blue LCD background.
        0x65, 0, 12, 128, 20, 0x4A, 0x4B, // ACK bytes inside a color are data.
        0x64, 0, 4, 16, 0xFF, 0xFF, 0x4A, 0x4B, b'C', b'H', b'0', b'0', b'1', b'@', b'R', b'X', 0,
        0, 0, // NOPs between complete frames must not swallow the next opcode.
        0x64, 2, 4, 40, 0xFF, 0xFF, 0, 0x80, b'1', b'4', b'6', b'.', b'5', b'2', b'0', 0, 0x66, 0,
        0, 0, 0xFF, 7, 0, 0x80, // Symbol zero at zero coordinates.
        0x69, 104, // Signal bar position has exactly one payload byte.
        0x67, 32, 0, // Signal strength has no battery/padding byte.
        0x68, 12, 0, 0x72, // Packed status has no payload bytes.
        0x64, 0, 96, 120, 0xFF, 0xFF, 0, 0x80, b'7', b'.', b'2', b'V', 0,
    ]
}

fn redraw_packets() -> Vec<RemotePacket> {
    vec![
        RemotePacket::DrawRectangle {
            x: 0,
            y: 0,
            width: 128,
            height: 128,
            color: 0x8000,
        },
        RemotePacket::DrawRectangle {
            x: 0,
            y: 12,
            width: 128,
            height: 20,
            color: 0x4B4A,
        },
        RemotePacket::DisplayText {
            font_size: 0,
            x: 4,
            y: 16,
            fg_color: 0xFFFF,
            bg_color: 0x4B4A,
            text: "CH001 RX".to_string(),
        },
        RemotePacket::DisplayText {
            font_size: 2,
            x: 4,
            y: 40,
            fg_color: 0xFFFF,
            bg_color: 0x8000,
            text: "146.520".to_string(),
        },
        RemotePacket::DrawSymbol {
            symbol_id: 0,
            x: 0,
            y: 0,
            fg_color: 0x07FF,
            bg_color: 0x8000,
        },
        RemotePacket::SignalBarPos { y: 104, aux: 0 },
        RemotePacket::SignalStrength {
            strength: 32,
            mode: 0,
            battery: 0,
        },
        RemotePacket::NoiseLevel { level: 12, mode: 0 },
        RemotePacket::SmallStatus {
            id: 0x72,
            value1: 0,
            value2: 0,
        },
        RemotePacket::DisplayText {
            font_size: 0,
            x: 96,
            y: 120,
            fg_color: 0xFFFF,
            bg_color: 0x8000,
            text: "7.2V".to_string(),
        },
    ]
}

fn expect_byte(radio: &mut TTYPort, trace: &mut Vec<u8>, expected: u8) {
    let started = Instant::now();
    loop {
        let mut byte = [0];
        match radio.read(&mut byte) {
            Ok(1) => {
                trace.push(byte[0]);
                assert_eq!(
                    byte[0], expected,
                    "unexpected host write; trace: {trace:02X?}"
                );
                return;
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            other => panic!("PTY read failed: {other:?}; trace: {trace:02X?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "missing host byte {expected:02X}; trace: {trace:02X?}"
        );
    }
}

fn fake_radio(mut radio: TTYPort, delivery: Delivery) -> Vec<u8> {
    let mut trace = Vec::new();
    for byte in [0x01; 10].into_iter().chain([0x4B]) {
        expect_byte(&mut radio, &mut trace, byte);
        radio.write_all(&[byte]).unwrap();
    }
    expect_byte(&mut radio, &mut trace, 0x4A);
    let mut reply = vec![0x4A];
    reply.extend(redraw_bytes());
    match delivery {
        Delivery::Coalesced => radio.write_all(&reply).unwrap(),
        Delivery::Fragmented => {
            // ACK plus a partial rectangle arrive together; split every remaining
            // field, color word, text character and terminator across writes.
            radio.write_all(&reply[..3]).unwrap();
            for byte in &reply[3..] {
                thread::sleep(Duration::from_millis(2));
                radio.write_all(&[*byte]).unwrap();
            }
        }
    }
    expect_byte(&mut radio, &mut trace, 0x4B);
    radio.write_all(&[0x4B]).unwrap();
    let mut byte = [0];
    let error = radio
        .read(&mut byte)
        .expect_err("host wrote after shutdown");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    trace
}

fn app_event(event: &RemoteCaptureEvent) -> AppEvent {
    match event {
        RemoteCaptureEvent::Received(at) => AppEvent::RemoteReceived(*at),
        RemoteCaptureEvent::Status(message) => AppEvent::Status(message.clone()),
        RemoteCaptureEvent::Log(message) => AppEvent::Log(message.clone()),
        RemoteCaptureEvent::Phase(phase) => AppEvent::RemotePhase(*phase),
        RemoteCaptureEvent::Control(report) => AppEvent::RemoteControl(report.clone()),
        RemoteCaptureEvent::Packet(packet) => AppEvent::RemotePacket(packet.clone()),
        RemoteCaptureEvent::Delta(delta) => AppEvent::RemoteDelta(delta.clone()),
    }
}

fn packet_details<'a>(packets: impl IntoIterator<Item = &'a RemotePacket>) -> Vec<String> {
    // RemotePacket does not implement PartialEq; Debug includes every field.
    packets
        .into_iter()
        .map(|packet| format!("{packet:?}"))
        .collect()
}

fn assert_app_redraw(events: &[RemoteCaptureEvent], expected: &[RemotePacket], batched: bool) {
    // Demo construction is hardware-free and supplies stale pixels/telemetry and
    // a prior confirmed control report, all of which Opening must replace.
    let mut app = App::demo();
    app.logs.clear();
    app.remote_screen.show_diagnostics = true;
    assert!(app.remote_screen.last_control_report.is_some());
    for event in events {
        app.event_tx.send(app_event(event)).unwrap();
        if !batched {
            assert!(app.update());
            if matches!(
                event,
                RemoteCaptureEvent::Phase(RemoteSessionPhase::Opening)
            ) {
                assert!(!app.remote_screen.has_display_updates);
                assert!(app.remote_screen.elements.is_empty());
                assert!(app.remote_screen.last_control_report.is_none());
                assert!(app.remote_screen.last_received.is_none());
            }
        }
    }
    if batched {
        assert!(app.update());
    }

    let screen = &app.remote_screen;
    assert!(screen.show_diagnostics);
    assert!(screen.has_display_updates);
    assert!(screen.last_control_report.is_none());
    assert!(screen.last_failure.is_none());
    assert!(screen.last_delta.is_none());
    assert_eq!(screen.unknown_packet_count, 0);
    assert_eq!(screen.signal_strength, 32);
    assert_eq!(screen.noise_level, 12);
    assert_eq!(screen.leds, 2);
    assert_eq!(screen.last_small_status, Some((0x72, 0, 0)));
    assert_eq!(screen.battery_text.as_deref(), Some("7.2V"));
    assert_eq!(screen.battery_level, None);
    for timestamp in [
        screen.last_received,
        screen.last_signal_update,
        screen.last_noise_update,
        screen.last_text_update,
        screen.last_status_update,
        screen.last_led_update,
        screen.last_battery_update,
    ] {
        assert!(timestamp.is_some());
    }
    assert_eq!(
        packet_details(&screen.elements),
        packet_details(expected.iter().filter(|packet| !matches!(
            packet,
            RemotePacket::SignalStrength { .. } | RemotePacket::NoiseLevel { .. }
        )))
    );

    let mut reference = RadioDisplay::default();
    for packet in expected {
        reference.apply(packet);
    }
    for y in 0..128 {
        for x in 0..128 {
            assert_eq!(
                screen.display.pixel(x, y),
                reference.pixel(x, y),
                "({x},{y})"
            );
        }
    }
    assert_eq!(screen.display.pixel(127, 127), Color::Rgb(0, 0, 128));
    assert_eq!(screen.display.pixel(127, 12), Color::Rgb(80, 104, 72));
    assert_eq!(screen.display.pixel(4, 104), Color::Rgb(50, 205, 50));
    assert_eq!(screen.display.pixel(4, 110), Color::Rgb(90, 90, 255));
    assert!((0..8).any(|y| (0..8).any(|x| screen.display.pixel(x, y) == Color::Rgb(248, 252, 0))));
    assert!(!app.status_message.contains("confirmed"));
    assert!(!app.logs.iter().any(|log| log.contains("confirmed")));
}

fn initial_redraw(delivery: Delivery) {
    let (mut radio, _slave) = TTYPort::pair().expect("create isolated fake-radio PTY");
    radio.set_timeout(Duration::from_millis(25)).unwrap();
    let port = _slave.name().unwrap();
    let worker = thread::spawn(move || fake_radio(radio, delivery));
    let expected = redraw_packets();
    let packet_count = Cell::new(0);
    let mut events = Vec::new();
    let result = run_remote_session(
        &port,
        &RemoteSessionOptions::default(),
        |elapsed| packet_count.get() >= expected.len() || elapsed > Duration::from_secs(3),
        |_| None,
        |event| {
            if matches!(event, RemoteCaptureEvent::Packet(_)) {
                packet_count.set(packet_count.get() + 1);
            }
            events.push(event);
        },
    );
    let trace = worker.join().expect("fake-radio worker failed");
    let summary = result.unwrap();
    let mut allowed_trace = vec![0x01; 10];
    allowed_trace.extend([0x4B, 0x4A, 0x4B]);
    assert_eq!(
        trace, allowed_trace,
        "only bootstrap/start/stop allowed: no NAV, PTT, programmer or EEPROM writes"
    );
    assert_eq!(summary.packet_count, expected.len());
    assert_eq!(summary.unknown_packet_count, 0);
    assert_eq!(summary.recovery_count, 0);
    assert_eq!(
        packet_details(events.iter().filter_map(|event| match event {
            RemoteCaptureEvent::Packet(packet) => Some(packet),
            _ => None,
        })),
        packet_details(&expected)
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RemoteCaptureEvent::Control(_)))
    );
    let phases: Vec<_> = events
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
            RemoteSessionPhase::Stopped,
        ]
    );
    assert!(matches!(
        events.first(),
        Some(RemoteCaptureEvent::Phase(RemoteSessionPhase::Opening))
    ));
    let armed = events
        .iter()
        .position(|event| matches!(event, RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed)))
        .unwrap();
    let first_packet = events
        .iter()
        .position(|event| matches!(event, RemoteCaptureEvent::Packet(_)))
        .unwrap();
    assert!(armed < first_packet);
    let stopped = events
        .iter()
        .position(|event| {
            matches!(
                event,
                RemoteCaptureEvent::Phase(RemoteSessionPhase::Stopped)
            )
        })
        .unwrap();
    for batched in [false, true] {
        assert_app_redraw(&events[..stopped], &expected, batched);
        assert_app_redraw(&events, &expected, batched);
    }
}

#[test]
fn coalesced_start_ack_preserves_full_initial_redraw_without_control_writes() {
    initial_redraw(Delivery::Coalesced);
}

#[test]
fn fragmented_initial_redraw_survives_opening_reset_without_control_writes() {
    initial_redraw(Delivery::Fragmented);
}
