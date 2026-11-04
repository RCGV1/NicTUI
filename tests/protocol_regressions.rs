#![cfg(unix)]

use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nictui::protocol::{RadioProtocol, RemotePacket};
use serialport::{SerialPort, TTYPort};

fn with_remote_stream<T>(
    fragments: Vec<(Duration, Vec<u8>)>,
    check: impl FnOnce(&mut RadioProtocol) -> T,
) -> T {
    let (mut radio, slave) = TTYPort::pair().unwrap();
    let name = slave.name().unwrap();
    drop(slave);
    let mut protocol = RadioProtocol::new(&name).unwrap();
    radio.set_timeout(Duration::from_secs(3)).unwrap();
    let (shutdown, stopped) = mpsc::channel();
    let worker = thread::spawn(move || {
        // Wait until the reader is open before publishing any bytes.
        let mut ready = [0];
        radio.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [0x01]);
        for (delay, bytes) in fragments {
            thread::sleep(delay);
            radio.write_all(&bytes).unwrap();
        }
        let _ = stopped.recv_timeout(Duration::from_secs(3));
    });
    protocol.send_bytes(&[0x01]).unwrap();
    let result = check(&mut protocol);
    let _ = shutdown.send(());
    worker.join().unwrap();
    result
}

#[test]
fn remote_fields_allow_programmer_inactivity_budget_and_preserve_next_packet() {
    let frames = [
        vec![0x64, 0x02, 0x03, 0x04, 0x34, 0x12, 0x78, 0x56, b'A', 0],
        vec![0x65, 0x03, 0x04, 0x05, 0x06, 0x34, 0x12],
        vec![0x66, 0x02, 0x03, 0x04, 0x34, 0x12, 0x78, 0x56],
        vec![0x67, 0x2A, 0x01],
        vec![0x68, 0x2A, 0x01],
        vec![0x69, 0x2A],
    ];
    for frame in frames {
        let opcode = frame[0];
        let fragments = vec![
            (Duration::ZERO, vec![opcode]),
            (Duration::from_millis(175), frame[1..].to_vec()),
            (Duration::ZERO, vec![0x72]),
        ];
        let (packet, next) = with_remote_stream(fragments, |protocol| {
            let packet = protocol.parse_remote_packet();
            let next = protocol.parse_remote_packet();
            (packet, next)
        });
        let packet = packet.unwrap().unwrap();
        assert!(match opcode {
            0x64 =>
                matches!(packet, RemotePacket::DisplayText { text, fg_color: 0x1234, .. } if text == "A"),
            0x65 => matches!(
                packet,
                RemotePacket::DrawRectangle {
                    width: 5,
                    color: 0x1234,
                    ..
                }
            ),
            0x66 => matches!(
                packet,
                RemotePacket::DrawSymbol {
                    symbol_id: 2,
                    bg_color: 0x5678,
                    ..
                }
            ),
            0x67 => matches!(
                packet,
                RemotePacket::SignalStrength {
                    strength: 42,
                    mode: 1,
                    ..
                }
            ),
            0x68 => matches!(packet, RemotePacket::NoiseLevel { level: 42, mode: 1 }),
            0x69 => matches!(packet, RemotePacket::SignalBarPos { y: 42, .. }),
            _ => unreachable!(),
        });
        assert!(matches!(
            next.unwrap(),
            Some(RemotePacket::SmallStatus { id: 0x72, .. })
        ));
    }
}

#[test]
fn remote_field_timeout_resets_on_progress_not_at_the_start_of_the_field() {
    let mut fragments = vec![(Duration::ZERO, vec![0x65])];
    for byte in [3, 4, 5, 6, 0x34, 0x12] {
        fragments.push((Duration::from_millis(150), vec![byte]));
    }
    let packet = with_remote_stream(fragments, |protocol| protocol.parse_remote_packet());
    assert!(matches!(
        packet.unwrap(),
        Some(RemotePacket::DrawRectangle { color: 0x1234, .. })
    ));
}

#[test]
fn remote_text_allows_a_fragment_gap_after_the_header() {
    let packet = with_remote_stream(
        vec![
            (Duration::ZERO, vec![0x64, 2, 3, 4, 0x34, 0x12, 0x78, 0x56]),
            (Duration::from_millis(175), vec![b'A']),
            (Duration::from_millis(175), vec![b'@', b'B', 0]),
        ],
        |protocol| protocol.parse_remote_packet(),
    );
    assert!(matches!(
        packet.unwrap(),
        Some(RemotePacket::DisplayText { text, .. }) if text == "A B"
    ));
}

#[test]
fn remote_partial_field_still_times_out_on_inactivity() {
    let started = Instant::now();
    let result = with_remote_stream(vec![(Duration::ZERO, vec![0x65, 3])], |protocol| {
        protocol.parse_remote_packet()
    });
    assert!(result.unwrap_err().to_string().contains("got 1/6 bytes"));
    assert!(started.elapsed() >= Duration::from_millis(500));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn fixed_response_deadlines_do_not_reset_on_partial_progress() {
    let result = with_remote_stream(
        vec![
            (Duration::ZERO, vec![0xAA]),
            (Duration::from_millis(175), vec![0xBB]),
        ],
        |protocol| protocol.recv(2, Duration::from_millis(100)),
    );
    assert!(result.unwrap_err().to_string().contains("got 1/2 bytes"));
}
