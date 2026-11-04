use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::ble::parse_ble_device_uri;
use crate::protocol::{RadioProtocol, RemotePacket};

const NICFW_REMOTE_BAUD: u32 = 38_400;
const NICFW_REMOTE_SYNC_BYTE: u8 = 0x01;
const NICFW_REMOTE_SYNC_COUNT: usize = 10;
const NICFW_REMOTE_STOP: u8 = 0x4B;
const NICFW_REMOTE_START: u8 = 0x4A;
const NICFW_REMOTE_RELEASE: u8 = 0xFF;
const NICFW_REMOTE_ACK_TIMEOUT: Duration = Duration::from_millis(500);
const NICFW_REMOTE_ECHO_MAX_BYTES: usize = 4096;
const NICFW_REMOTE_TEXT_MAX_BYTES: usize = 256;
const NICFW_REMOTE_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(5000);
// Nominal uninterrupted tap release in the programmer: two 200ms stages.
const PROGRAMMER_TAP_HOLD: Duration = Duration::from_millis(400);
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
    /// Reserved for command-specific, correlated state responses. Aggregate
    /// drawing/telemetry changes alone cannot establish this evidence.
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
                    pause_after: PROGRAMMER_TAP_HOLD,
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
    /// Whether the transport writes completed, not whether the radio acted on them.
    pub success: bool,
    pub evidence: RemoteEvidenceKind,
    pub reaction: Option<RemoteCommandReaction>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub enum RemoteCaptureEvent {
    Received(Instant),
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
    /// Observed packet changes, including unsolicited updates and buffered data.
    /// This count does not attribute any change to the command.
    pub deltas: usize,
}

struct RemoteSessionCleanup {
    protocol: RadioProtocol,
    stop_required: bool,
    enable_required: bool,
    decoder_boundary_trusted: bool,
}

impl std::ops::Deref for RemoteSessionCleanup {
    type Target = RadioProtocol;

    fn deref(&self) -> &Self::Target {
        &self.protocol
    }
}

impl std::ops::DerefMut for RemoteSessionCleanup {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.protocol
    }
}

impl RemoteSessionCleanup {
    fn parse_remote_packet_with_options(
        &mut self,
        capture_limit: usize,
        capture_gap: Duration,
    ) -> Result<Option<RemotePacket>> {
        let packet = self
            .protocol
            .parse_remote_packet_with_options(capture_limit, capture_gap);
        if packet.is_err() {
            self.decoder_boundary_trusted = false;
        }
        packet
    }

    fn wait_for_bootstrap_echoes(&mut self, expected: u8, count: usize) -> Result<()> {
        let Self {
            protocol,
            decoder_boundary_trusted,
            ..
        } = self;
        wait_for_remote_echoes_with_boundary(
            || {
                if crate::cancellation::is_requested() {
                    bail!("Remote bootstrap cancelled");
                }
                protocol.read_byte()
            },
            NICFW_REMOTE_ACK_TIMEOUT,
            expected,
            count,
            decoder_boundary_trusted,
        )
    }

    fn close(&mut self) -> Result<()> {
        // Attempt both operations even if stopping remote mode fails. Clear the
        // flags first so unwinding or Drop cannot send duplicate cleanup commands.
        let stop_result = if std::mem::take(&mut self.stop_required) {
            nicfw_remote_off(&mut self.protocol)
                .context("Failed to send remote stop 4B")
                .and_then(|()| {
                    if !self.decoder_boundary_trusted {
                        bail!("Remote stop 4B acknowledgment cannot be confirmed: decoder frame boundary is unknown");
                    }
                    wait_for_remote_stop_echo(&mut self.protocol, NICFW_REMOTE_ACK_TIMEOUT)
                        .context("Remote stop 4B acknowledgment was not confirmed")
                })
        } else {
            Ok(())
        };
        let enable_result = if std::mem::take(&mut self.enable_required) {
            self.protocol.enable_radio().and_then(|ack| {
                if ack {
                    Ok(())
                } else {
                    bail!("Radio did not acknowledge re-enable after remote session")
                }
            })
        } else {
            Ok(())
        };
        match (stop_result, enable_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(enable_error)) => {
                Err(error.context(format!("Radio re-enable also failed: {enable_error}")))
            }
        }
    }
}

impl Drop for RemoteSessionCleanup {
    fn drop(&mut self) {
        let _ = self.close();
    }
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
    let mut stop_requested = false;
    let mut should_stop = |elapsed| {
        stop_requested |= crate::cancellation::is_requested() || should_stop(elapsed);
        stop_requested
    };

