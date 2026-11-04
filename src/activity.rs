//! Passive nicFW activity records: 2B heartbeat, or 2A + LE u32 (10 Hz) + raw signal.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Serialize;

pub const ACTIVITY_BAUD: u32 = 38_400;
pub const ACTIVITY_PING: u8 = 0x02;
pub const DEFAULT_PARTIAL_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_MAX_FREQUENCIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityEvent {
    Heartbeat {
        #[serde(serialize_with = "serialize_timestamp")]
        timestamp: DateTime<Utc>,
    },
    Hit {
        #[serde(serialize_with = "serialize_timestamp")]
        timestamp: DateTime<Utc>,
        frequency_10hz: u32,
        signal_raw: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncatedActivityFrame {
    /// Payload bytes received after the 2A marker, out of five required bytes.
    pub received: usize,
}

impl fmt::Display for TruncatedActivityFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "truncated activity hit: received {}/5 payload bytes",
            self.received
        )
    }
}

impl std::error::Error for TruncatedActivityFrame {}

/// Retains at most five payload bytes, independent of input size. There is no
/// checksum or escaping: marker bytes inside a payload are ordinary data.
#[derive(Debug)]
pub struct ActivityDecoder {
    payload: [u8; 5],
    received: Option<usize>,
    last_payload_at: Option<Instant>,
    partial_timeout: Duration,
}

impl Default for ActivityDecoder {
    fn default() -> Self {
        Self::new(DEFAULT_PARTIAL_TIMEOUT)
    }
}

impl ActivityDecoder {
    pub fn new(partial_timeout: Duration) -> Self {
        Self {
            payload: [0; 5],
            received: None,
            last_payload_at: None,
            partial_timeout,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<ActivityEvent> {
        self.push_at(bytes, Utc::now())
    }

    /// Events use the host receive time of the chunk completing the record.
    pub fn push_at(&mut self, bytes: &[u8], timestamp: DateTime<Utc>) -> Vec<ActivityEvent> {
        self.push_timed(bytes, timestamp, Instant::now())
    }

    pub(crate) fn push_timed(
        &mut self,
        bytes: &[u8],
        timestamp: DateTime<Utc>,
        now: Instant,
    ) -> Vec<ActivityEvent> {
        self.expire_partial_at(now);
        let mut events = Vec::new();
        for &byte in bytes {
            if let Some(received) = self.received {
                self.payload[received] = byte;
                self.last_payload_at = Some(now);
                if received + 1 == self.payload.len() {
                    events.push(ActivityEvent::Hit {
                        timestamp,
                        frequency_10hz: u32::from_le_bytes([
                            self.payload[0],
                            self.payload[1],
                            self.payload[2],
                            self.payload[3],
                        ]),
                        signal_raw: self.payload[4],
                    });
                    self.received = None;
                    self.last_payload_at = None;
                } else {
                    self.received = Some(received + 1);
                }
            } else {
                match byte {
                    0x2A => {
                        self.received = Some(0);
                        self.last_payload_at = Some(now);
                    }
                    0x2B => events.push(ActivityEvent::Heartbeat { timestamp }),
                    _ => {}
                }
            }
        }
        events
    }

    /// Call on read timeouts too; push also expires stale payloads automatically.
    pub fn expire_partial(&mut self) -> Option<TruncatedActivityFrame> {
        self.expire_partial_at(Instant::now())
    }

    pub fn expire_partial_at(&mut self, now: Instant) -> Option<TruncatedActivityFrame> {
        let last = self.last_payload_at?;
        if now.checked_duration_since(last)? >= self.partial_timeout {
            self.reset()
        } else {
            None
        }
    }

    pub fn reset(&mut self) -> Option<TruncatedActivityFrame> {
        self.last_payload_at = None;
        self.received
            .take()
            .map(|received| TruncatedActivityFrame { received })
    }

    /// Marker-like payload bytes must not be interpreted as protocol replies.
    pub(crate) fn is_at_frame_boundary(&self) -> bool {
        self.received.is_none()
    }

    /// Report an incomplete record at end of input, and clear it for reuse.
    pub fn finish(&mut self) -> std::result::Result<(), TruncatedActivityFrame> {
        match self.reset() {
            Some(truncated) => Err(truncated),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivitySummary {
    pub frequency_10hz: u32,
    pub count: u64,
    pub signal_sum_raw: u64,
    #[serde(serialize_with = "serialize_timestamp")]
    pub first_seen: DateTime<Utc>,
    #[serde(serialize_with = "serialize_timestamp")]
    pub last_seen: DateTime<Utc>,
}

fn serialize_timestamp<S: serde::Serializer>(
    timestamp: &DateTime<Utc>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&timestamp.to_rfc3339())
}

impl ActivitySummary {
    pub fn mean_signal_raw(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.signal_sum_raw as f64 / self.count as f64
        }
    }
}

/// Bounded summaries only; individual events are delivered to the caller.
#[derive(Debug, Clone)]
pub struct ActivityLog {
    summaries: BTreeMap<u32, ActivitySummary>,
    max_frequencies: usize,
}

impl Default for ActivityLog {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FREQUENCIES)
    }
}

impl ActivityLog {
    pub fn new(max_frequencies: usize) -> Self {
        Self {
            summaries: BTreeMap::new(),
            max_frequencies,
        }
    }

