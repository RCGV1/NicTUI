//! Passive activity capture. The sole transmitted command is the activity ping.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;

use crate::activity::{
    ACTIVITY_BAUD, ACTIVITY_PING, ActivityDecoder, ActivityEvent, DEFAULT_PARTIAL_TIMEOUT,
    TruncatedActivityFrame,
};
use crate::cancellation;
use crate::protocol::radio::RadioProtocol;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default)]
pub struct ActivityMonitorOptions {
    /// Capture time after the validated ACK and Ready notification, not open time.
    pub duration: Option<Duration>,
    pub stop_signal: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone)]
pub enum ActivityMonitorEvent {
    Ready,
    Event(ActivityEvent),
    Warning(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ActivityMonitorSummary {
    pub hit_count: u64,
    pub heartbeat_count: u64,
    pub truncated_frames: u64,
}

fn stopped(options: &ActivityMonitorOptions) -> bool {
    cancellation::is_requested()
        || options
            .stop_signal
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::SeqCst))
}

/// Open serial at 38400 baud, or the existing ble:// transport, without probing,
/// changing radio operation, or configuring Activity Mode. Callers own their logs.
/// Complete pre-ACK records are delivered before Ready. Cancellation is a graceful
/// stop returning the collected summary, including if requested before Ready.
/// Incomplete records are warned about and discarded on timeout or session end.
pub fn monitor_activity(
    port_name: &str,
    options: &ActivityMonitorOptions,
    report: impl FnMut(ActivityMonitorEvent),
) -> Result<ActivityMonitorSummary> {
    if stopped(options) {
        return Ok(ActivityMonitorSummary::default());
    }
    let mut radio = RadioProtocol::new_with_baud(port_name, ACTIVITY_BAUD)
        .with_context(|| format!("Could not open {port_name}"))?;
    if stopped(options) {
        return Ok(ActivityMonitorSummary::default());
    }
    radio
        .send_bytes(&[ACTIVITY_PING])
        .context("Could not send activity ping 02")?;
    receive_activity(|| radio.read_byte(), options, report)
}

fn warn_truncated(
    frame: TruncatedActivityFrame,
    summary: &mut ActivityMonitorSummary,
    report: &mut impl FnMut(ActivityMonitorEvent),
) {
    summary.truncated_frames += 1;
    report(ActivityMonitorEvent::Warning(format!(
        "Discarded incomplete activity frame: {frame}"
    )));
}

fn receive_activity(
    mut read_byte: impl FnMut() -> Result<Option<u8>>,
    options: &ActivityMonitorOptions,
    mut report: impl FnMut(ActivityMonitorEvent),
) -> Result<ActivityMonitorSummary> {
    let handshake_started = Instant::now();
    let mut ready_at: Option<Instant> = None;
    let mut decoder = ActivityDecoder::new(DEFAULT_PARTIAL_TIMEOUT);
    let mut summary = ActivityMonitorSummary::default();
    let result = (|| -> Result<()> {
        loop {
            if stopped(options) {
                return Ok(());
            }
            if let Some(ready) = ready_at {
                if options
                    .duration
                    .is_some_and(|limit| ready.elapsed() >= limit)
                {
                    return Ok(());
                }
            } else if handshake_started.elapsed() >= HANDSHAKE_TIMEOUT {
                bail!("Timed out waiting for activity ping response 02");
            }

            // Expire explicitly before and after the bounded read. Use the same
            // instant for expiry and decoding so push cannot silently lose a warning.
            if let Some(frame) = decoder.expire_partial() {
                warn_truncated(frame, &mut summary, &mut report);
            }
            let byte = read_byte().context("Could not receive radio activity")?;
            let now = Instant::now();
            if let Some(frame) = decoder.expire_partial_at(now) {
                warn_truncated(frame, &mut summary, &mut report);
            }
            match byte {
                Some(ACTIVITY_PING) if ready_at.is_none() && decoder.is_at_frame_boundary() => {
                    report(ActivityMonitorEvent::Ready);
                    ready_at = Some(Instant::now());
                }
                Some(byte) => {
                    for event in decoder.push_timed(&[byte], Utc::now(), now) {
                        match &event {
                            ActivityEvent::Hit { .. } => summary.hit_count += 1,
                            ActivityEvent::Heartbeat { .. } => summary.heartbeat_count += 1,
                        }
                        report(ActivityMonitorEvent::Event(event));
                    }
                }
                // Some transports return None immediately on EOF. Avoid spinning;
                // normal timeouts and cancellation remain bounded by the 50ms read.
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    })();
    if let Some(frame) = decoder.reset() {
        warn_truncated(frame, &mut summary, &mut report);
    }
    result?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_eof_is_reported_without_a_fabricated_event() {
        let mut bytes = [ACTIVITY_PING, 0x2A, ACTIVITY_PING].into_iter();
        let mut reports = Vec::new();
        let error = receive_activity(
            || match bytes.next() {
                Some(byte) => Ok(Some(byte)),
                None => Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
            },
            &ActivityMonitorOptions::default(),
            |event| reports.push(event),
        )
        .unwrap_err();
        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::UnexpectedEof)
        }));
        assert_eq!(reports.len(), 2);
        assert!(matches!(reports[0], ActivityMonitorEvent::Ready));
        assert!(
            matches!(&reports[1], ActivityMonitorEvent::Warning(message) if message.contains("1/5"))
        );
    }
}
