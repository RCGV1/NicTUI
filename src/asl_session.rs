//! Exclusive USB serial COS monitoring with zero UART command bytes.
//!
//! Opening a serial port can assert or clear modem pins through the OS/driver.
//! Use only a known HT USB serial connection, never a pin-controlled PTT adapter.

use std::fmt;
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serialport::{
    DataBits, FlowControl, Parity, SerialPort, SerialPortInfo, SerialPortType, StopBits,
};

use crate::asl::{CosDecoder, CosReport, CosSnapshot, CosState, UnexpectedByte};
use crate::cancellation;

const ASL_BAUD: u32 = 38_400;
const READ_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Debug, Clone)]
pub struct AslMonitorOptions {
    /// Finite capture budget after the port is opened and configured.
    pub duration: Duration,
    pub stop_signal: Option<Arc<AtomicBool>>,
}

impl Default for AslMonitorOptions {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(30),
            stop_signal: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AslMonitorEvent {
    /// The OS port opened, not a firmware handshake or health confirmation.
    PortOpened {
        at: SystemTime,
    },
    Cos(CosReport),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AslStopReason {
    DurationElapsed,
    Cancelled,
    Fault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AslMonitorSummary {
    pub cos: CosSnapshot,
    pub elapsed: Duration,
    pub reason: AslStopReason,
}

#[derive(Debug)]
pub enum AslFailureKind {
    Setup(String),
    Read(io::Error),
    UnexpectedByte(UnexpectedByte),
}

impl fmt::Display for AslFailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setup(detail) => write!(f, "ASL setup failed: {detail}"),
            Self::Read(error) => write!(f, "ASL receive failed: {error}"),
            Self::UnexpectedByte(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AslFailureKind {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Setup(_) => None,
            Self::Read(error) => Some(error),
            Self::UnexpectedByte(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub struct AslMonitorError {
    pub kind: AslFailureKind,
    pub partial: AslMonitorSummary,
}

impl fmt::Display for AslMonitorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for AslMonitorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.kind)
    }
}

fn stopped(options: &AslMonitorOptions) -> bool {
    cancellation::is_requested()
        || options
            .stop_signal
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::SeqCst))
}

fn empty_summary(reason: AslStopReason) -> AslMonitorSummary {
    AslMonitorSummary {
        cos: CosSnapshot::default(),
        elapsed: Duration::ZERO,
        reason,
    }
}

fn setup_error(detail: impl Into<String>) -> AslMonitorError {
    AslMonitorError {
        kind: AslFailureKind::Setup(detail.into()),
        partial: empty_summary(AslStopReason::Fault),
    }
}

fn require_usb(port: &str, inventory: &[SerialPortInfo]) -> Result<(), AslMonitorError> {
    if inventory
        .iter()
        .any(|info| info.port_name == port && matches!(info.port_type, SerialPortType::UsbPort(_)))
    {
        Ok(())
    } else {
        Err(setup_error(
            "An explicitly selected, enumerated wired USB serial port is required; unknown, Bluetooth, and virtual ports are not accepted",
        ))
    }
}

/// Listen at 38400/8N1 with zero UART command bytes: no handshake, application
/// writes, buffer-clear requests, PTT commands, or RTS/DTR method calls.
/// ASL USB must be set manually; Activity and Remote must not share this stream.
/// Silence leaves COS Unknown and is not proof of health or disconnection.
///
/// Opening the port may assert or clear RTS/DTR through the OS/driver despite
/// making no explicit pin-control requests. This is not an electrically read-only
/// guarantee. Use only a known HT USB serial connection, not a pin-controlled PTT
/// adapter; USB metadata alone cannot distinguish those wiring arrangements.
///
/// `report` must be nonblocking. Read polling is bounded by 50ms (or the remaining
/// duration); OS setup, scheduling, and caller callbacks are not hard-real-time.
pub fn monitor_asl(
    port: &str,
    options: &AslMonitorOptions,
    report: impl FnMut(AslMonitorEvent),
) -> Result<AslMonitorSummary, AslMonitorError> {
    if options.duration.is_zero() {
        return Err(setup_error("Capture duration must be nonzero"));
    }
    if stopped(options) {
        return Ok(empty_summary(AslStopReason::Cancelled));
    }
    if port.is_empty() || port.trim() != port || port.to_ascii_lowercase().starts_with("ble:") {
        return Err(setup_error("Select an explicit wired USB serial port"));
    }
    let inventory =
        serialport::available_ports().map_err(|error| setup_error(error.to_string()))?;
    require_usb(port, &inventory)?;
    if stopped(options) {
        return Ok(empty_summary(AslStopReason::Cancelled));
    }
    let mut serial = open_native_passive(port).map_err(|error| setup_error(error.to_string()))?;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    serial
        .set_baud_rate(ASL_BAUD)
        .map_err(|error| setup_error(error.to_string()))?;
    receive_asl(
        |timeout| read_serial_byte(&mut serial, timeout),
        options,
        report,
    )
}

#[cfg(unix)]
fn open_native_passive(port: &str) -> serialport::Result<serialport::TTYPort> {
    let serial = passive_builder(port).open_native()?;
    if !serial.exclusive() {
        return Err(serialport::Error::new(
            serialport::ErrorKind::Unknown,
            "Exclusive serial ownership was not acquired",
        ));
    }
    Ok(serial)
}

#[cfg(windows)]
fn open_native_passive(port: &str) -> serialport::Result<serialport::COMPort> {
    // COMPort opens with share mode zero (exclusive).
    passive_builder(port).open_native()
}

fn passive_builder(port: &str) -> serialport::SerialPortBuilder {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let baud = 0; // serialport 4.8 clears macOS buffers on open if baud > 0.
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let baud = ASL_BAUD;
    serialport::new(port, baud)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .timeout(READ_TIMEOUT)
}

fn read_serial_byte(port: &mut dyn SerialPort, timeout: Duration) -> io::Result<u8> {
    port.set_timeout(timeout).map_err(io::Error::from)?;
    let mut byte = [0];
    match port.read(&mut byte)? {
        0 => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "ASL serial port reached EOF",
        )),
        _ => Ok(byte[0]),
    }
}