    loop {
        check_remote_cancellation()?;
        // Setup has no armed clock yet. Never reuse the previous stream's age.
        if should_stop(Duration::ZERO) {
            emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Stopped);
            on_event(RemoteCaptureEvent::Status("Remote mode OFF".to_string()));
            return Ok(summary);
        }
        emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Opening);
        on_event(RemoteCaptureEvent::Status(format!(
            "Opening {port_name} for remote session"
        )));

        check_remote_cancellation()?;
        if should_stop(Duration::ZERO) {
            emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Stopped);
            on_event(RemoteCaptureEvent::Status("Remote mode OFF".to_string()));
            return Ok(summary);
        }
        let proto = match RadioProtocol::new_with_baud(port_name, NICFW_REMOTE_BAUD) {
            Ok(proto) => proto,
            Err(error) => {
                let failure = RemoteSessionFailure::new(
                    RemoteSessionFailureKind::OpenFailed,
                    format!("Failed to open remote port {port_name}."),
                    error.to_string(),
                );
                if recoveries_left > 0 && !should_stop(Duration::ZERO) {
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
        let mut proto = RemoteSessionCleanup {
            protocol: proto,
            stop_required: false,
            enable_required: false,
            decoder_boundary_trusted: true,
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
            check_remote_cancellation()?;
            if should_stop(Duration::ZERO) {
                finish_remote_stop(&mut proto, &log_rx, options, &mut on_event, &mut last_phase)?;
                return Ok(summary);
            }
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
                if !should_stop(Duration::ZERO)
                    && should_recover(
                        &mut on_event,
                        &mut last_phase,
                        &mut summary,
                        &mut recoveries_left,
                        &failure.summary,
                    )
                {
                    continue;
                }
                return Err(failure);
            }
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
            check_remote_cancellation()?;
            if should_stop(Duration::ZERO) {
                finish_remote_stop(&mut proto, &log_rx, options, &mut on_event, &mut last_phase)?;
                return Ok(summary);
            }
            // A missing ACK does not prove the radio ignored the disable byte.
            proto.enable_required = true;
            let disabled = proto.disable_radio().map_err(|error| {
                RemoteSessionFailure::new(
                    RemoteSessionFailureKind::BootstrapHandshakeFailed,
                    "Remote bootstrap handshake failed while preparing the radio.",
                    format!("Failed to disable radio before remote session: {error}"),
                )
            });
            let failure = match disabled {
                Ok(true) => None,
                Ok(false) => Some(RemoteSessionFailure::new(
                    RemoteSessionFailureKind::BootstrapHandshakeFailed,
                    "Remote bootstrap handshake failed while preparing the radio.",
                    "Radio did not acknowledge disable request".to_string(),
                )),
                Err(failure) => Some(failure),
            };
            if let Some(mut failure) = failure {
                if let Err(error) = proto.close() {
                    failure
                        .detail
                        .push_str(&format!("; remote cleanup failed: {error:#}"));
                }
                drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
                if !should_stop(Duration::ZERO)
                    && should_recover(
                        &mut on_event,
                        &mut last_phase,
                        &mut summary,
                        &mut recoveries_left,
                        &failure.summary,
                    )
                {
                    continue;
                }
                return Err(failure);
            }
            on_event(RemoteCaptureEvent::Status(
                "Radio disabled for remote probing".to_string(),
            ));
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
        }

        if let Err(mut failure) = check_remote_cancellation() {
            if let Err(error) = proto.close() {
                failure
                    .detail
                    .push_str(&format!("; remote cleanup failed: {error:#}"));
            }
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
            return Err(failure);
        }
        if should_stop(Duration::ZERO) {
            finish_remote_stop(&mut proto, &log_rx, options, &mut on_event, &mut last_phase)?;
            return Ok(summary);
        }
        proto.stop_required = true;
        if let Err(mut failure) = nicfw_remote_on(
            &mut proto,
            parse_ble_device_uri(port_name).is_none(),
            || should_stop(Duration::ZERO),
        ) {
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
            on_event(RemoteCaptureEvent::Log(format!(
                "Remote setup failed: {}",
                failure.detail
            )));
            if let Err(error) = proto.close() {
                failure
                    .detail
                    .push_str(&format!("; remote cleanup failed: {error:#}"));
                on_event(RemoteCaptureEvent::Log(format!(
                    "Remote cleanup failed: {error:#}"
                )));
            }
            drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
            if !should_stop(Duration::ZERO)
                && should_recover(
                    &mut on_event,
                    &mut last_phase,
                    &mut summary,
                    &mut recoveries_left,
                    &failure.summary,
                )
            {
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
        let mut stopping = false;
        let session_result = (|| -> RemoteSessionResult<()> {
            'stream: loop {
                let armed_elapsed = armed_at.elapsed();

                if should_stop(armed_elapsed) {
                    stopping = true;
                    release_remote_key(&mut proto, &mut current_key).map_err(|error| {
                        RemoteSessionFailure::new(
                            RemoteSessionFailureKind::StreamLost,
                            "Held remote key release failed during cancellation.",
                            format!("{error:#}"),
                        )
                    })?;
                    proto.close().map_err(|error| {
                        RemoteSessionFailure::new(
                            RemoteSessionFailureKind::StreamLost,
                            "Remote shutdown was not confirmed.",
                            format!("{error:#}"),
                        )
                    })?;
                    drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
                    emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Stopped);
                    on_event(RemoteCaptureEvent::Status("Remote mode OFF".to_string()));
                    return Ok(());
                }

                while !should_stop(armed_at.elapsed())
                    && let Some(command) = next_command(armed_elapsed)
                {
                    emit_phase(&mut on_event, &mut last_phase, RemoteSessionPhase::Probing);
                    let mut report = execute_remote_command(
                        &mut proto,
                        &command,
                        armed_at,
                        &mut current_key,
                        &mut next_heartbeat_at,
                        || should_stop(armed_at.elapsed()),
                    );
                    if should_stop(armed_at.elapsed()) {
                        on_event(RemoteCaptureEvent::Control(report));
                        continue 'stream;
                    }
                    let reaction = observe_command_reaction_window(
                        &mut proto,
                        options,
                        &log_rx,
                        &mut on_event,
                        &mut summary,
                        &mut packet_cache,
                        &mut last_phase,
                        &mut live_seen,
                        || should_stop(armed_at.elapsed()),
                    )?;
                    if should_stop(armed_at.elapsed()) {
                        on_event(RemoteCaptureEvent::Control(report));
                        continue 'stream;
                    }
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

                if should_stop(armed_at.elapsed()) {
                    continue;
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
                    // An idle transport read already waits for its timeout. A NOP
                    // must not delay valid packets queued behind it.
                    Ok(None) => {}
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
            Err(mut failure) => {
                if let Err(error) = release_remote_key(&mut proto, &mut current_key) {
                    failure
                        .detail
                        .push_str(&format!("; remote key release failed: {error:#}"));
                }
                if let Err(error) = proto.close() {
                    failure
                        .detail
                        .push_str(&format!("; remote cleanup failed: {error:#}"));
                    on_event(RemoteCaptureEvent::Log(format!(
                        "Remote cleanup failed: {error:#}"
                    )));
                }
                drain_logs_with_filter(&log_rx, &mut on_event, options.suppress_idle_zero_logs);
                // An explicit stop must never reopen remote mode to recover an ACK.
                if stopping || should_stop(armed_at.elapsed()) {
                    return Err(failure);
                }
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

fn finish_remote_stop<F>(
    proto: &mut RemoteSessionCleanup,
    log_rx: &Option<mpsc::Receiver<String>>,
    options: &RemoteSessionOptions,
    on_event: &mut F,
    last_phase: &mut Option<RemoteSessionPhase>,
) -> RemoteSessionResult<()>
where
    F: FnMut(RemoteCaptureEvent),
{
    let closed = proto.close().map_err(|error| {
        RemoteSessionFailure::new(
            RemoteSessionFailureKind::StreamLost,
            "Remote shutdown was not confirmed.",
            format!("{error:#}"),
        )
    });
    drain_logs_with_filter(log_rx, on_event, options.suppress_idle_zero_logs);
    closed?;
    emit_phase(on_event, last_phase, RemoteSessionPhase::Stopped);
    on_event(RemoteCaptureEvent::Status("Remote mode OFF".to_string()));
    Ok(())
}

fn release_remote_key(proto: &mut RadioProtocol, current_key: &mut u8) -> Result<()> {
    if *current_key != NICFW_REMOTE_RELEASE {
        *current_key = NICFW_REMOTE_RELEASE;
        proto
            .send_bytes(&[NICFW_REMOTE_RELEASE])
            .context("Failed to release held remote key before stop")?;
    }
    Ok(())
}

fn check_remote_cancellation() -> RemoteSessionResult<()> {
    if crate::cancellation::is_requested() {
        return Err(RemoteSessionFailure::new(
            RemoteSessionFailureKind::StreamLost,
            "Remote session cancelled.",
            "Cancellation requested; no further remote commands will be sent.",
        ));
    }
    Ok(())
}

fn nicfw_remote_on(
    proto: &mut RemoteSessionCleanup,
    staged_sync: bool,
    mut should_stop: impl FnMut() -> bool,
) -> RemoteSessionResult<()> {
    let mut check_stop = || {
        check_remote_cancellation()?;
        if should_stop() {
            return Err(RemoteSessionFailure::new(
                RemoteSessionFailureKind::StreamLost,
                "Remote session stopped during bootstrap.",
                "Stop requested; no further bootstrap commands will be sent.",
            ));
        }
        Ok(())
    };
    check_stop()?;
    if staged_sync {
        // Pace each USB sync byte by its echo; never queue a burst or add sleeps.
        for index in 0..NICFW_REMOTE_SYNC_COUNT {
            check_stop()?;
            proto
                .send_bytes(&[NICFW_REMOTE_SYNC_BYTE])
                .map_err(|error| {
                    RemoteSessionFailure::new(
                        RemoteSessionFailureKind::BootstrapHandshakeFailed,
                        "Failed to send a nicFW bootstrap sync byte.",
                        format!("Sync byte {}/10: {error}", index + 1),
                    )
                })?;
            proto
                .wait_for_bootstrap_echoes(NICFW_REMOTE_SYNC_BYTE, 1)
                .map_err(|error| {
                    RemoteSessionFailure::new(
                        RemoteSessionFailureKind::BootstrapHandshakeFailed,
                        "Remote bootstrap did not echo all ten sync bytes.",
                        format!(
                            "nicFW bootstrap sync failed at byte {}/10: {error:#}",
                            index + 1
                        ),
                    )
                })?;
        }
        check_stop()?;
        proto.send_bytes(&[NICFW_REMOTE_STOP]).map_err(|error| {
            RemoteSessionFailure::new(
                RemoteSessionFailureKind::BootstrapHandshakeFailed,
                "Failed to send the standalone nicFW bootstrap stop byte.",
                error.to_string(),
            )
        })?;
    } else {
        check_stop()?;
        // BLE retains the programmer's combined bootstrap write.
        let mut bootstrap = vec![NICFW_REMOTE_SYNC_BYTE; NICFW_REMOTE_SYNC_COUNT];
        bootstrap.push(NICFW_REMOTE_STOP);
        proto.send_bytes(&bootstrap).map_err(|error| {
            RemoteSessionFailure::new(
                RemoteSessionFailureKind::BootstrapHandshakeFailed,
                "Failed to send the nicFW remote bootstrap sequence.",
                error.to_string(),
            )
        })?;
    }
    proto
        .wait_for_bootstrap_echoes(NICFW_REMOTE_STOP, 1)
        .map_err(|error| {
            RemoteSessionFailure::new(
                RemoteSessionFailureKind::BootstrapHandshakeFailed,
                "Remote bootstrap did not echo the expected stop byte.",
                format!("nicFW bootstrap did not echo 4B: {error}"),
            )
        })?;

    check_stop()?;
    proto.send_bytes(&[NICFW_REMOTE_START]).map_err(|error| {
        RemoteSessionFailure::new(
            RemoteSessionFailureKind::RemoteOnAckFailed,
            "Failed to send the nicFW remote-on byte.",
            error.to_string(),
        )
    })?;
    read_required_remote_byte(proto, NICFW_REMOTE_ACK_TIMEOUT, NICFW_REMOTE_START).map_err(
        |error| {
            proto.decoder_boundary_trusted = false;
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

fn wait_for_remote_stop_echo_with<R>(read_next: R, timeout: Duration) -> Result<()>
where
    R: FnMut() -> Result<Option<u8>>,
{
    wait_for_remote_echoes_with(read_next, timeout, NICFW_REMOTE_STOP, 1)
}

fn wait_for_remote_echoes_with<R>(
    read_next: R,
    timeout: Duration,
    expected: u8,
    required_echoes: usize,
) -> Result<()>
where
    R: FnMut() -> Result<Option<u8>>,
{
    wait_for_remote_echoes_with_boundary(read_next, timeout, expected, required_echoes, &mut true)
}

fn wait_for_remote_echoes_with_boundary<R>(
    mut read_next: R,
    timeout: Duration,
    expected: u8,
    required_echoes: usize,
    boundary_trusted: &mut bool,
) -> Result<()>
where
    R: FnMut() -> Result<Option<u8>>,
{
    let echo_label = if expected == NICFW_REMOTE_STOP {
        "remote stop 4B echo"
    } else {
        "remote sync 01 echoes"
    };
    let started = Instant::now();
    let mut observed = Vec::with_capacity(32);
    let mut byte_count = 0;
    let mut echoes = 0;
    let mut payload_remaining = 0;
    let mut text_pending = false;
    let mut text_bytes = 0;
    while started.elapsed() < timeout {
        if byte_count == NICFW_REMOTE_ECHO_MAX_BYTES {
            bail!("{echo_label} exceeded {NICFW_REMOTE_ECHO_MAX_BYTES}-byte drain limit");
        }
        let Some(byte) = read_next().with_context(|| format!("Failed reading {echo_label}"))?
        else {
            continue;
        };
        if started.elapsed() >= timeout {
            *boundary_trusted = false;
            break;
        }
        byte_count += 1;
        if observed.len() < 32 {
            observed.push(byte);
        }
        // Match only an opcode, never a coordinate, color, meter value, or 'K'
        // in text. Lengths mirror RadioProtocol::parse_remote_packet_inner.
        if payload_remaining > 0 {
            payload_remaining -= 1;
            *boundary_trusted = payload_remaining == 0 && !text_pending;
            continue;
        }
        if text_pending {
            if byte == 0 {
                text_pending = false;
                *boundary_trusted = true;
            } else {
                text_bytes += 1;
                if text_bytes > NICFW_REMOTE_TEXT_MAX_BYTES {
                    bail!(
                        "remote text exceeds {NICFW_REMOTE_TEXT_MAX_BYTES} bytes while waiting for {echo_label}"
                    );
                }
            }
            continue;
        }
        match byte {
            byte if byte == expected => {
                echoes += 1;
                if echoes == required_echoes {
                    return Ok(());
                }
            }
            0x64 => {
                *boundary_trusted = false;
                payload_remaining = 7;
                text_pending = true;
                text_bytes = 0;
            }
            0x65..=0x69 => {
                *boundary_trusted = false;
                payload_remaining = match byte {
                    0x65 => 6,
                    0x66 => 7,
                    0x67 | 0x68 => 2,
                    _ => 1,
                };
            }
            0x00
            | NICFW_REMOTE_SYNC_BYTE
            | NICFW_REMOTE_START
            | NICFW_REMOTE_RELEASE
            | 0x70..=0x7F => {}
            _ => {
                *boundary_trusted = false;
                bail!(
                    "unexpected or unknown remote opcode {byte:02X}; cannot confirm {echo_label} at a packet boundary"
                );
            }
        }
    }
    let observed = observed
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    bail!(
        "timed out waiting for {echo_label} after {}ms; {echoes}/{required_echoes} echoes; {byte_count} bytes observed; prefix [{observed}]",
        timeout.as_millis()
    );
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
        if crate::cancellation::is_requested() {
            return Ok(());
        }
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
    proto: &mut RemoteSessionCleanup,
    options: &RemoteSessionOptions,
    log_rx: &Option<mpsc::Receiver<String>>,
    on_event: &mut F,
    summary: &mut RemoteCaptureSummary,
    packet_cache: &mut HashMap<String, String>,
    last_phase: &mut Option<RemoteSessionPhase>,
    live_seen: &mut bool,
    mut should_stop: impl FnMut() -> bool,
) -> RemoteSessionResult<RemoteCommandReaction>
where
    F: FnMut(RemoteCaptureEvent),
{
    let mut reaction = RemoteCommandReaction {
        window_ms: options.command_reaction_window.as_millis(),
        ..RemoteCommandReaction::default()
    };
    let window_started = Instant::now();
    while window_started.elapsed() < options.command_reaction_window && !should_stop() {
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

        let packet = proto
            .parse_remote_packet_with_options(
                options.unknown_capture_limit,
                options.unknown_capture_gap,
            )
            .map_err(|error| {
                RemoteSessionFailure::new(
                    RemoteSessionFailureKind::StreamLost,
                    "Remote packet stream failed while observing command reaction.",
                    format!("Remote reaction window failed: {error}"),
                )
            })?;
        if should_stop() {
            break;
        }
        match packet {
            Some(packet) => {
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
            None => thread::sleep(options.idle_poll_interval),
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
    let mut should_stop = || crate::cancellation::is_requested() || should_stop();
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

        if should_stop() {
            return RemoteControlReport {
                label: command.label.clone(),
                strategy: command.strategy,
                bytes_hex: command.bytes_hex(),
                success: false,
                evidence: RemoteEvidenceKind::CommandFailed,
                reaction: None,
                detail: "cancelled before command write".to_string(),
            };
        }
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
    if *recoveries_left == 0 || crate::cancellation::is_requested() {
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
    if !matches!(packet, RemotePacket::UnknownFrame { .. }) {
        on_event(RemoteCaptureEvent::Received(Instant::now()));
    }
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

    if !observed.unknown && !*live_seen {
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

    // The stream contains unsolicited display/meter updates, with no command
    // identifier. Even changed frequency text may be a scan or a physical key.
    if reaction.deltas > 0
        || reaction.surfaced_packets > 0
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
        assert_eq!(command.steps[0].pause_after, Duration::from_millis(400));
        assert_eq!(command.estimated_duration(), Duration::from_millis(420));
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
    fn stop_echo_ignores_payload_bytes_and_preserves_bytes_after_ack() {
        let mut bytes = vec![0x00, 0x01, 0x4A, 0xFF];
        for (opcode, length) in [(0x65, 6), (0x66, 7), (0x67, 2), (0x68, 2), (0x69, 1)] {
            bytes.push(opcode);
            bytes.extend(vec![0x4B; length]);
        }
        bytes.extend([0x64, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B]);
        bytes.extend(b"K4BK\0");
        bytes.extend(0x70..=0x7F);
        bytes.extend([0x4B, 0x46]);
        let mut bytes = bytes.into_iter();
        wait_for_remote_stop_echo_with(|| Ok(bytes.next()), Duration::from_millis(100)).unwrap();
        assert_eq!(bytes.next(), Some(0x46), "must stop at the single ACK byte");
    }

    #[test]
    fn stop_echo_rejects_payload_only_ack_and_truncated_frames() {
        for bytes in [
            vec![0x67, 0x4B, 0],
            vec![0x65, 0x4B],
            vec![0x64, 0, 0, 0, 0, 0, 0, 0, b'K'],
            vec![0x01, 0x4A, 0xFF],
            vec![],
        ] {
            let mut bytes = bytes.into_iter();
            let started = Instant::now();
            let error =
                wait_for_remote_stop_echo_with(|| Ok(bytes.next()), Duration::from_millis(5))
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("timed out waiting for remote stop 4B")
            );
            assert!(started.elapsed() < Duration::from_secs(1));
        }
    }

    #[test]
    fn stop_echo_fails_closed_on_unknown_frames_and_overlong_text() {
        let mut bytes = [0xE8, 0x4B].into_iter();
        let error = wait_for_remote_stop_echo_with(|| Ok(bytes.next()), Duration::from_secs(1))
            .unwrap_err();
        assert!(error.to_string().contains("unknown remote opcode E8"));
        assert_eq!(bytes.next(), Some(0x4B));

        let mut bytes = vec![0x64, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend(vec![b'K'; NICFW_REMOTE_TEXT_MAX_BYTES + 1]);
        bytes.push(0x4B);
        let mut bytes = bytes.into_iter();
        let error = wait_for_remote_stop_echo_with(|| Ok(bytes.next()), Duration::from_secs(1))
            .unwrap_err();
        assert!(error.to_string().contains("remote text exceeds 256 bytes"));
    }

    #[test]
    fn stop_echo_has_byte_and_time_bounds_and_propagates_transport_errors() {
        let mut reads = 0;
        let error = wait_for_remote_stop_echo_with(
            || {
                reads += 1;
                Ok(Some(0))
            },
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(reads, NICFW_REMOTE_ECHO_MAX_BYTES);
        assert!(error.to_string().contains("drain limit"));

        let error = wait_for_remote_stop_echo_with(
            || {
                thread::sleep(Duration::from_millis(10));
                Ok(Some(0x4B))
            },
            Duration::from_millis(5),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("timed out"),
            "late ACK is not confirmation"
        );

        let error = wait_for_remote_stop_echo_with(
            || Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe).into()),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("Failed reading remote stop 4B echo"));
    }

    #[test]
    fn sync_echoes_count_only_packet_boundaries_and_leave_stop_ack_unread() {
        let mut bytes = Vec::new();
        for _ in 0..NICFW_REMOTE_SYNC_COUNT {
            for (opcode, length) in [(0x65, 6), (0x66, 7), (0x67, 2), (0x68, 2), (0x69, 1)] {
                bytes.push(opcode);
                bytes.extend(vec![0x01; length]);
            }
            bytes.extend([0x64, 1, 1, 1, 1, 1, 1, 1, 1, b'K', 0, 0x70, 0x01]);
        }
        bytes.push(0x4B);
        let mut bytes = bytes.into_iter();
        wait_for_remote_echoes_with(
            || Ok(bytes.next()),
            NICFW_REMOTE_ACK_TIMEOUT,
            NICFW_REMOTE_SYNC_BYTE,
            NICFW_REMOTE_SYNC_COUNT,
        )
        .unwrap();
        assert_eq!(bytes.next(), Some(0x4B));
    }

    #[test]
    fn sync_echoes_fail_closed_and_retain_hard_time_and_byte_limits() {
        for bytes in [
            vec![0x01; 9],
            vec![0x65, 0x01],
            vec![0x64, 0, 0, 0, 0, 0, 0, 0, 1],
            vec![0xE8, 0x01],
        ] {
            let mut bytes = bytes.into_iter();
            let error = wait_for_remote_echoes_with(
                || Ok(bytes.next()),
                Duration::from_millis(5),
                NICFW_REMOTE_SYNC_BYTE,
                NICFW_REMOTE_SYNC_COUNT,
            )
            .unwrap_err();
            assert!(error.to_string().contains("remote sync 01 echoes"));
        }
        let mut reads = 0;
        let error = wait_for_remote_echoes_with(
            || {
                reads += 1;
                Ok(Some(0))
            },
            Duration::from_secs(1),
            NICFW_REMOTE_SYNC_BYTE,
            NICFW_REMOTE_SYNC_COUNT,
        )
        .unwrap_err();
        assert_eq!(reads, NICFW_REMOTE_ECHO_MAX_BYTES);
        assert!(error.to_string().contains("drain limit"));
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
    fn aggregate_deltas_do_not_confirm_command_control() {
        let reaction = RemoteCommandReaction {
            window_ms: 250,
            rx_first_ms: Some(12),
            surfaced_packets: 1,
            unknown_packets: 0,
            deltas: 1,
        };

        assert_eq!(
            classify_remote_evidence(true, Some(&reaction)),
            RemoteEvidenceKind::NoControlEvidence
        );
    }

    #[test]
    fn unsolicited_meter_and_display_changes_are_observations_not_command_responses() {
        let text = |text: &str, y| RemotePacket::DisplayText {
            font_size: 0,
            x: 0,
            y,
            fg_color: 0xFFFF,
            bg_color: 0,
            text: text.to_string(),
        };
        let fixtures = [
            (
                RemotePacket::SignalStrength {
                    strength: 20,
                    mode: 0,
                    battery: 0,
                },
                RemotePacket::SignalStrength {
                    strength: 21,
                    mode: 0,
                    battery: 0,
                },
            ),
            (
                RemotePacket::NoiseLevel { level: 30, mode: 0 },
                RemotePacket::NoiseLevel { level: 31, mode: 0 },
            ),
            (text("6.5V", 70), text("6.6V", 70)),
            (text("145.35000", 20), text("145.36250", 20)),
        ];
        for (before, after) in fixtures {
            let mut cache = HashMap::new();
            assert!(record_remote_packet(&mut cache, &before, true).1.is_none());
            assert!(record_remote_packet(&mut cache, &after, true).1.is_some());
            let reaction = RemoteCommandReaction {
                surfaced_packets: 1,
                deltas: 1,
                ..RemoteCommandReaction::default()
            };
            assert_eq!(
                classify_remote_evidence(true, Some(&reaction)),
                RemoteEvidenceKind::NoControlEvidence,
                "passive update: {after:?}"
            );
        }
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
    fn unknown_bytes_do_not_establish_a_live_decoded_stream() {
        let mut cache = HashMap::new();
        let mut summary = RemoteCaptureSummary::default();
        let mut phase = None;
        let mut live = false;
        let mut events = Vec::new();
        process_remote_packet(
            RemotePacket::UnknownFrame {
                opcode: 1,
                payload: Vec::new(),
            },
            &RemoteSessionOptions::default(),
            &mut cache,
            &mut summary,
            &mut |event| events.push(event),
            &mut phase,
            &mut live,
        );
        assert!(!live);
        assert_eq!(phase, None);
        assert_eq!(summary.unknown_packet_count, 1);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RemoteCaptureEvent::Received(_)))
        );
    }

    #[test]
    fn known_frame_receive_timestamp_precedes_idle_deduplication() {
        let mut cache = HashMap::new();
        let mut summary = RemoteCaptureSummary::default();
        let mut phase = None;
        let mut live = false;
        let mut events = Vec::new();
        let packet = RemotePacket::SmallStatus {
            id: 0x70,
            value1: 0,
            value2: 0,
        };
        let before = Instant::now();
        for _ in 0..2 {
            process_remote_packet(
                packet.clone(),
                &RemoteSessionOptions::default(),
                &mut cache,
                &mut summary,
                &mut |event| events.push(event),
                &mut phase,
                &mut live,
            );
        }
        let after = Instant::now();
        let receives: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let RemoteCaptureEvent::Received(at) = event {
                    Some(*at)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(receives.len(), 2);
        assert!(receives.iter().all(|at| *at >= before && *at <= after));
        assert!(matches!(
            events.first(),
            Some(RemoteCaptureEvent::Received(_))
        ));
        assert_eq!(summary.packet_count, 1);
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
        let first = RemotePacket::NoiseLevel { level: 20, mode: 0 };
        let second = RemotePacket::NoiseLevel { level: 21, mode: 0 };

        let (emit_first, delta_first) = record_remote_packet(&mut cache, &first, true);
        assert!(emit_first);
        assert!(delta_first.is_none());

        let (emit_second, delta_second) = record_remote_packet(&mut cache, &second, true);
        assert!(emit_second);
        assert_eq!(
            delta_second.as_deref(),
            Some("noise_level: NOISE:20:0 -> NOISE:21:0")
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

#[cfg(all(test, unix))]
mod session_tests {
    use super::*;
    use serialport::SerialPort;
    use std::cell::Cell;
    use std::io::{Read, Write};

    fn expect_bytes(radio: &mut serialport::TTYPort, expected: &[u8]) {
        let mut bytes = vec![0; expected.len()];
        radio.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
    }

    fn sync_exchange(radio: &mut serialport::TTYPort) {
        for _ in 0..NICFW_REMOTE_SYNC_COUNT {
            expect_bytes(radio, &[0x01]);
            radio.write_all(&[0x01]).unwrap();
        }
    }

    fn bootstrap(radio: &mut serialport::TTYPort) {
        sync_exchange(radio);
        expect_bytes(radio, &[0x4B]);
        radio.write_all(&[0x4B]).unwrap();
        expect_bytes(radio, &[0x4A]);
    }

    fn expect_silence(radio: &mut serialport::TTYPort, timeout: Duration) {
        radio.set_timeout(timeout).unwrap();
        let mut byte = [0];
        let error = radio
            .read(&mut byte)
            .expect_err("unexpected outbound command");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    fn acknowledge_stop(radio: &mut serialport::TTYPort) {
        expect_bytes(radio, &[0x4B]);
        radio.write_all(&[0x4B]).unwrap();
        expect_silence(radio, Duration::from_millis(100));
    }

    #[test]
    fn bytewise_usb_bootstrap_avoids_sync_drops_when_a_previous_echo_is_pending() {
        for staged_sync in [false, true] {
            let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
            radio.set_timeout(Duration::from_secs(3)).unwrap();
            let port = slave.name().unwrap();
            let worker = thread::spawn(move || {
                let mut dropped = Vec::new();
                for _ in 0..NICFW_REMOTE_SYNC_COUNT {
                    expect_bytes(&mut radio, &[0x01]);
                    radio.set_timeout(Duration::from_millis(20)).unwrap();
                    let mut byte = [0];
                    loop {
                        match radio.read(&mut byte) {
                            Ok(1) => dropped.push(byte[0]),
                            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
                            other => panic!("unexpected sync-stage read: {other:?}"),
                        }
                    }
                    radio.write_all(&[0x01]).unwrap();
                    radio.set_timeout(Duration::from_secs(3)).unwrap();
                    if !dropped.is_empty() {
                        break;
                    }
                }
                expect_bytes(&mut radio, &[0x4B]);
                radio.write_all(&[0x4B]).unwrap();
                if dropped.is_empty() {
                    expect_bytes(&mut radio, &[0x4A]);
                    radio.write_all(&[0x4A]).unwrap();
                    acknowledge_stop(&mut radio);
                } else {
                    expect_silence(&mut radio, Duration::from_millis(100));
                }
                dropped
            });
            let mut proto = RemoteSessionCleanup {
                protocol: RadioProtocol::new(&port).unwrap(),
                stop_required: true,
                enable_required: false,
                decoder_boundary_trusted: true,
            };
            let result = nicfw_remote_on(&mut proto, staged_sync, || false);
            proto.close().unwrap();
            let dropped = worker.join().unwrap();
            if staged_sync {
                result.unwrap();
                assert!(dropped.is_empty());
            } else {
                let mut expected = vec![0x01; NICFW_REMOTE_SYNC_COUNT - 1];
                expected.push(0x4B);
                assert_eq!(dropped, expected);
                assert_eq!(
                    result.unwrap_err().kind,
                    RemoteSessionFailureKind::BootstrapHandshakeFailed
                );
            }
        }
    }

    #[test]
    fn staged_usb_bootstrap_drains_interleaved_old_remote_drawing_frames() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            for _ in 0..NICFW_REMOTE_SYNC_COUNT {
                expect_bytes(&mut radio, &[0x01]);
                radio
                    .write_all(&[0x67, 1, 1, 0x64, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0x01])
                    .unwrap();
            }
            expect_bytes(&mut radio, &[0x4B]);
            radio
                .write_all(&[0x65, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B])
                .unwrap();
            expect_bytes(&mut radio, &[0x4A]);
            radio.write_all(&[0x4A]).unwrap();
            acknowledge_stop(&mut radio);
        });
        let armed = Cell::new(false);
        let summary = run_remote_session(
            &port,
            &RemoteSessionOptions::default(),
            |_| armed.get(),
            |_| None,
            |event| {
                if matches!(event, RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed)) {
                    armed.set(true);
                }
            },
        )
        .unwrap();
        worker.join().unwrap();
        assert_eq!(summary.recovery_count, 0);
    }

    #[test]
    fn staged_usb_sync_failure_never_activates_and_preserves_raw_logs() {
        for (successful_echoes, response) in [
            (0, vec![]),
            (7, vec![]),
            (0, vec![0xE8]),
            (0, vec![0x65, 0x01]),
        ] {
            let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
            radio.set_timeout(Duration::from_secs(3)).unwrap();
            let port = slave.name().unwrap();
            let worker = thread::spawn(move || {
                for _ in 0..successful_echoes {
                    expect_bytes(&mut radio, &[0x01]);
                    radio.write_all(&[0x01]).unwrap();
                }
                expect_bytes(&mut radio, &[0x01]);
                radio.write_all(&response).unwrap();
                // The only stop on sync failure is the existing cleanup, not activation.
                acknowledge_stop(&mut radio);
            });
            let mut logs = Vec::new();
            let failure = run_remote_session(
                &port,
                &RemoteSessionOptions {
                    include_raw_logs: true,
                    ..RemoteSessionOptions::default()
                },
                |_| false,
                |_| None,
                |event| {
                    if let RemoteCaptureEvent::Log(log) = event {
                        logs.push(log);
                    }
                },
            )
            .unwrap_err();
            worker.join().unwrap();
            assert_eq!(
                failure.kind,
                RemoteSessionFailureKind::BootstrapHandshakeFailed
            );
            assert_eq!(
                failure.summary,
                "Remote bootstrap did not echo all ten sync bytes."
            );
            assert!(failure.detail.contains("nicFW bootstrap sync failed"));
            assert!(
                failure
                    .detail
                    .contains(&format!("at byte {}/10", successful_echoes + 1))
            );
            assert_eq!(
                logs.iter().filter(|log| *log == "TX: [01]").count(),
                successful_echoes + 1
            );
            assert!(logs.iter().any(|log| log.starts_with("TX: [01")));
            assert!(logs.iter().any(|log| log.starts_with("RX:")));
            assert!(!logs.iter().any(|log| log == "TX: [4A]"));
            assert_eq!(logs.iter().filter(|log| *log == "TX: [4B]").count(), 1);
        }
    }

    #[test]
    fn staged_usb_bootstrap_keeps_start_ack_strictly_immediate() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            bootstrap(&mut radio);
            radio.write_all(&[0x01, 0x4A]).unwrap();
            acknowledge_stop(&mut radio);
        });
        let failure = run_remote_session(
            &port,
            &RemoteSessionOptions::default(),
            |_| false,
            |_| None,
            |_| {},
        )
        .unwrap_err();
        worker.join().unwrap();
        assert_eq!(failure.kind, RemoteSessionFailureKind::RemoteOnAckFailed);
        assert!(failure.detail.contains("expected 4A, got 01"));
    }

    #[test]
    fn combined_bootstrap_branch_retains_single_write_for_ble() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            let mut expected = vec![0x01; NICFW_REMOTE_SYNC_COUNT];
            expected.push(0x4B);
            expect_bytes(&mut radio, &expected);
            radio.write_all(&[0x4B]).unwrap();
            expect_bytes(&mut radio, &[0x4A]);
            radio.write_all(&[0x4A]).unwrap();
            acknowledge_stop(&mut radio);
        });
        let mut proto = RemoteSessionCleanup {
            protocol: RadioProtocol::new(&port).unwrap(),
            stop_required: true,
            enable_required: false,
            decoder_boundary_trusted: true,
        };
        nicfw_remote_on(&mut proto, false, || false).unwrap();
        proto.close().unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn passive_stop_waits_for_packet_boundary_ack_without_other_commands() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            bootstrap(&mut radio);
            radio.write_all(&[0x4A]).unwrap();
            expect_bytes(&mut radio, &[0x4B]);
            radio
                .write_all(&[0x65, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B, 0x4B])
                .unwrap();
            thread::sleep(Duration::from_millis(80));
            let acknowledged_at = Instant::now();
            radio.write_all(&[0x4B]).unwrap();
            expect_silence(&mut radio, Duration::from_millis(100));
            acknowledged_at
        });
        let mut stopped_at = None;
        let mut logs = Vec::new();
        let armed = Cell::new(false);
        let summary = run_remote_session(
            &port,
            &RemoteSessionOptions {
                include_raw_logs: true,
                ..RemoteSessionOptions::default()
            },
            |_| armed.get(),
            |_| None,
            |event| match event {
                RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed) => armed.set(true),
                RemoteCaptureEvent::Phase(RemoteSessionPhase::Stopped) => {
                    stopped_at = Some(Instant::now())
                }
                RemoteCaptureEvent::Log(log) => logs.push(log),
                _ => {}
            },
        )
        .unwrap();
        let acknowledged_at = worker.join().unwrap();
        assert!(stopped_at.unwrap() >= acknowledged_at);
        assert_eq!(summary.recovery_count, 0);
        assert_eq!(logs.iter().filter(|log| *log == "TX: [4B]").count(), 2);
        assert_eq!(logs.iter().filter(|log| *log == "RX: [4B]").count(), 8);
    }

    #[test]
    fn missing_passive_stop_ack_fails_without_reopening_or_duplicate_cleanup() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            bootstrap(&mut radio);
            radio.write_all(&[0x4A]).unwrap();
            expect_bytes(&mut radio, &[0x4B]);
            // An RSSI value of 4B is not a stop acknowledgment.
            radio.write_all(&[0x67, 0x4B, 0]).unwrap();
            expect_silence(&mut radio, Duration::from_millis(800));
        });
        let mut phases = Vec::new();
        let armed = Cell::new(false);
        let started = Instant::now();
        let failure = run_remote_session(
            &port,
            &RemoteSessionOptions {
                recover_retries: 1,
                ..RemoteSessionOptions::default()
            },
            |_| armed.get(),
            |_| None,
            |event| {
                if let RemoteCaptureEvent::Phase(phase) = event {
                    if phase == RemoteSessionPhase::Armed {
                        armed.set(true);
                    }
                    phases.push(phase);
                }
            },
        )
        .unwrap_err();
        let elapsed = started.elapsed();
        worker.join().unwrap();
        assert_eq!(failure.kind, RemoteSessionFailureKind::StreamLost);
        assert_eq!(failure.summary, "Remote shutdown was not confirmed.");
        assert!(
            failure
                .detail
                .contains("timed out waiting for remote stop 4B")
        );
        assert!(!phases.contains(&RemoteSessionPhase::Stopped));
        assert!(!phases.contains(&RemoteSessionPhase::Recovering));
        assert!(elapsed >= NICFW_REMOTE_ACK_TIMEOUT);
        assert!(elapsed < Duration::from_secs(2));
    }

    #[test]
    fn failed_bootstrap_preserves_raw_logs_and_original_failure_with_cleanup() {
        for cleanup_ack in [true, false] {
            let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
            radio.set_timeout(Duration::from_secs(3)).unwrap();
            let port = slave.name().unwrap();
            let worker = thread::spawn(move || {
                sync_exchange(&mut radio);
                expect_bytes(&mut radio, &[0x4B]); // Withhold the bootstrap stop ACK.
                expect_bytes(&mut radio, &[0x4B]);
                if cleanup_ack {
                    radio.write_all(&[0x4B]).unwrap();
                }
                expect_silence(&mut radio, Duration::from_millis(800));
            });
            let mut logs = Vec::new();
            let failure = run_remote_session(
                &port,
                &RemoteSessionOptions {
                    include_raw_logs: true,
                    ..RemoteSessionOptions::default()
                },
                |_| false,
                |_| None,
                |event| {
                    if let RemoteCaptureEvent::Log(log) = event {
                        logs.push(log);
                    }
                },
            )
            .unwrap_err();
            worker.join().unwrap();
            assert_eq!(
                failure.kind,
                RemoteSessionFailureKind::BootstrapHandshakeFailed
            );
            assert_eq!(
                failure.summary,
                "Remote bootstrap did not echo the expected stop byte."
            );
            assert!(failure.detail.contains("nicFW bootstrap did not echo 4B"));
            assert_eq!(
                failure.detail.contains("remote cleanup failed"),
                !cleanup_ack
            );
            assert_eq!(
                logs.iter().filter(|log| *log == "TX: [01]").count(),
                NICFW_REMOTE_SYNC_COUNT
            );
            assert!(logs.iter().any(|log| log == "RX: [01]"));
            assert!(logs.iter().any(|log| log == "TX: [4B]"));
            assert_eq!(logs.iter().any(|log| log == "RX: [4B]"), cleanup_ack);
            assert!(!logs.iter().any(|log| log == "TX: [4A]"));
        }
    }

    #[test]
    fn failed_bootstrap_drains_raw_logs_before_recovery() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            sync_exchange(&mut radio);
            expect_bytes(&mut radio, &[0x4B]); // Withhold the bootstrap stop ACK.
            expect_bytes(&mut radio, &[0x4B]);
            radio.write_all(&[0x4B]).unwrap();
            bootstrap(&mut radio);
            radio.write_all(&[0x4A]).unwrap();
            acknowledge_stop(&mut radio);
        });
        let mut events = Vec::new();
        let armed = Cell::new(false);
        let summary = run_remote_session(
            &port,
            &RemoteSessionOptions {
                include_raw_logs: true,
                recover_retries: 1,
                ..RemoteSessionOptions::default()
            },
            |_| armed.get(),
            |_| None,
            |event| {
                if matches!(event, RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed)) {
                    armed.set(true);
                }
                events.push(event);
            },
        )
        .unwrap();
        worker.join().unwrap();
        assert_eq!(summary.recovery_count, 1);
        let recovering = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    RemoteCaptureEvent::Phase(RemoteSessionPhase::Recovering)
                )
            })
            .unwrap();
        let before = &events[..recovering];
        for expected in ["RX: [01]", "TX: [4B]", "RX: [4B]"] {
            assert!(
                before
                    .iter()
                    .any(|event| matches!(event, RemoteCaptureEvent::Log(log) if log == expected))
            );
        }
    }

    #[test]
    fn passive_unwind_stops_once_and_waits_for_ack() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            bootstrap(&mut radio);
            radio.write_all(&[0x4A]).unwrap();
            acknowledge_stop(&mut radio);
        });
        let result = std::panic::catch_unwind(|| {
            let armed = Cell::new(false);
            let _ = run_remote_session(
                &port,
                &RemoteSessionOptions::default(),
                |_| {
                    assert!(!armed.get(), "simulated caller unwind");
                    false
                },
                |_| None,
                |event| {
                    if matches!(event, RemoteCaptureEvent::Phase(RemoteSessionPhase::Armed)) {
                        armed.set(true);
                    }
                },
            );
        });
        worker.join().unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn raw_key_session_uses_programmer_tap_hold_before_release() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            bootstrap(&mut radio);
            radio.write_all(&[0x4A]).unwrap();
            expect_bytes(&mut radio, &[0x8A]);
            let pressed_at = Instant::now();
            expect_bytes(&mut radio, &[0xFF]);
            let held = pressed_at.elapsed();
            acknowledge_stop(&mut radio);
            held
        });
        let sent = Cell::new(false);
        let done = Cell::new(false);
        let result = run_remote_session(
            &port,
            &RemoteSessionOptions::default(),
            |elapsed| done.get() || elapsed > Duration::from_secs(2),
            |_| {
                if sent.replace(true) {
                    None
                } else {
                    Some(RemoteControlCommand::raw_key("menu", 0x0B))
                }
            },
            |event| {
                if let RemoteCaptureEvent::Control(report) = event {
                    assert!(report.success);
                    done.set(true);
                }
            },
        );
        let held = worker.join().unwrap();
        result.unwrap();
        assert!(done.get());
        // Allow scheduler latency between receiving the key and timestamping it.
        assert!(held >= Duration::from_millis(350), "wire hold was {held:?}");
    }

    #[test]
    fn passive_wire_updates_during_an_ignored_command_never_confirm_control() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(4)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            bootstrap(&mut radio);
            // Original Remote.cs opcodes: signal, noise, packed LEDs, voltage text.
            radio
                .write_all(&[
                    0x4A, 0x67, 20, 0, 0x68, 30, 0, 0x70, 0x64, 0, 103, 70, 0xFF, 0xFF, 0, 0, b'6',
                    b'.', b'5', b'V', 0,
                ])
                .unwrap();
            expect_bytes(&mut radio, &[0x8A]);
            // Ignore the key. Only the autonomous telemetry changes; these bytes
            // are buffered while the client waits to send the release.
            radio
                .write_all(&[
                    0x67, 21, 0, 0x68, 31, 0, 0x71, 0x64, 0, 103, 70, 0xFF, 0xFF, 0, 0, b'6', b'.',
                    b'6', b'V', 0,
                ])
                .unwrap();
            expect_bytes(&mut radio, &[0xFF, 0x4B]);
            radio.write_all(&[0x4B]).unwrap();
            expect_silence(&mut radio, Duration::from_millis(100));
        });
        let packet_count = Cell::new(0);
        let command_sent = Cell::new(false);
        let done = Cell::new(false);
        let mut report = None;
        let result = run_remote_session(
            &port,
            &RemoteSessionOptions::default(),
            |elapsed| done.get() || elapsed > Duration::from_secs(3),
            |_| {
                if packet_count.get() >= 4 && !command_sent.replace(true) {
                    Some(RemoteControlCommand::raw_key("menu", 0x0B))
                } else {
                    None
                }
            },
            |event| match event {
                RemoteCaptureEvent::Packet(_) => packet_count.set(packet_count.get() + 1),
                RemoteCaptureEvent::Control(control) => {
                    report = Some(control);
                    done.set(true);
                }
                _ => {}
            },
        );
        worker.join().unwrap();
        result.unwrap();
        let report = report.expect("virtual radio should observe the command");
        assert!(report.success);
        assert_eq!(report.evidence, RemoteEvidenceKind::NoControlEvidence);
        assert_eq!(report.reaction.unwrap().deltas, 3);
    }

    #[test]
    fn buffered_nops_do_not_add_half_second_delays() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(4)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            bootstrap(&mut radio);
            radio.write_all(&[0x4A, 0, 0, 0, 0, 0x67, 20, 0]).unwrap();
            acknowledge_stop(&mut radio);
        });
        let done = Cell::new(false);
        let started = Instant::now();
        let result = run_remote_session(
            &port,
            &RemoteSessionOptions::default(),
            |elapsed| done.get() || elapsed > Duration::from_secs(3),
            |_| None,
            |event| {
                if matches!(
                    event,
                    RemoteCaptureEvent::Packet(RemotePacket::SignalStrength { .. })
                ) {
                    done.set(true);
                }
            },
        );
        worker.join().unwrap();
        result.unwrap();
        assert!(done.get());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn failed_remote_start_stops_stream_and_restores_optional_disable() {
        let (mut radio, slave) = serialport::TTYPort::pair().unwrap();
        radio.set_timeout(Duration::from_secs(3)).unwrap();
        let port = slave.name().unwrap();
        let worker = thread::spawn(move || {
            expect_bytes(&mut radio, &[0x01]);
            radio.write_all(&[0x01]).unwrap();
            expect_bytes(&mut radio, &[0x45]);
            radio.write_all(&[0x45]).unwrap();
            bootstrap(&mut radio);
            radio.write_all(&[0x01]).unwrap(); // Wrong remote-on ACK.
            expect_bytes(&mut radio, &[0x4B]);
            radio.write_all(&[0x4B]).unwrap();
            expect_bytes(&mut radio, &[0x46]);
            radio.write_all(&[0x46]).unwrap();
        });
        let result = run_remote_session(
            &port,
            &RemoteSessionOptions {
                disable_radio_before_remote: true,
                ..RemoteSessionOptions::default()
            },
            |_| false,
            |_| None,
            |_| {},
        );
        worker.join().unwrap();
        assert_eq!(
            result.unwrap_err().kind,
            RemoteSessionFailureKind::RemoteOnAckFailed
        );
    }
}
