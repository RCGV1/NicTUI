use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::ble::parse_ble_device_uri;
use crate::protocol::{RadioProtocol, RemotePacket};

const NICFW_REMOTE_BAUD: u32 = 38_400;
const NICFW_REMOTE_SYNC_BYTE: u8 = 0x01;
const NICFW_REMOTE_SYNC_COUNT: usize = 10;
const NICFW_REMOTE_STOP: u8 = 0x4B;
const NICFW_REMOTE_START: u8 = 0x4A;
const NICFW_REMOTE_RELEASE: u8 = 0xFF;
const NICFW_REMOTE_ACK_TIMEOUT: Duration = Duration::from_millis(500);
const NICFW_REMOTE_IDLE_CADENCE: Duration = Duration::from_millis(500);
const NICFW_REMOTE_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(5000);
const BLE_REMOTE_SETTLE_DELAY: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoteSessionPhase {
    Opening,
    Armed,
    Probing,
    Live,
    Recovering,
    #[default]
    Stopped,
}

impl fmt::Display for RemoteSessionPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteSessionPhase::Opening => write!(f, "opening"),
            RemoteSessionPhase::Armed => write!(f, "armed"),
            RemoteSessionPhase::Probing => write!(f, "probing"),
            RemoteSessionPhase::Live => write!(f, "live"),
            RemoteSessionPhase::Recovering => write!(f, "recovering"),
            RemoteSessionPhase::Stopped => write!(f, "stopped"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoteEvidenceKind {
    ControlConfirmed,
    #[default]
    NoTelemetry,
    NoControlEvidence,
    CommandFailed,
}

impl fmt::Display for RemoteEvidenceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::ControlConfirmed => "control-confirmed",
            Self::NoTelemetry => "no-telemetry",
            Self::NoControlEvidence => "no-control-evidence",
            Self::CommandFailed => "command-failed",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSessionFailureKind {
    OpenFailed,
    BootstrapHandshakeFailed,
    RemoteOnAckFailed,
    StreamLost,
}

impl fmt::Display for RemoteSessionFailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::OpenFailed => "open-failed",
            Self::BootstrapHandshakeFailed => "bootstrap-handshake-failed",
            Self::RemoteOnAckFailed => "remote-on-ack-failed",
            Self::StreamLost => "stream-lost",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Clone)]
pub struct RemoteSessionFailure {
    pub kind: RemoteSessionFailureKind,
    pub summary: String,
    pub detail: String,
}

impl RemoteSessionFailure {
    fn new(
        kind: RemoteSessionFailureKind,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            summary: summary.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for RemoteSessionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.summary)
    }
}

impl std::error::Error for RemoteSessionFailure {}

pub type RemoteSessionResult<T> = std::result::Result<T, RemoteSessionFailure>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlStrategy {
    RawKey,
    Sequence,
}

impl fmt::Display for RemoteControlStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteControlStrategy::RawKey => write!(f, "raw-key"),
            RemoteControlStrategy::Sequence => write!(f, "sequence"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteWriteStep {
    pub bytes: Vec<u8>,
    pub pause_after: Duration,
}

#[derive(Debug, Clone)]
pub struct RemoteControlCommand {
    pub label: String,
    pub strategy: RemoteControlStrategy,
    pub steps: Vec<RemoteWriteStep>,
    steady_state_key: Option<u8>,
}

impl RemoteControlCommand {
    pub fn raw_key(label: impl Into<String>, key_code: u8) -> Self {
        let key_code = programmer_remote_key_code(key_code);
        Self {
            label: label.into(),
            strategy: RemoteControlStrategy::RawKey,
            steps: vec![
                RemoteWriteStep {
                    bytes: vec![key_code],
                    pause_after: Duration::from_millis(200),
                },
                RemoteWriteStep {
                    bytes: vec![NICFW_REMOTE_RELEASE],
                    pause_after: Duration::from_millis(20),
                },
            ],
            steady_state_key: Some(NICFW_REMOTE_RELEASE),
        }
    }

    pub fn sequence(
        label: impl Into<String>,
        bytes: Vec<u8>,
        gap: Duration,
        repeat: u32,
        hold: Duration,
    ) -> Self {
        let mut steps = Vec::new();
        let rounds = repeat.max(1);
        for round in 0..rounds {
            for byte in &bytes {
                steps.push(RemoteWriteStep {
                    bytes: vec![*byte],
                    pause_after: gap,
                });
            }
            if hold > Duration::ZERO {
                steps.push(RemoteWriteStep {
                    bytes: Vec::new(),
                    pause_after: hold,
                });
            }
            if round + 1 < rounds {
                steps.push(RemoteWriteStep {
                    bytes: Vec::new(),
                    pause_after: gap,
                });
            }
        }

        Self {
            label: label.into(),
            strategy: RemoteControlStrategy::Sequence,
            steps,
            steady_state_key: None,
        }
    }

    pub fn held_key(
        label: impl Into<String>,
        key_down: u8,
        _key_up: u8,
        gap: Duration,
        repeat: u32,
        hold: Duration,
    ) -> Self {
        let mut steps = Vec::new();
        let rounds = repeat.max(1);
        let hold = hold.max(Duration::from_millis(1));
        let key_down = programmer_remote_key_code(key_down);
        for round in 0..rounds {
            steps.push(RemoteWriteStep {
                bytes: vec![key_down],
                pause_after: hold,
            });
            steps.push(RemoteWriteStep {
                bytes: vec![NICFW_REMOTE_RELEASE],
                pause_after: gap,
            });
            if round + 1 < rounds {
                steps.push(RemoteWriteStep {
                    bytes: Vec::new(),
                    pause_after: gap,
                });
            }
        }

        Self {
            label: label.into(),
            strategy: RemoteControlStrategy::Sequence,
            steps,
            steady_state_key: Some(NICFW_REMOTE_RELEASE),
        }
    }

    pub fn burst(label: impl Into<String>, bytes: Vec<u8>, pause_after: Duration) -> Self {
        Self {
            label: label.into(),
            strategy: RemoteControlStrategy::Sequence,
            steps: vec![RemoteWriteStep { bytes, pause_after }],
            steady_state_key: None,
        }
    }

    pub fn bytes_hex(&self) -> String {
        let parts = self
            .steps
            .iter()
            .flat_map(|step| step.bytes.iter())
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            "<none>".to_string()
        } else {
            parts.join(" ")
        }
    }

    pub fn execute(&self, proto: &mut RadioProtocol) -> Result<()> {
        for step in &self.steps {
            if !step.bytes.is_empty() {
                proto.send_bytes(&step.bytes)?;
            }
            if step.pause_after > Duration::ZERO {
                thread::sleep(step.pause_after);
            }
        }
        Ok(())
    }

    pub fn estimated_duration(&self) -> Duration {
        self.steps
            .iter()
            .fold(Duration::ZERO, |total, step| total + step.pause_after)
    }
}