fn receive_asl(
    mut read_byte: impl FnMut(Duration) -> io::Result<u8>,
    options: &AslMonitorOptions,
    mut report: impl FnMut(AslMonitorEvent),
) -> Result<AslMonitorSummary, AslMonitorError> {
    let started = Instant::now();
    let mut decoder = CosDecoder::new();
    if !stopped(options) {
        report(AslMonitorEvent::PortOpened {
            at: SystemTime::now(),
        });
    }
    loop {
        let reason = if stopped(options) {
            Some(AslStopReason::Cancelled)
        } else if started.elapsed() >= options.duration {
            Some(AslStopReason::DurationElapsed)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(AslMonitorSummary {
                cos: decoder.snapshot(),
                elapsed: started.elapsed(),
                reason,
            });
        }
        let timeout = READ_TIMEOUT.min(options.duration.saturating_sub(started.elapsed()));
        if timeout < Duration::from_millis(1) {
            // Windows truncates serial timeouts to milliseconds; avoid zero on conversion.
            if !timeout.is_zero() {
                std::thread::sleep(timeout);
            }
            continue;
        }
        let failure = match read_byte(timeout) {
            Ok(byte) => match decoder.feed(byte, SystemTime::now()) {
                Ok(Some(cos)) => {
                    report(AslMonitorEvent::Cos(cos));
                    None
                }
                Ok(None) => None,
                Err(error) => Some(AslFailureKind::UnexpectedByte(error)),
            },
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) =>
            {
                // Avoid spinning on transports returning idle/error immediately.
                let pause = Duration::from_millis(1)
                    .min(options.duration.saturating_sub(started.elapsed()));
                if !stopped(options) && !pause.is_zero() {
                    std::thread::sleep(pause);
                }
                None
            }
            Err(error) => Some(AslFailureKind::Read(error)),
        };
        if let Some(kind) = failure {
            let mut cos = decoder.snapshot();
            cos.state = CosState::Unknown;
            return Err(AslMonitorError {
                kind,
                partial: AslMonitorSummary {
                    cos,
                    elapsed: started.elapsed(),
                    reason: AslStopReason::Fault,
                },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::error::Error;

    fn options(duration: Duration) -> AslMonitorOptions {
        AslMonitorOptions {
            duration,
            stop_signal: None,
        }
    }

    #[test]
    fn only_exact_enumerated_usb_metadata_is_accepted() {
        let usb = SerialPortInfo {
            port_name: "selected-usb".into(),
            port_type: SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid: 0x1234,
                pid: 0x5678,
                serial_number: None,
                manufacturer: None,
                product: None,
            }),
        };
        assert!(require_usb("selected-usb", std::slice::from_ref(&usb)).is_ok());
        assert!(require_usb("another-port", std::slice::from_ref(&usb)).is_err());
        for port_type in [
            SerialPortType::Unknown,
            SerialPortType::BluetoothPort,
            SerialPortType::PciPort,
        ] {
            let info = SerialPortInfo {
                port_name: usb.port_name.clone(),
                port_type,
            };
            let failure = require_usb("selected-usb", &[info]).unwrap_err();
            assert_eq!(failure.partial.cos.state, CosState::Unknown);
            assert_eq!(failure.partial.reason, AslStopReason::Fault);
        }
        assert!(require_usb("selected-usb", &[]).is_err());
    }

    #[test]
    fn submillisecond_duration_expires_unknown_without_reading() {
        for duration in [
            Duration::from_nanos(1),
            Duration::from_micros(500),
            Duration::from_nanos(999_999),
        ] {
            let mut events = Vec::new();
            let summary = receive_asl(
                |_| panic!("Sub-millisecond capture must never call the reader"),
                &options(duration),
                |event| events.push(event),
            )
            .unwrap();
            assert_eq!(summary.reason, AslStopReason::DurationElapsed);
            assert_eq!(summary.cos, CosSnapshot::default());
            assert!(summary.elapsed >= duration);
            assert_eq!(events.len(), 1);
            assert!(matches!(events[0], AslMonitorEvent::PortOpened { .. }));
        }
    }

    #[test]
    fn silent_timeout_and_nop_streams_expire_unknown_with_bounded_reads() {
        for nop in [false, true] {
            let mut events = Vec::new();
            let mut timeouts = Vec::new();
            let limit = Duration::from_millis(65);
            let summary = receive_asl(
                |timeout| {
                    assert!(timeout >= Duration::from_millis(1) && timeout <= READ_TIMEOUT);
                    assert!(timeout <= limit);
                    timeouts.push(timeout);
                    std::thread::sleep(timeout);
                    if nop {
                        Ok(0)
                    } else {
                        Err(io::ErrorKind::TimedOut.into())
                    }
                },
                &options(limit),
                |event| events.push(event),
            )
            .unwrap();
            assert_eq!(summary.reason, AslStopReason::DurationElapsed);
            assert_eq!(summary.cos, CosSnapshot::default());
            assert!(summary.elapsed >= limit && summary.elapsed < Duration::from_secs(1));
            assert_eq!(events.len(), 1);
            assert!(matches!(events[0], AslMonitorEvent::PortOpened { .. }));
            assert!(timeouts.len() >= 2);
            assert!(timeouts[1] < READ_TIMEOUT);
        }
    }

    #[test]
    fn callback_cancellation_is_checked_before_next_read() {
        let stop = Arc::new(AtomicBool::new(false));
        let options = AslMonitorOptions {
            duration: Duration::from_secs(30),
            stop_signal: Some(stop.clone()),
        };
        let mut reads = 0;
        let summary = receive_asl(
            |_| {
                reads += 1;
                Ok(0x11)
            },
            &options,
            |event| {
                if matches!(event, AslMonitorEvent::Cos(_)) {
                    stop.store(true, Ordering::SeqCst);
                }
            },
        )
        .unwrap();
        assert_eq!(reads, 1);
        assert_eq!(summary.reason, AslStopReason::Cancelled);
        assert_eq!(summary.cos.open_reports, 1);
    }

    #[test]
    fn cancellation_during_idle_read_finishes_within_one_poll() {
        let stop = Arc::new(AtomicBool::new(false));
        let options = AslMonitorOptions {
            duration: Duration::from_secs(30),
            stop_signal: Some(stop.clone()),
        };
        let worker = std::thread::spawn(move || {
            receive_asl(
                |timeout| {
                    stop.store(true, Ordering::SeqCst);
                    std::thread::sleep(timeout);
                    Err(io::ErrorKind::TimedOut.into())
                },
                &options,
                |_| {},
            )
            .unwrap()
        });
        let summary = worker.join().unwrap();
        assert_eq!(summary.reason, AslStopReason::Cancelled);
        assert_eq!(summary.cos.state, CosState::Unknown);
        assert!(summary.elapsed < Duration::from_millis(500));
    }

    #[test]
    fn read_faults_invalidate_state_and_keep_observed_counts_and_error_chain() {
        for kind in [
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::NotConnected,
        ] {
            let mut input = VecDeque::from([Ok(0x11), Ok(0x10), Err(io::Error::from(kind))]);
            let mut events = Vec::new();
            let error = receive_asl(
                |_| input.pop_front().unwrap(),
                &AslMonitorOptions::default(),
                |event| events.push(event),
            )
            .unwrap_err();
            assert_eq!(error.partial.reason, AslStopReason::Fault);
            assert_eq!(error.partial.cos.state, CosState::Unknown);
            assert_eq!(error.partial.cos.open_reports, 1);
            assert_eq!(error.partial.cos.closed_reports, 1);
            assert_eq!(error.partial.cos.transitions, 1);
            assert!(error.partial.cos.last_transition_at.is_some());
            assert_eq!(events.len(), 3);
            assert!(matches!(&error.kind, AslFailureKind::Read(cause) if cause.kind() == kind));
            let cause = error
                .source()
                .unwrap()
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap();
            assert_eq!(cause.kind(), kind);
            assert!(error.to_string().contains("receive failed"));
        }
    }

    #[test]
    fn activity_remote_and_other_payloads_stop_before_embedded_cos_is_read() {
        for marker in [0x2A, 0x2B, 0x64, 0x4A, 0xFF] {
            let mut input = VecDeque::from([0, 0x11, marker, 0x10, 0x11]);
            let mut events = Vec::new();
            let error = receive_asl(
                |_| Ok(input.pop_front().unwrap()),
                &AslMonitorOptions::default(),
                |event| events.push(event),
            )
            .unwrap_err();
            assert!(
                matches!(error.kind, AslFailureKind::UnexpectedByte(UnexpectedByte { byte, offset: 2 }) if byte == marker)
            );
            assert_eq!(input, VecDeque::from([0x10, 0x11]));
            assert_eq!(events.len(), 2);
            assert_eq!(error.partial.cos.state, CosState::Unknown);
            assert_eq!(error.partial.cos.open_reports, 1);
            assert_eq!(error.partial.cos.closed_reports, 0);
            assert_eq!(error.partial.cos.transitions, 0);
        }
    }

    #[test]
    fn retryable_read_errors_do_not_reset_observations() {
        let stop = Arc::new(AtomicBool::new(false));
        let mut input = VecDeque::from([
            Ok(0x10),
            Err(io::ErrorKind::Interrupted.into()),
            Err(io::ErrorKind::WouldBlock.into()),
            Err(io::ErrorKind::TimedOut.into()),
            Ok(0x11),
        ]);
        let options = AslMonitorOptions {
            stop_signal: Some(stop.clone()),
            ..AslMonitorOptions::default()
        };
        let summary = receive_asl(
            |_| input.pop_front().unwrap(),
            &options,
            |event| {
                if matches!(
                    event,
                    AslMonitorEvent::Cos(CosReport {
                        state: CosState::Open,
                        ..
                    })
                ) {
                    stop.store(true, Ordering::SeqCst);
                }
            },
        )
        .unwrap();
        assert_eq!(summary.cos.state, CosState::Open);
        assert_eq!(summary.cos.transitions, 1);
    }

    #[cfg(unix)]
    mod native {
        use super::*;
        use serialport::TTYPort;
        use std::io::{Read, Write};

        fn assert_zero_tx(master: &mut TTYPort) {
            master.set_timeout(Duration::from_millis(15)).unwrap();
            let mut bytes = [0; 64];
            let error = master
                .read(&mut bytes)
                .expect_err("ASL must never transmit, even during cleanup");
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }

        #[test]
        fn queued_cos_survives_native_open_and_cancel_without_any_tx() {
            let (mut master, slave) = TTYPort::pair().unwrap();
            master.write_all(&[0, 0x11, 0x10]).unwrap();
            let name = slave.name().unwrap();
            // PTYs have no USB provenance and cannot set custom baud on macOS.
            // Exercise the private opener/read loop, never bypass the public guard.
            let mut reader = open_native_passive(&name).unwrap();
            assert!(reader.exclusive());
            assert!(
                open_native_passive(&name).is_err(),
                "second exclusive open must fail"
            );
            assert_eq!(reader.data_bits().unwrap(), DataBits::Eight);
            assert_eq!(reader.parity().unwrap(), Parity::None);
            assert_eq!(reader.stop_bits().unwrap(), StopBits::One);
            assert_eq!(reader.flow_control().unwrap(), FlowControl::None);
            let stop = Arc::new(AtomicBool::new(false));
            let options = AslMonitorOptions {
                stop_signal: Some(stop.clone()),
                ..AslMonitorOptions::default()
            };
            let summary = receive_asl(
                |timeout| read_serial_byte(&mut reader, timeout),
                &options,
                |event| {
                    if matches!(
                        event,
                        AslMonitorEvent::Cos(CosReport {
                            state: CosState::Closed,
                            ..
                        })
                    ) {
                        stop.store(true, Ordering::SeqCst);
                    }
                },
            )
            .unwrap();
            assert_eq!(summary.reason, AslStopReason::Cancelled);
            assert_eq!(summary.cos.transitions, 1);
            assert_zero_tx(&mut master);
            drop(reader);
            let reopened = open_native_passive(&name).unwrap();
            assert!(reopened.exclusive());
            assert_zero_tx(&mut master);
        }

        #[test]
        fn protocol_fault_and_silent_duration_keep_native_wire_quiet() {
            for bytes in [&[0x11, 0x2A, 0x10, 0x11][..], &[][..]] {
                let (mut master, slave) = TTYPort::pair().unwrap();
                let mut reader = open_native_passive(&slave.name().unwrap()).unwrap();
                master.write_all(bytes).unwrap();
                let options = options(Duration::from_millis(30));
                let result = receive_asl(
                    |timeout| read_serial_byte(&mut reader, timeout),
                    &options,
                    |_| {},
                );
                if bytes.is_empty() {
                    let summary = result.unwrap();
                    assert_eq!(summary.reason, AslStopReason::DurationElapsed);
                    assert_eq!(summary.cos, CosSnapshot::default());
                } else {
                    let error = result.unwrap_err();
                    assert_eq!(error.partial.cos.state, CosState::Unknown);
                    assert_eq!(error.partial.cos.open_reports, 1);
                    assert_eq!(read_serial_byte(&mut reader, READ_TIMEOUT).unwrap(), 0x10);
                }
                assert_zero_tx(&mut master);
                drop(reader);
                assert_zero_tx(&mut master);
            }
        }

        #[test]
        fn native_peer_disconnect_is_a_bounded_fault() {
            let (master, slave) = TTYPort::pair().unwrap();
            let mut reader = open_native_passive(&slave.name().unwrap()).unwrap();
            drop(master);
            let started = Instant::now();
            let error = receive_asl(
                |timeout| read_serial_byte(&mut reader, timeout),
                &options(Duration::from_secs(2)),
                |_| {},
            )
            .unwrap_err();
            assert!(matches!(error.kind, AslFailureKind::Read(_)));
            assert_eq!(error.partial.cos.state, CosState::Unknown);
            assert!(started.elapsed() < Duration::from_secs(1));
        }
    }
}