    pub fn record(&mut self, event: &ActivityEvent) -> Result<()> {
        let ActivityEvent::Hit {
            timestamp,
            frequency_10hz,
            signal_raw,
        } = event
        else {
            return Ok(());
        };
        if let Some(summary) = self.summaries.get_mut(frequency_10hz) {
            let Some(count) = summary.count.checked_add(1) else {
                bail!("activity count overflow");
            };
            let Some(sum) = summary.signal_sum_raw.checked_add(u64::from(*signal_raw)) else {
                bail!("activity raw signal sum overflow");
            };
            summary.count = count;
            summary.signal_sum_raw = sum;
            summary.first_seen = summary.first_seen.min(*timestamp);
            summary.last_seen = summary.last_seen.max(*timestamp);
        } else {
            if self.summaries.len() >= self.max_frequencies {
                bail!(
                    "activity frequency limit ({}) reached",
                    self.max_frequencies
                );
            }
            self.summaries.insert(
                *frequency_10hz,
                ActivitySummary {
                    frequency_10hz: *frequency_10hz,
                    count: 1,
                    signal_sum_raw: u64::from(*signal_raw),
                    first_seen: *timestamp,
                    last_seen: *timestamp,
                },
            );
        }
        Ok(())
    }

    pub fn rows(&self) -> impl Iterator<Item = &ActivitySummary> {
        self.summaries.values()
    }

    /// Programmer-compatible CSV: MHz, hit count, mean raw signal (not dBm).
    pub fn write_csv<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(b"Frequency,Count,Signal\r\n")?;
        for summary in self.rows() {
            // Round the exact integer sum only for presentation, not per hit.
            let count = u128::from(summary.count);
            let hundredths = (u128::from(summary.signal_sum_raw) * 100 + count / 2) / count;
            write!(
                writer,
                "{},{},{:03}.{:02}\r\n",
                format_frequency_mhz(summary.frequency_10hz),
                summary.count,
                hundredths / 100,
                hundredths % 100
            )?;
        }
        writer.flush()
    }
}

/// Atomically replace a CSV only after the complete new file has been flushed.
/// Temporary files are created exclusively in the destination's parent directory.
pub fn write_activity_csv(path: &Path, log: &ActivityLog) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TemporaryCsv(Option<PathBuf>);
    impl Drop for TemporaryCsv {
        fn drop(&mut self) {
            if let Some(path) = &self.0 {
                let _ = fs::remove_file(path);
            }
        }
    }

    let name = path.file_name().context("Activity CSV needs a file name")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (mut temporary, file) = (0..128)
        .find_map(|_| {
            let mut temporary_name = std::ffi::OsString::from(".");
            temporary_name.push(name);
            temporary_name.push(format!(
                ".activity-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let temporary = parent.join(temporary_name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => Some(Ok((TemporaryCsv(Some(temporary)), file))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "activity CSV temporary names exhausted",
            ))
        })
        .with_context(|| {
            format!(
                "Could not create temporary activity CSV for {}",
                path.display()
            )
        })?;
    let mut writer = io::BufWriter::new(file);
    log.write_csv(&mut writer)
        .with_context(|| format!("Could not write activity CSV {}", path.display()))?;
    writer.flush().context("Could not flush activity CSV")?;
    writer
        .get_ref()
        .sync_all()
        .context("Could not sync activity CSV")?;
    drop(writer);
    fs::rename(temporary.0.as_ref().expect("temporary CSV is armed"), path)
        .with_context(|| format!("Could not replace activity CSV {}", path.display()))?;
    temporary.0.take();
    Ok(())
}