fn programmer_remote_key_code(key_code: u8) -> u8 {
    match key_code {
        0x01 => 0x80,
        0x02 => 0x81,
        0x03 => 0x82,
        0x04 => 0x83,
        0x05 => 0x84,
        0x06 => 0x85,
        0x07 => 0x86,
        0x08 => 0x87,
        0x09 => 0x88,
        0x0A => 0x89,
        0x0B => 0x8A,
        0x0C => 0x8B,
        0x0D => 0x8C,
        0x0E => 0x8D,
        0x0F => 0x8E,
        0x10 => 0x8F,
        0x11 => 0x91,
        0x12 => 0x92,
        0x13 => 0x90,
        0x1A => 0x91,
        0x80..=0xFF => key_code,
        _ => 0x80 | (key_code & 0x7F),
    }
}

#[derive(Debug, Clone)]
pub struct RemoteControlReport {
    pub label: String,
    pub strategy: RemoteControlStrategy,
    pub bytes_hex: String,
    pub success: bool,
    pub evidence: RemoteEvidenceKind,
    pub reaction: Option<RemoteCommandReaction>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub enum RemoteCaptureEvent {
    Status(String),
    Log(String),
    Phase(RemoteSessionPhase),
    Control(RemoteControlReport),
    Packet(RemotePacket),
    Delta(String),
}

#[derive(Debug, Clone)]
pub struct RemoteSessionOptions {
    pub include_raw_logs: bool,
    pub disable_radio_before_remote: bool,
    pub recover_retries: usize,
    pub suppress_repeated_idle: bool,
    pub suppress_idle_zero_logs: bool,
    pub idle_poll_interval: Duration,
    pub command_reaction_window: Duration,
    pub unknown_capture_limit: usize,
    pub unknown_capture_gap: Duration,
}

impl Default for RemoteSessionOptions {
    fn default() -> Self {
        Self {
            include_raw_logs: false,
            disable_radio_before_remote: false,
            recover_retries: 0,
            suppress_repeated_idle: true,
            suppress_idle_zero_logs: true,
            idle_poll_interval: Duration::from_millis(5),
            command_reaction_window: Duration::from_millis(250),
            unknown_capture_limit: 64,
            unknown_capture_gap: Duration::from_millis(20),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RemoteCaptureSummary {
    pub packet_count: usize,
    pub unknown_packet_count: usize,
    pub recovery_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteCommandReaction {
    pub window_ms: u128,
    pub rx_first_ms: Option<u128>,
    pub surfaced_packets: usize,
    pub unknown_packets: usize,
    pub deltas: usize,
}

pub fn run_remote_session<F, S, N>(
    port_name: &str,
    options: &RemoteSessionOptions,
    mut should_stop: S,
    mut next_command: N,
    mut on_event: F,
) -> RemoteSessionResult<RemoteCaptureSummary>
where
    F: FnMut(RemoteCaptureEvent),
    S: FnMut(Duration) -> bool,
    N: FnMut(Duration) -> Option<RemoteControlCommand>,
{
    let mut summary = RemoteCaptureSummary::default();
    let mut last_phase = None;
    let mut recoveries_left = options.recover_retries;

    loop {
        emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Opening);
        on_event(RemoteCaptureEvent::Status(format!(
            "Opening {port_name} for remote session"
        )));

        let mut proto = match RadioProtocol::new_with_baud(port_name, NICFW_REMOTE_BAUD) {
            Ok(proto) => proto,
            Err(error) => {
                let failure = RemoteSessionFailure::new(
                    RemoteSessionFailureKind::OpenFailed,
                    format!("Failed to open remote port {port_name}."),
                    error.to_string(),
                );
                if recoveries_left > 0 {
                    recoveries_left -= 1;
                    summary.recovery_count += 1;
                    emit_phase(
                        &mut on_event,
                        &mut last_phase,
                        RemoteSessionPhase::Recovering,
                    );
                    on_event(RemoteCaptureEvent::Status(format!(
                        "{} Retrying...",
                        failure.summary
                    )));
                    thread::sleep(Duration::from_millis(300));
                    continue;
                }
                return Err(failure);
            }
        };

        let log_rx = if options.include_raw_logs {
            let (log_tx, log_rx) = mpsc::channel();
            proto.log_callback = Some(Box::new(move |message| {
                let _ = log_tx.send(message);
            }));
            Some(log_rx)
        } else {
            None
        };

        if parse_ble_device_uri(port_name).is_some() {
            on_event(RemoteCaptureEvent::Status(
                "Waiting for BLE remote session to settle".to_string(),
            ));
            thread::sleep(BLE_REMOTE_SETTLE_DELAY);
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
        }

        if options.disable_radio_before_remote {
            let handshake_ok = proto.handshake().map_err(|error| {
                RemoteSessionFailure::new(
                    RemoteSessionFailureKind::BootstrapHandshakeFailed,
                    "Remote bootstrap handshake failed before nicFW remote mode opened.",
                    format!("Handshake attempt failed: {error}"),
                )
            })?;
            if !handshake_ok {
                let failure = RemoteSessionFailure::new(
                    RemoteSessionFailureKind::BootstrapHandshakeFailed,
                    "Remote bootstrap handshake failed before nicFW remote mode opened.",
                    "Remote handshake failed".to_string(),
                );
                if should_recover(
                    &mut on_event,
                    &mut last_phase,
                    &mut summary,
                    &mut recoveries_left,
                    &failure.summary,
                ) {
                    continue;
                }
                return Err(failure);
            }
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
            let disabled = proto.disable_radio().map_err(|error| {
                RemoteSessionFailure::new(
                    RemoteSessionFailureKind::BootstrapHandshakeFailed,
                    "Remote bootstrap handshake failed while preparing the radio.",
                    format!("Failed to disable radio before remote session: {error}"),
                )
            })?;
            if !disabled {
                let failure = RemoteSessionFailure::new(
                    RemoteSessionFailureKind::BootstrapHandshakeFailed,
                    "Remote bootstrap handshake failed while preparing the radio.",
                    "Radio did not acknowledge disable request".to_string(),
                );
                if should_recover(
                    &mut on_event,
                    &mut last_phase,
                    &mut summary,
                    &mut recoveries_left,
                    &failure.summary,
                ) {
                    continue;
                }
                return Err(failure);
            }
            on_event(RemoteCaptureEvent::Status(
                "Radio disabled for remote probing".to_string(),
            ));
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
        }

        if let Err(failure) = nicfw_remote_on(&mut proto) {
            if should_recover(
                &mut on_event,
                &mut last_phase,
                &mut summary,
                &mut recoveries_left,
                &failure.summary,
            ) {
                continue;
            }
            return Err(failure);
        }
        emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Armed);
        on_event(RemoteCaptureEvent::Status("Remote mode ON".to_string()));
        drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
        let armed_at = Instant::now();
        let mut current_key = NICFW_REMOTE_RELEASE;
        let mut next_heartbeat_at = NICFW_REMOTE_HEARTBEAT_INTERVAL;
        let mut packet_cache = HashMap::<String, String>::new();

        let mut live_seen = false;
        let session_result = (|| -> RemoteSessionResult<()> {
            loop {
                let armed_elapsed = armed_at.elapsed();

                if should_stop(armed_elapsed) {
                    let _ = nicfw_remote_off(&mut proto);
                    drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
                    emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Stopped);
                    on_event(RemoteCaptureEvent::Status("Remote mode OFF".to_string()));
                    return Ok(());
                }

                while let Some(command) = next_command(armed_elapsed) {
                    emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Probing);
                    let mut report = execute_remote_command(
                        &mut proto,
                        &command,
                        armed_at,
                        &mut current_key,
                        &mut next_heartbeat_at,
                        || should_stop(armed_at.elapsed()),
                    );
                    let reaction = observe_command_reaction_window(
                        &mut proto,
                        options,
                        &log_rx,
                        &mut on_event,
                        &mut summary,
                        &mut packet_cache,
                        &mut last_phase,
                        &mut live_seen,
                    )?;
                    report.reaction = Some(reaction.clone());
                    if report.detail.is_empty() {
                        report.detail =
                            format_command_reaction(options.command_reaction_window, &reaction);
                    } else {
                        report.detail = format!(
                            "{} | {}",
                            report.detail,
                            format_command_reaction(options.command_reaction_window, &reaction)
                        );
                    }
                    let success = report.success;
                    report.evidence =
                        classify_remote_evidence(report.success, report.reaction.as_ref());
                    let failure_detail = report.detail.clone();
                    on_event(RemoteCaptureEvent::Control(report));
                    drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
                    if !success {
                        return Err(RemoteSessionFailure::new(
                            RemoteSessionFailureKind::StreamLost,
                            "Remote command transport failed after the session opened.",
                            failure_detail,
                        ));
                    }
                    if let Some(steady_state_key) = command.steady_state_key {
                        current_key = steady_state_key;
                    }
                    emit_phase(
                        &mut on_event,
                        &mut last_phase,
                        if live_seen {
                            RemoteSessionPhase::Live
                        } else {
                            RemoteSessionPhase::Armed
                        },
                    );
                }

                service_remote_heartbeat(
                    &mut proto,
                    current_key,
                    &mut next_heartbeat_at,
                    armed_elapsed,
                )
                .map_err(|error| {
                    RemoteSessionFailure::new(
                        RemoteSessionFailureKind::StreamLost,
                        "Remote packet stream stalled while sending keepalive traffic.",
                        error.to_string(),
                    )
                })?;
                drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);

                match proto.parse_remote_packet_with_options(
                    options.unknown_capture_limit,
                    options.unknown_capture_gap,
                ) {
                    Ok(Some(packet)) => {
                        process_remote_packet(
                            packet,
                            options,
                            &mut packet_cache,
                            &mut summary,
                            &mut on_event,
                            &mut last_phase,
                            &mut live_seen,
                        );
                    }
                    Ok(None) => thread::sleep(NICFW_REMOTE_IDLE_CADENCE),
                    Err(error) => {
                        return Err(RemoteSessionFailure::new(
                            RemoteSessionFailureKind::StreamLost,
                            "Remote packet stream failed after nicFW remote mode opened.",
                            error.to_string(),
                        ));
                    }
                }
            }
        })();

