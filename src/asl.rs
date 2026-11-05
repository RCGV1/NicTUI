//! Strict, unframed nicFW USB COS reports. No radio control or audio operations.

use std::fmt;
use std::time::SystemTime;

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CosState {
    #[default]
    Unknown,
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CosSnapshot {
    pub state: CosState,
    pub open_reports: u64,
    pub closed_reports: u64,
    pub transitions: u64,
    pub last_report_at: Option<SystemTime>,
    pub last_transition_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CosReport {
    pub at: SystemTime,
    pub previous: CosState,
    pub state: CosState,
    pub transitioned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UnexpectedByte {
    pub byte: u8,
    /// Zero-based stream offset, including NOP bytes.
    pub offset: u64,
}

impl fmt::Display for UnexpectedByte {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Unexpected ASL byte 0x{:02X} at offset {}; stopped without resynchronizing",
            self.byte, self.offset
        )
    }
}

impl std::error::Error for UnexpectedByte {}

/// A byte decoder shared by live capture and offline replay.
///
/// Configure ASL USB manually and exclude Activity/Remote streams first. This
/// unframed format cannot identify payloads consisting only of 00/10/11, such as
/// when capture starts mid-payload. A decoded report is not a health handshake.
#[derive(Debug, Default)]
pub struct CosDecoder {
    snapshot: CosSnapshot,
    offset: u64,
    failure: Option<UnexpectedByte>,
}

impl CosDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode one byte with a caller-supplied host timestamp. After an invalid
    /// byte, every call returns the original error; no later payload is decoded.
    pub fn feed(&mut self, byte: u8, at: SystemTime) -> Result<Option<CosReport>, UnexpectedByte> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let offset = self.offset;
        self.offset = self.offset.saturating_add(1);
        let state = match byte {
            0x00 => return Ok(None),
            0x10 => CosState::Closed,
            0x11 => CosState::Open,
            _ => {
                let error = UnexpectedByte { byte, offset };
                self.snapshot.state = CosState::Unknown;
                self.failure = Some(error);
                return Err(error);
            }
        };
        let previous = self.snapshot.state;
        let transitioned = previous != CosState::Unknown && previous != state;
        if state == CosState::Open {
            self.snapshot.open_reports = self.snapshot.open_reports.saturating_add(1);
        } else {
            self.snapshot.closed_reports = self.snapshot.closed_reports.saturating_add(1);
        }
        if transitioned {
            self.snapshot.transitions = self.snapshot.transitions.saturating_add(1);
            self.snapshot.last_transition_at = Some(at);
        }
        self.snapshot.state = state;
        self.snapshot.last_report_at = Some(at);
        Ok(Some(CosReport {
            at,
            previous,
            state,
            transitioned,
        }))
    }

    pub fn snapshot(&self) -> CosSnapshot {
        self.snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn unknown_and_nop_do_not_claim_a_state_or_timestamp() {
        let mut decoder = CosDecoder::new();
        assert_eq!(decoder.snapshot(), CosSnapshot::default());
        assert_eq!(decoder.feed(0, at(1)), Ok(None));
        assert_eq!(decoder.snapshot(), CosSnapshot::default());
    }

    #[test]
    fn initial_reports_duplicates_and_real_transitions_have_distinct_counts() {
        for (first, other, initial) in
            [(0x10, 0x11, CosState::Closed), (0x11, 0x10, CosState::Open)]
        {
            let mut decoder = CosDecoder::new();
            let report = decoder.feed(first, at(1)).unwrap().unwrap();
            assert_eq!(report.previous, CosState::Unknown);
            assert_eq!(report.state, initial);
            assert!(!report.transitioned);
            assert_eq!(decoder.snapshot().last_transition_at, None);
            assert!(!decoder.feed(first, at(2)).unwrap().unwrap().transitioned);
            assert!(decoder.feed(other, at(3)).unwrap().unwrap().transitioned);
            assert!(!decoder.feed(other, at(4)).unwrap().unwrap().transitioned);
            assert!(decoder.feed(first, at(5)).unwrap().unwrap().transitioned);
            assert_eq!(decoder.feed(0, at(6)), Ok(None));
            let snapshot = decoder.snapshot();
            assert_eq!(snapshot.state, initial);
            assert_eq!(snapshot.open_reports + snapshot.closed_reports, 5);
            assert_eq!(snapshot.transitions, 2);
            assert_eq!(snapshot.last_report_at, Some(at(5)));
            assert_eq!(snapshot.last_transition_at, Some(at(5)));
        }
    }

    #[test]
    fn every_unknown_nonzero_byte_is_terminal_even_with_cos_in_later_payload() {
        for byte in 1..=u8::MAX {
            if matches!(byte, 0x10 | 0x11) {
                continue;
            }
            let mut decoder = CosDecoder::new();
            decoder.feed(0, at(0)).unwrap();
            decoder.feed(0x11, at(1)).unwrap();
            let error = UnexpectedByte { byte, offset: 2 };
            assert_eq!(decoder.feed(byte, at(2)), Err(error));
            let snapshot = decoder.snapshot();
            assert_eq!(snapshot.state, CosState::Unknown);
            assert_eq!(snapshot.open_reports, 1);
            assert_eq!(snapshot.last_report_at, Some(at(1)));
            for embedded in [0, 0x10, 0x11, byte] {
                assert_eq!(decoder.feed(embedded, at(3)), Err(error));
                assert_eq!(decoder.snapshot(), snapshot);
            }
        }
    }

    #[test]
    fn state_snapshot_and_report_serialize_without_custom_time_support() {
        let mut decoder = CosDecoder::new();
        let report = decoder.feed(0x10, at(1)).unwrap().unwrap();
        assert_eq!(serde_json::to_value(report).unwrap()["state"], "closed");
        let snapshot = serde_json::to_value(decoder.snapshot()).unwrap();
        assert_eq!(snapshot["state"], "closed");
        assert_eq!(snapshot["last_report_at"]["secs_since_epoch"], 1);
        assert_eq!(snapshot["transitions"], 0);
    }
}
