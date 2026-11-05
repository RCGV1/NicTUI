use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, SystemTime};

use nictui::asl::{CosDecoder, CosSnapshot, CosState};
use nictui::asl_session::{AslFailureKind, AslMonitorOptions, AslStopReason, monitor_asl};

#[test]
fn finite_default_and_pre_cancel_never_open_or_report() {
    let mut options = AslMonitorOptions::default();
    assert_eq!(options.duration, Duration::from_secs(30));
    assert!(options.stop_signal.is_none());
    options.stop_signal = Some(Arc::new(AtomicBool::new(true)));
    let summary = monitor_asl("not-a-real-device", &options, |_| {
        panic!("pre-cancel must not open")
    })
    .unwrap();
    assert_eq!(summary.reason, AslStopReason::Cancelled);
    assert_eq!(summary.elapsed, Duration::ZERO);
    assert_eq!(summary.cos, CosSnapshot::default());
    assert_eq!(
        serde_json::to_value(summary).unwrap()["reason"],
        "cancelled"
    );
}

#[test]
fn zero_duration_and_ble_targets_are_rejected_before_open() {
    let options = AslMonitorOptions {
        duration: Duration::ZERO,
        stop_signal: None,
    };
    let error = monitor_asl("not-a-real-device", &options, |_| {
        panic!("invalid setup must not open")
    })
    .unwrap_err();
    assert!(matches!(error.kind, AslFailureKind::Setup(_)));
    assert_eq!(error.partial.reason, AslStopReason::Fault);
    for port in ["", " ", "ble://radio", "BLE:radio", " selected-port "] {
        let error = monitor_asl(port, &AslMonitorOptions::default(), |_| {
            panic!("invalid setup must not open")
        })
        .unwrap_err();
        assert_eq!(error.partial.cos.state, CosState::Unknown);
        assert!(matches!(error.kind, AslFailureKind::Setup(_)));
    }
}

#[test]
fn replay_uses_the_same_strict_latched_decoder() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    let mut decoder = CosDecoder::new();
    let mut reports = Vec::new();
    for byte in [0, 0x11, 0x11, 0x10] {
        if let Some(report) = decoder.feed(byte, at).unwrap() {
            reports.push(report);
        }
    }
    assert_eq!(reports.len(), 3);
    assert!(!reports[0].transitioned);
    assert!(!reports[1].transitioned);
    assert!(reports[2].transitioned);
    let error = decoder.feed(0x64, at).unwrap_err();
    assert_eq!(error.offset, 4);
    assert_eq!(decoder.feed(0x11, at), Err(error));
    let snapshot = decoder.snapshot();
    assert_eq!(snapshot.state, CosState::Unknown);
    assert_eq!(snapshot.open_reports, 2);
    assert_eq!(snapshot.closed_reports, 1);
    assert_eq!(snapshot.transitions, 1);
    assert_eq!(snapshot.last_transition_at, Some(at));
}

#[cfg(unix)]
#[test]
fn public_monitor_rejects_virtual_pty_without_sending_a_byte() {
    use serialport::{SerialPort, TTYPort};
    use std::io::Read;

    let (mut master, slave) = TTYPort::pair().unwrap();
    let error = monitor_asl(
        &slave.name().unwrap(),
        &AslMonitorOptions::default(),
        |_| panic!("virtual port must not open"),
    )
    .unwrap_err();
    assert!(matches!(error.kind, AslFailureKind::Setup(_)));
    master.set_timeout(Duration::from_millis(20)).unwrap();
    let mut byte = [0];
    assert_eq!(
        master.read(&mut byte).unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
}