        match session_result {
            Ok(()) => return Ok(summary),
            Err(failure) => {
                let _ = nicfw_remote_off(&mut proto);
                drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
                if should_recover(
                    &mut on_event,
                    &mut last_phase,
                    &mut summary,
                    &mut recoveries_left,
                    &failure.summary,
                ) {
                    continue;
                }
                return Err(failure);
            }
        }
    }
}

fn nicfw_remote_on(proto: &mut RadioProtocol) -> RemoteSessionResult<()> {
    let mut bootstrap = vec![NICFW_REMOTE_SYNC_BYTE; NICFW_REMOTE_SYNC_COUNT];
    bootstrap.push(NICFW_REMOTE_STOP);
    proto.send_bytes(&bootstrap).map_err(|error| {
        RemoteSessionFailure::new(
            RemoteSessionFailureKind::BootstrapHandshakeFailed,
            "Failed to send the nicFW remote bootstrap sequence.",
            error.to_string(),
        )
    })?;
    wait_for_remote_stop_echo(proto, NICFW_REMOTE_ACK_TIMEOUT).map_err(|error| {
        RemoteSessionFailure::new(
            RemoteSessionFailureKind::BootstrapHandshakeFailed,
            "Remote bootstrap did not echo the expected stop byte.",
            format!("nicFW bootstrap did not echo 4B: {error}"),
        )
    })?;

    proto.send_bytes(&[NICFW_REMOTE_START]).map_err(|error| {
        RemoteSessionFailure::new(
            RemoteSessionFailureKind::RemoteOnAckFailed,
            "Failed to send the nicFW remote-on byte.",
            error.to_string(),
        )
    })?;
    read_required_remote_byte(proto, NICFW_REMOTE_ACK_TIMEOUT, NICFW_REMOTE_START).map_err(
        |error| {
            RemoteSessionFailure::new(
                RemoteSessionFailureKind::RemoteOnAckFailed,
                "Remote mode did not acknowledge the expected 4A start echo.",
                format!("nicFW start did not echo 4A as the immediate ack: {error}"),
            )
        },
    )?;
    Ok(())
}