pub fn format_frequency_mhz(frequency_10hz: u32) -> String {
    format!(
        "{}.{:05}",
        frequency_10hz / 100_000,
        frequency_10hz % 100_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn hit(frequency_10hz: u32, signal_raw: u8) -> ActivityEvent {
        ActivityEvent::Hit {
            timestamp: timestamp(),
            frequency_10hz,
            signal_raw,
        }
    }

    #[test]
    fn all_fragment_splits_preserve_little_endian_hits() {
        let bytes = [0x2A, 0x12, 0x34, 0x56, 0x78, 255];
        for split in 0..=bytes.len() {
            let mut decoder = ActivityDecoder::default();
            let mut events = decoder.push_at(&bytes[..split], timestamp());
            events.extend(decoder.push_at(&bytes[split..], timestamp()));
            assert_eq!(events, vec![hit(0x78563412, 255)], "split {split}");
            assert!(decoder.finish().is_ok());
        }
    }

    #[test]
    fn coalesced_noise_and_payload_markers() {
        let mut decoder = ActivityDecoder::default();
        let events = decoder.push_at(
            &[
                0, 2, 255, 0x2B, 0x2A, 0x2A, 0x2B, 0x2A, 0x2B, 0x2B, 0, 0x2A, 0, 0, 0, 0, 0x2A,
                0x2B,
            ],
            timestamp(),
        );
        assert_eq!(
            events,
            vec![
                ActivityEvent::Heartbeat {
                    timestamp: timestamp()
                },
                hit(0x2B2A2B2A, 0x2B),
                hit(0, 0x2A),
                ActivityEvent::Heartbeat {
                    timestamp: timestamp()
                },
            ]
        );
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn idle_expiry_prevents_splicing() {
        let now = Instant::now();
        let mut decoder = ActivityDecoder::default();
        assert!(
            decoder
                .push_timed(&[0x2A, 1, 2], timestamp(), now)
                .is_empty()
        );
        let later = now + DEFAULT_PARTIAL_TIMEOUT;
        let events = decoder.push_timed(&[0x2B, 0x2A, 3, 0, 0, 0, 4], timestamp(), later);
        assert_eq!(
            events,
            vec![
                ActivityEvent::Heartbeat {
                    timestamp: timestamp()
                },
                hit(3, 4)
            ]
        );
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn truncation_is_reported_and_resets() {
        for received in 0..5 {
            let mut decoder = ActivityDecoder::default();
            decoder.push_at(&[0x2A, 1, 2, 3, 4][..received + 1], timestamp());
            assert_eq!(decoder.finish(), Err(TruncatedActivityFrame { received }));
            assert_eq!(decoder.finish(), Ok(()));
            assert_eq!(decoder.push_at(&[0x2B], timestamp()).len(), 1);
        }
    }

    #[test]
    fn aggregation_and_csv_are_exact() {
        let mut log = ActivityLog::default();
        log.record(&hit(14_652_001, 0)).unwrap();
        log.record(&hit(14_652_001, 255)).unwrap();
        log.record(&hit(14_652_001, 1)).unwrap();
        log.record(&hit(1, 3)).unwrap();
        log.record(&ActivityEvent::Heartbeat {
            timestamp: timestamp(),
        })
        .unwrap();
        let rows: Vec<_> = log.rows().collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].count, 3);
        assert_eq!(rows[1].signal_sum_raw, 256);
        assert_eq!(rows[1].mean_signal_raw(), 256.0 / 3.0);
        let mut csv = Vec::new();
        log.write_csv(&mut csv).unwrap();
        assert_eq!(
            String::from_utf8(csv).unwrap(),
            "Frequency,Count,Signal\r\n0.00001,1,003.00\r\n146.52001,3,085.33\r\n"
        );
    }

    #[test]
    fn every_partition_of_mixed_stream_decodes_identically() {
        let bytes = [
            0x2B, 0x2A, 0x2A, 0x2B, 0, 128, 0x2A, 0x2A, 255, 255, 255, 255, 255,
        ];
        let expected = vec![
            ActivityEvent::Heartbeat {
                timestamp: timestamp(),
            },
            hit(0x80002B2A, 0x2A),
            hit(u32::MAX, 255),
        ];
        for partition in 0..1usize << (bytes.len() - 1) {
            let mut decoder = ActivityDecoder::default();
            let mut events = Vec::new();
            let mut start = 0;
            for end in 1..=bytes.len() {
                if end == bytes.len() || partition & (1 << (end - 1)) != 0 {
                    events.extend(decoder.push_at(&bytes[start..end], timestamp()));
                    start = end;
                }
            }
            assert_eq!(events, expected, "partition {partition}");
            assert_eq!(decoder.finish(), Ok(()));
        }
    }

    #[test]
    fn expiry_is_based_on_last_payload_byte_not_wall_clock() {
        let now = Instant::now();
        let mut decoder = ActivityDecoder::default();
        decoder.push_timed(&[0x2A], timestamp(), now);
        assert_eq!(
            decoder.expire_partial_at(now + Duration::from_secs(2)),
            None
        );
        decoder.push_timed(&[1], timestamp(), now + Duration::from_secs(2));
        assert_eq!(
            decoder.expire_partial_at(now + Duration::from_secs(4)),
            None
        );
        assert_eq!(
            decoder.expire_partial_at(now + Duration::from_secs(5)),
            Some(TruncatedActivityFrame { received: 1 })
        );
        assert_eq!(
            decoder.expire_partial_at(now + Duration::from_secs(6)),
            None
        );
        assert_eq!(decoder.finish(), Ok(()));
    }

    #[test]
    fn reset_and_empty_chunks_do_not_create_events() {
        let mut decoder = ActivityDecoder::default();
        assert!(decoder.push(&[]).is_empty());
        assert!(decoder.push(&[0x2A, 1]).is_empty());
        assert!(decoder.push(&[]).is_empty());
        assert_eq!(
            decoder.reset(),
            Some(TruncatedActivityFrame { received: 1 })
        );
        assert_eq!(decoder.reset(), None);
        assert_eq!(decoder.push(&[0x2B]).len(), 1);
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn timestamp_is_from_completing_chunk_and_extrema_are_preserved() {
        let later = timestamp() + chrono::Duration::seconds(1);
        let mut decoder = ActivityDecoder::default();
        decoder.push_at(&[0x2A, 1, 0], timestamp());
        let events = decoder.push_at(&[0, 0, 10], later);
        assert_eq!(
            events,
            vec![ActivityEvent::Hit {
                timestamp: later,
                frequency_10hz: 1,
                signal_raw: 10,
            }]
        );
        let mut log = ActivityLog::default();
        log.record(&events[0]).unwrap();
        log.record(&hit(1, 20)).unwrap();
        let row = log.rows().next().unwrap();
        assert_eq!(row.first_seen, timestamp());
        assert_eq!(row.last_seen, later);
        assert_eq!(row.mean_signal_raw(), 15.0);
    }

    #[test]
    fn frequency_capacity_rejects_new_hits_without_losing_existing_rows() {
        let mut log = ActivityLog::new(1);
        log.record(&hit(1, 10)).unwrap();
        assert!(log.record(&hit(2, 20)).is_err());
        log.record(&hit(1, 30)).unwrap();
        assert_eq!(log.rows().count(), 1);
        assert_eq!(log.rows().next().unwrap().count, 2);
        let mut zero = ActivityLog::new(0);
        zero.record(&ActivityEvent::Heartbeat {
            timestamp: timestamp(),
        })
        .unwrap();
        assert!(zero.record(&hit(1, 1)).is_err());
        assert_eq!(zero.rows().count(), 0);
    }

    #[test]
    fn aggregation_overflow_does_not_partially_mutate_summary() {
        let mut log = ActivityLog::default();
        log.record(&hit(1, 10)).unwrap();
        for (count, sum) in [(u64::MAX, 10), (1, u64::MAX)] {
            let row = log.summaries.get_mut(&1).unwrap();
            row.count = count;
            row.signal_sum_raw = sum;
            let before = row.clone();
            assert!(log.record(&hit(1, 1)).is_err());
            assert_eq!(log.rows().next(), Some(&before));
        }
    }

    #[test]
    fn csv_rounds_exact_rational_mean_and_preserves_full_u32_frequency() {
        let mut log = ActivityLog::default();
        for signal in [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] {
            log.record(&hit(u32::MAX, signal)).unwrap();
        }
        let mut csv = Vec::new();
        log.write_csv(&mut csv).unwrap();
        assert_eq!(
            String::from_utf8(csv).unwrap(),
            "Frequency,Count,Signal\r\n42949.67295,16,000.13\r\n"
        );
    }

    #[test]
    fn empty_csv_and_writer_failures() {
        let log = ActivityLog::default();
        let mut csv = Vec::new();
        log.write_csv(&mut csv).unwrap();
        assert_eq!(csv, b"Frequency,Count,Signal\r\n");
        assert!(log.write_csv(&mut [0u8; 1][..]).is_err());
    }

    #[test]
    fn events_and_summary_serialize_timestamps_without_chrono_serde_feature() {
        let event = hit(14_652_000, 100);
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["kind"], "hit");
        assert_eq!(value["timestamp"], timestamp().to_rfc3339());
        assert_eq!(value["signal_raw"], 100);
        let mut log = ActivityLog::default();
        log.record(&event).unwrap();
        let value = serde_json::to_value(log.rows().next().unwrap()).unwrap();
        assert_eq!(value["first_seen"], timestamp().to_rfc3339());
        assert_eq!(value["last_seen"], timestamp().to_rfc3339());
    }
}