fn nicfw_remote_off(proto: &mut RadioProtocol) -> Result<()> {
    proto.send_bytes(&[NICFW_REMOTE_STOP])?;
    Ok(())
}

fn wait_for_remote_stop_echo(proto: &mut RadioProtocol, timeout: Duration) -> Result<()> {
    wait_for_remote_stop_echo_with(|| proto.read_byte(), timeout)
}

fn read_required_remote_byte(
    proto: &mut RadioProtocol,
    timeout: Duration,
    expected: u8,
) -> Result<()> {
    let byte = read_next_remote_byte_with_timeout(|| proto.read_byte(), timeout)?;
    if byte == expected {
        Ok(())
    } else {
        bail!("expected {expected:02X}, got {byte:02X}");
    }
}

fn wait_for_remote_stop_echo_with<R>(mut read_next: R, timeout: Duration) -> Result<()>
where
    R: FnMut() -> Result<Option<u8>>,
{
    wait_for_remote_byte_with(&mut read_next, timeout, |byte| byte == NICFW_REMOTE_STOP)
}

fn wait_for_remote_byte_with<R, F>(
    read_next: &mut R,
    timeout: Duration,
    mut matches: F,
) -> Result<()>
where
    R: FnMut() -> Result<Option<u8>>,
    F: FnMut(u8) -> bool,
{
    let started = Instant::now();
    let mut observed = Vec::new();
    while started.elapsed() < timeout {
        match read_next()? {
            Some(byte) if matches(byte) => return Ok(()),
            Some(byte) => observed.push(byte),
            None => {}
        }
    }

    if observed.is_empty() {
        bail!("timed out waiting for bootstrap echo");
    }

    let observed = observed
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    bail!("timed out waiting for bootstrap echo; observed {observed}");
}

fn read_next_remote_byte_with_timeout<R>(mut read_next: R, timeout: Duration) -> Result<u8>
where
    R: FnMut() -> Result<Option<u8>>,
{
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(byte) = read_next()? {
            return Ok(byte);
        }
    }
    bail!("timed out waiting for remote byte");
}

fn service_remote_heartbeat(
    proto: &mut RadioProtocol,
    current_key: u8,
    next_heartbeat_at: &mut Duration,
    session_elapsed: Duration,
) -> Result<()> {
    while session_elapsed >= *next_heartbeat_at {
        proto.send_bytes(&[current_key])?;
        *next_heartbeat_at += NICFW_REMOTE_HEARTBEAT_INTERVAL;
    }
    Ok(())
}

fn heartbeat_key_after_step(command: &RemoteControlCommand, step: &RemoteWriteStep) -> Option<u8> {
    if command.steady_state_key.is_none() || step.bytes.len() != 1 {
        return None;
    }

    let byte = step.bytes[0];
    if byte == NICFW_REMOTE_RELEASE || byte >= 0x80 {
        Some(byte)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn observe_command_reaction_window<F>(
    proto: &mut RadioProtocol,
    options: &RemoteSessionOptions,
    log_rx: &Option<mpsc::Receiver<String>>,
    on_event: &mut F,
    summary: &mut RemoteCaptureSummary,
    packet_cache: &mut HashMap<String, String>,
    last_phase: &mut Option<RemoteSessionPhase>,
    live_seen: &mut bool,
) -> RemoteSessionResult<RemoteCommandReaction>
where
    F: FnMut(RemoteCaptureEvent),
{
    let mut reaction = RemoteCommandReaction {
        window_ms: options.command_reaction_window.as_millis(),
        ..RemoteCommandReaction::default()
    };
    let window_started = Instant::now();
    while window_started.elapsed() < options.command_reaction_window {
        drain_logs_with_filter_and_observer(
            log_rx,
            on_event,
            options.suppress_idle_zero_logs,
            |message| {
                if reaction.rx_first_ms.is_none() && message.trim_start().starts_with("RX:") {
                    reaction.rx_first_ms = Some(window_started.elapsed().as_millis());
                }
            },
        );

        match proto.parse_remote_packet_with_options(
            options.unknown_capture_limit,
            options.unknown_capture_gap,
        ) {
            Ok(Some(packet)) => {
                if reaction.rx_first_ms.is_none() {
                    reaction.rx_first_ms = Some(window_started.elapsed().as_millis());
                }
                let observed = process_remote_packet(
                    packet,
                    options,
                    packet_cache,
                    summary,
                    on_event,
                    last_phase,
                    live_seen,
                );
                if observed.surfaced {
                    if observed.unknown {
                        reaction.unknown_packets += 1;
                    } else {
                        reaction.surfaced_packets += 1;
                    }
                }
                if observed.had_delta {
                    reaction.deltas += 1;
                }
            }
            Ok(None) => thread::sleep(options.idle_poll_interval),
            Err(error) => {
                return Err(RemoteSessionFailure::new(
                    RemoteSessionFailureKind::StreamLost,
                    "Remote packet stream failed while observing command reaction.",
                    format!("Remote reaction window failed: {error}"),
                ));
            }
        }
    }

    drain_logs_with_filter_and_observer(
        log_rx,
        on_event,
        options.suppress_idle_zero_logs,
        |message| {
            if reaction.rx_first_ms.is_none() && message.trim_start().starts_with("RX:") {
                reaction.rx_first_ms = Some(window_started.elapsed().as_millis());
            }
        },
    );

    Ok(reaction)
}

fn execute_remote_command(
    proto: &mut RadioProtocol,
    command: &RemoteControlCommand,
    session_started: Instant,
    current_key: &mut u8,
    next_heartbeat_at: &mut Duration,
    mut should_stop: impl FnMut() -> bool,
) -> RemoteControlReport {
    let started = Instant::now();
    let mut step_details = Vec::new();
    for (index, step) in command.steps.iter().enumerate() {
        if should_stop() {
            return RemoteControlReport {
                label: command.label.clone(),
                strategy: command.strategy,
                bytes_hex: command.bytes_hex(),
                success: false,
                evidence: RemoteEvidenceKind::CommandFailed,
                reaction: None,
                detail: format!(
                    "stopped before command completed | {}",
                    step_details.join(" ; ")
                ),
            };
        }

        if let Err(error) = service_remote_heartbeat(
            proto,
            *current_key,
            next_heartbeat_at,
            session_started.elapsed(),
        ) {
            return RemoteControlReport {
                label: command.label.clone(),
                strategy: command.strategy,
                bytes_hex: command.bytes_hex(),
                success: false,
                evidence: RemoteEvidenceKind::CommandFailed,
                reaction: None,
                detail: format!("heartbeat failed before step {}: {error}", index + 1),
            };
        }

        if matches!(command.strategy, RemoteControlStrategy::RawKey) && !step.bytes.is_empty() {
            thread::sleep(Duration::from_millis(50));
        }

        let elapsed_ms = started.elapsed().as_millis();
        let bytes_hex = if step.bytes.is_empty() {
            "<pause>".to_string()
        } else {
            format_step_bytes(&step.bytes)
        };
        step_details.push(format!(
            "step{} +{}ms {} wait {}ms",
            index + 1,
            elapsed_ms,
            bytes_hex,
            step.pause_after.as_millis()
        ));

        if !step.bytes.is_empty()
            && let Err(error) = proto.send_bytes(&step.bytes)
        {
            return RemoteControlReport {
                label: command.label.clone(),
                strategy: command.strategy,
                bytes_hex: command.bytes_hex(),
                success: false,
                evidence: RemoteEvidenceKind::CommandFailed,
                reaction: None,
                detail: format!("{} | {}", error, step_details.join(" ; ")),
            };
        }

        if let Some(heartbeat_key_after) = heartbeat_key_after_step(command, step) {
            *current_key = heartbeat_key_after;
        }

        if step.pause_after > Duration::ZERO {
            let pause_started = Instant::now();
            while pause_started.elapsed() < step.pause_after {
                if should_stop() {
                    return RemoteControlReport {
                        label: command.label.clone(),
                        strategy: command.strategy,
                        bytes_hex: command.bytes_hex(),
                        success: false,
                        evidence: RemoteEvidenceKind::CommandFailed,
                        reaction: None,
                        detail: format!(
                            "stopped during command wait | {}",
                            step_details.join(" ; ")
                        ),
                    };
                }
                if let Err(error) = service_remote_heartbeat(
                    proto,
                    *current_key,
                    next_heartbeat_at,
                    session_started.elapsed(),
                ) {
                    return RemoteControlReport {
                        label: command.label.clone(),
                        strategy: command.strategy,
                        bytes_hex: command.bytes_hex(),
                        success: false,
                        evidence: RemoteEvidenceKind::CommandFailed,
                        reaction: None,
                        detail: format!(
                            "heartbeat failed during command wait | {} | {}",
                            error,
                            step_details.join(" ; ")
                        ),
                    };
                }
                let remaining = step.pause_after.saturating_sub(pause_started.elapsed());
                thread::sleep(remaining.min(Duration::from_millis(10)));
            }
        }
    }

    RemoteControlReport {
        label: command.label.clone(),
        strategy: command.strategy,
        bytes_hex: command.bytes_hex(),
        success: true,
        evidence: RemoteEvidenceKind::NoTelemetry,
        reaction: None,
        detail: step_details.join(" ; "),
    }
}

fn emit_phase<F>(
    on_event: &mut F,
    last_phase: &mut Option<RemoteSessionPhase>,
    phase: RemoteSessionPhase,
) where
    F: FnMut(RemoteCaptureEvent),
{
    if last_phase != &Some(phase) {
        *last_phase = Some(phase);
        on_event(RemoteCaptureEvent::Phase(phase));
    }
}

fn should_recover<F>(
    on_event: &mut F,
    last_phase: &mut Option<RemoteSessionPhase>,
    summary: &mut RemoteCaptureSummary,
    recoveries_left: &mut usize,
    message: &str,
) -> bool
where
    F: FnMut(RemoteCaptureEvent),
{
    if *recoveries_left == 0 {
        return false;
    }

    *recoveries_left -= 1;
    summary.recovery_count += 1;
    emit_phase(on_event, last_phase, RemoteSessionPhase::Recovering);
    on_event(RemoteCaptureEvent::Status(format!(
        "{message}. Retrying..."
    )));
    thread::sleep(Duration::from_millis(300));
    true
}

fn drain_logs_with_filter<F>(
    log_rx: &Option<mpsc::Receiver<String>>,
    on_event: &mut F,
    suppress_idle_zero_logs: bool,
) where
    F: FnMut(RemoteCaptureEvent),
{
    drain_logs_with_filter_and_observer(log_rx, on_event, suppress_idle_zero_logs, |_| {});
}

fn drain_logs_with_filter_and_observer<F, O>(
    log_rx: &Option<mpsc::Receiver<String>>,
    on_event: &mut F,
    suppress_idle_zero_logs: bool,
    mut observe: O,
) where
    F: FnMut(RemoteCaptureEvent),
    O: FnMut(&str),
{
    let Some(log_rx) = log_rx else {
        return;
    };

    while let Ok(message) = log_rx.try_recv() {
        if suppress_idle_zero_logs && matches!(message.trim(), "RX: [00]" | "RX: [00, 00]") {
            continue;
        }
        observe(&message);
        on_event(RemoteCaptureEvent::Log(message));
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ObservedPacket {
    surfaced: bool,
    unknown: bool,
    had_delta: bool,
}

fn process_remote_packet<F>(
    packet: RemotePacket,
    options: &RemoteSessionOptions,
    packet_cache: &mut HashMap<String, String>,
    summary: &mut RemoteCaptureSummary,
    on_event: &mut F,
    last_phase: &mut Option<RemoteSessionPhase>,
    live_seen: &mut bool,
) -> ObservedPacket
where
    F: FnMut(RemoteCaptureEvent),
{
    let (should_emit, delta) =
        record_remote_packet(packet_cache, &packet, options.suppress_repeated_idle);

    let observed = ObservedPacket {
        surfaced: should_emit,
        unknown: matches!(packet, RemotePacket::UnknownFrame { .. }),
        had_delta: delta.is_some(),
    };

    if let Some(delta) = delta {
        on_event(RemoteCaptureEvent::Delta(delta));
    }

    if observed.unknown {
        summary.unknown_packet_count += 1;
    }

    if !*live_seen {
        *live_seen = true;
        emit_phase(on_event, last_phase, RemoteSessionPhase::Live);
    }

    if should_emit {
        summary.packet_count += 1;
        on_event(RemoteCaptureEvent::Packet(packet));
    }

    observed
}

fn record_remote_packet(
    packet_cache: &mut HashMap<String, String>,
    packet: &RemotePacket,
    suppress_repeated_idle: bool,
) -> (bool, Option<String>) {
    let family_key = packet.family_key();
    let current_detail = packet.detail_key();
    let previous = packet_cache.insert(family_key.clone(), current_detail.clone());
    let repeated_idle = suppress_repeated_idle
        && packet.is_idle_telemetry()
        && previous.as_deref() == Some(current_detail.as_str());
    let delta = previous.and_then(|previous_detail| {
        if previous_detail != current_detail {
            Some(format!(
                "{}: {} -> {}",
                family_key, previous_detail, current_detail
            ))
        } else {
            None
        }
    });

    (!repeated_idle, delta)
}

fn format_step_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_command_reaction(reaction_window: Duration, reaction: &RemoteCommandReaction) -> String {
    let rx_first = reaction
        .rx_first_ms
        .map(|millis| format!("{millis}ms"))
        .unwrap_or_else(|| "none".to_string());
    format!(
        "reaction {}ms: rx-first={} surfaced={} unknown={} delta={}",
        reaction_window.as_millis(),
        rx_first,
        reaction.surfaced_packets,
        reaction.unknown_packets,
        reaction.deltas
    )
}

pub fn classify_remote_evidence(
    success: bool,
    reaction: Option<&RemoteCommandReaction>,
) -> RemoteEvidenceKind {
    if !success {
        return RemoteEvidenceKind::CommandFailed;
    }

    let Some(reaction) = reaction else {
        return RemoteEvidenceKind::NoTelemetry;
    };

    if reaction.deltas > 0 {
        RemoteEvidenceKind::ControlConfirmed
    } else if reaction.surfaced_packets > 0
        || reaction.unknown_packets > 0
        || reaction.rx_first_ms.is_some()
    {
        RemoteEvidenceKind::NoControlEvidence
    } else {
        RemoteEvidenceKind::NoTelemetry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::RemotePacket;

    #[test]
    fn raw_key_command_includes_release() {
        let command = RemoteControlCommand::raw_key("menu", 0x0B);
        assert_eq!(command.strategy, RemoteControlStrategy::RawKey);
        assert_eq!(command.bytes_hex(), "8A FF");
        assert_eq!(command.steps.len(), 2);
        assert_eq!(command.steady_state_key, Some(NICFW_REMOTE_RELEASE));
    }

    #[test]
    fn sequence_command_repeats_bytes() {
        let command = RemoteControlCommand::sequence(
            "menu-seq",
            vec![0x0B, 0x00],
            Duration::from_millis(20),
            2,
            Duration::ZERO,
        );
        assert_eq!(command.strategy, RemoteControlStrategy::Sequence);
        assert_eq!(command.bytes_hex(), "0B 00 0B 00");
        assert_eq!(command.steady_state_key, None);
    }

    #[test]
    fn held_key_command_delays_release_until_after_hold() {
        let command = RemoteControlCommand::held_key(
            "hold-menu",
            0x0B,
            0x00,
            Duration::from_millis(80),
            1,
            Duration::from_millis(500),
        );

        assert_eq!(command.bytes_hex(), "8A FF");
        assert_eq!(command.steps[0].pause_after, Duration::from_millis(500));
        assert_eq!(command.steps[1].pause_after, Duration::from_millis(80));
        assert_eq!(command.steady_state_key, Some(NICFW_REMOTE_RELEASE));
    }

    #[test]
    fn burst_command_keeps_bytes_in_one_step() {
        let command = RemoteControlCommand::burst(
            "prime",
            vec![0x64, 0x00, 0x67],
            Duration::from_millis(700),
        );
        assert_eq!(command.bytes_hex(), "64 00 67");
        assert_eq!(command.steps.len(), 1);
        assert_eq!(command.steps[0].pause_after, Duration::from_millis(700));
    }

    #[test]
    fn raw_key_duration_estimate_includes_press_and_release_spacing() {
        let command = RemoteControlCommand::raw_key("menu", 0x0B);
        assert_eq!(command.estimated_duration(), Duration::from_millis(220));
    }

    #[test]
    fn translates_existing_logical_key_codes_to_programmer_wire_bytes() {
        assert_eq!(programmer_remote_key_code(0x0B), 0x8A);
        assert_eq!(programmer_remote_key_code(0x0C), 0x8B);
        assert_eq!(programmer_remote_key_code(0x0D), 0x8C);
        assert_eq!(programmer_remote_key_code(0x0E), 0x8D);
        assert_eq!(programmer_remote_key_code(0x0F), 0x8E);
        assert_eq!(programmer_remote_key_code(0x10), 0x8F);
        assert_eq!(programmer_remote_key_code(0x13), 0x90);
        assert_eq!(programmer_remote_key_code(0x1A), 0x91);
        assert_eq!(programmer_remote_key_code(0x12), 0x92);
    }

    #[test]
    fn bootstrap_wait_ignores_echoed_sync_bytes_until_stop_echo() {
        let mut reads = vec![Some(0x01), Some(0x01), Some(0x4B)].into_iter();
        let result = wait_for_remote_stop_echo_with(
            || Ok(reads.next().unwrap_or(None)),
            Duration::from_millis(10),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn bootstrap_second_stage_requires_immediate_start_echo() {
        let mut reads = vec![Some(0x01)].into_iter();
        let result = read_next_remote_byte_with_timeout(
            || Ok(reads.next().unwrap_or(None)),
            Duration::from_millis(10),
        );
        assert_eq!(result.unwrap(), 0x01);
    }

    #[test]
    fn bootstrap_second_stage_treats_non_start_echo_as_failure() {
        let mut reads = vec![Some(0x01)].into_iter();
        let byte = read_next_remote_byte_with_timeout(
            || Ok(reads.next().unwrap_or(None)),
            Duration::from_millis(10),
        )
        .unwrap();
        let error = if byte == NICFW_REMOTE_START {
            String::new()
        } else {
            format!("expected {:02X}, got {:02X}", NICFW_REMOTE_START, byte)
        };
        assert!(error.contains("expected 4A, got 01"));
    }

    #[test]
    fn classify_remote_evidence_distinguishes_confirmed_control() {
        let reaction = RemoteCommandReaction {
            window_ms: 250,
            rx_first_ms: Some(12),
            surfaced_packets: 1,
            unknown_packets: 0,
            deltas: 1,
        };

        assert_eq!(
            classify_remote_evidence(true, Some(&reaction)),
            RemoteEvidenceKind::ControlConfirmed
        );
    }

    #[test]
    fn classify_remote_evidence_distinguishes_telemetry_without_control() {
        let reaction = RemoteCommandReaction {
            window_ms: 250,
            rx_first_ms: Some(12),
            surfaced_packets: 1,
            unknown_packets: 0,
            deltas: 0,
        };

        assert_eq!(
            classify_remote_evidence(true, Some(&reaction)),
            RemoteEvidenceKind::NoControlEvidence
        );
    }

    #[test]
    fn classify_remote_evidence_distinguishes_no_telemetry() {
        let reaction = RemoteCommandReaction {
            window_ms: 250,
            rx_first_ms: None,
            surfaced_packets: 0,
            unknown_packets: 0,
            deltas: 0,
        };

        assert_eq!(
            classify_remote_evidence(true, Some(&reaction)),
            RemoteEvidenceKind::NoTelemetry
        );
    }

    #[test]
    fn classify_remote_evidence_distinguishes_command_failures() {
        assert_eq!(
            classify_remote_evidence(false, None),
            RemoteEvidenceKind::CommandFailed
        );
    }

    #[test]
    fn idle_packets_share_expected_keys() {
        let status = RemotePacket::SmallStatus {
            id: 0x70,
            value1: 0x00,
            value2: 0x00,
        };
        let battery = RemotePacket::DisplayText {
            font_size: 0,
            x: 103,
            y: 70,
            fg_color: 0,
            bg_color: 0,
            text: "6.5V".to_string(),
        };

        assert!(status.is_idle_telemetry());
        assert_eq!(status.family_key(), "status:70");
        assert!(battery.is_idle_telemetry());
        assert_eq!(battery.family_key(), "battery_text");
    }

    #[test]
    fn detail_key_preserves_full_display_text() {
        let packet = RemotePacket::DisplayText {
            font_size: 1,
            x: 12,
            y: 34,
            fg_color: 0x1234,
            bg_color: 0x5678,
            text: "THIS IS A LONGER REMOTE STRING".to_string(),
        };

        assert!(
            packet
                .detail_key()
                .contains("THIS IS A LONGER REMOTE STRING")
        );
    }

    #[test]
    fn packet_recording_suppresses_repeated_idle_packets() {
        let mut cache = HashMap::new();
        let packet = RemotePacket::DisplayText {
            font_size: 0,
            x: 103,
            y: 70,
            fg_color: 0,
            bg_color: 0,
            text: "6.5V".to_string(),
        };

        let (emit_first, delta_first) = record_remote_packet(&mut cache, &packet, true);
        assert!(emit_first);
        assert!(delta_first.is_none());

        let (emit_second, delta_second) = record_remote_packet(&mut cache, &packet, true);
        assert!(!emit_second);
        assert!(delta_second.is_none());
    }

    #[test]
    fn packet_recording_emits_deltas_for_changed_packets() {
        let mut cache = HashMap::new();
        let first = RemotePacket::SmallStatus {
            id: 0x70,
            value1: 0x00,
            value2: 0x00,
        };
        let second = RemotePacket::SmallStatus {
            id: 0x70,
            value1: 0x01,
            value2: 0x00,
        };

        let (emit_first, delta_first) = record_remote_packet(&mut cache, &first, true);
        assert!(emit_first);
        assert!(delta_first.is_none());

        let (emit_second, delta_second) = record_remote_packet(&mut cache, &second, true);
        assert!(emit_second);
        assert_eq!(
            delta_second.as_deref(),
            Some("status:70: STS:70:00:00 -> STS:70:01:00")
        );
    }

    #[test]
    fn command_reaction_formats_without_activity() {
        let reaction = RemoteCommandReaction::default();
        assert_eq!(
            format_command_reaction(Duration::from_millis(250), &reaction),
            "reaction 250ms: rx-first=none surfaced=0 unknown=0 delta=0"
        );
    }

    #[test]
    fn command_reaction_formats_with_activity() {
        let reaction = RemoteCommandReaction {
            window_ms: 250,
            rx_first_ms: Some(17),
            surfaced_packets: 2,
            unknown_packets: 1,
            deltas: 3,
        };
        assert_eq!(
            format_command_reaction(Duration::from_millis(250), &reaction),
            "reaction 250ms: rx-first=17ms surfaced=2 unknown=1 delta=3"
        );
    }
}
