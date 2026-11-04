use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use nictui::activity::{ActivityEvent, ActivityLog, write_activity_csv};
use nictui::activity_session::{ActivityMonitorOptions, ActivityMonitorSummary, monitor_activity};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nictui-activity-session-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn event(frequency_10hz: u32, signal_raw: u8) -> ActivityEvent {
    ActivityEvent::Hit {
        timestamp: chrono::Utc::now(),
        frequency_10hz,
        signal_raw,
    }
}

#[test]
fn options_default_to_unlimited_and_pre_cancel_does_not_open_a_port() {
    let options = ActivityMonitorOptions::default();
    assert!(options.duration.is_none());
    assert!(options.stop_signal.is_none());
    let options = ActivityMonitorOptions {
        stop_signal: Some(Arc::new(AtomicBool::new(true))),
        ..options
    };
    let summary = monitor_activity("not-a-real-device", &options, |_| {
        panic!("no reports before open")
    })
    .unwrap();
    assert_eq!(summary, ActivityMonitorSummary::default());
    assert_eq!(serde_json::to_value(summary).unwrap()["hit_count"], 0);
}

#[test]
fn csv_atomically_replaces_with_exact_headers_frequencies_and_raw_means() {
    let directory = TestDirectory::new();
    let path = directory.0.join("activity.csv");
    std::fs::write(&path, b"previous contents").unwrap();
    let mut log = ActivityLog::new(1);
    for signal in [0, 255, 1] {
        log.record(&event(14_652_001, signal)).unwrap();
    }
    assert!(log.record(&event(1, 42)).is_err());
    let snapshot = log.clone();
    log.record(&event(14_652_001, 255)).unwrap();
    write_activity_csv(&path, &snapshot).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"Frequency,Count,Signal\r\n146.52001,3,085.33\r\n"
    );
    assert_eq!(
        std::fs::read_dir(&directory.0).unwrap().count(),
        1,
        "no temporary file left behind"
    );
    write_activity_csv(&path, &ActivityLog::default()).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"Frequency,Count,Signal\r\n");
}

#[test]
fn csv_failures_preserve_destination_and_remove_temporary_files() {
    let directory = TestDirectory::new();
    let path = directory.0.join("activity.csv");
    std::fs::create_dir(&path).unwrap();
    let sentinel = path.join("existing");
    std::fs::write(&sentinel, b"preserved").unwrap();
    assert!(write_activity_csv(&path, &ActivityLog::default()).is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserved");
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    // Failure creating a same-parent temporary must not truncate an existing file.
    assert!(write_activity_csv(&sentinel.join("child.csv"), &ActivityLog::default()).is_err());
    assert_eq!(std::fs::read(sentinel).unwrap(), b"preserved");
}

#[cfg(unix)]
#[test]
fn csv_temp_creation_failure_does_not_truncate_existing_regular_file() {
    let directory = TestDirectory::new();
    // A valid destination component whose same-parent temp suffix exceeds NAME_MAX.
    let path = directory.0.join("a".repeat(250));
    std::fs::write(&path, b"existing regular file").unwrap();
    assert!(write_activity_csv(&path, &ActivityLog::default()).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"existing regular file");
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
}

#[cfg(unix)]
mod native {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use nictui::activity::{ACTIVITY_PING, DEFAULT_PARTIAL_TIMEOUT};
    use nictui::activity_session::ActivityMonitorEvent;
    use serialport::{SerialPort, TTYPort};

    struct Session {
        master: TTYPort,
        _slave: TTYPort,
        stop: Arc<AtomicBool>,
        events: mpsc::Receiver<ActivityMonitorEvent>,
        completed: mpsc::Receiver<anyhow::Result<ActivityMonitorSummary>>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl Session {
        fn new(duration: Option<Duration>) -> Self {
            let (mut master, slave) = TTYPort::pair().unwrap();
            master.set_timeout(Duration::from_secs(2)).unwrap();
            let port = slave.name().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let options = ActivityMonitorOptions {
                duration,
                stop_signal: Some(stop.clone()),
            };
            let (event_tx, events) = mpsc::channel();
            let (completed_tx, completed) = mpsc::channel();
            let worker = thread::spawn(move || {
                let result = monitor_activity(&port, &options, |event| {
                    let _ = event_tx.send(event);
                });
                let _ = completed_tx.send(result);
            });
            let mut ping = [0];
            master.read_exact(&mut ping).unwrap();
            assert_eq!(ping, [ACTIVITY_PING]);
            master.set_timeout(Duration::from_millis(20)).unwrap();
            Self {
                master,
                _slave: slave,
                stop,
                events,
                completed,
                worker: Some(worker),
            }
        }

        fn send(&mut self, bytes: &[u8]) {
            self.master.write_all(bytes).unwrap();
        }

        fn next(&self) -> ActivityMonitorEvent {
            self.events.recv_timeout(Duration::from_secs(5)).unwrap()
        }

        fn ready(&mut self) {
            self.send(&[ACTIVITY_PING]);
            assert!(matches!(self.next(), ActivityMonitorEvent::Ready));
        }

        fn finish(&mut self) -> anyhow::Result<ActivityMonitorSummary> {
            let result = self.completed.recv_timeout(Duration::from_secs(2)).unwrap();
            self.worker.take().unwrap().join().unwrap();
            let mut bytes = [0; 32];
            let error = self
                .master
                .read(&mut bytes)
                .expect_err("only the initial 02 may be transmitted, including at shutdown");
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            result
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn hit(frequency: u32, signal: u8) -> Vec<u8> {
        let mut bytes = vec![0x2A];
        bytes.extend(frequency.to_le_bytes());
        bytes.push(signal);
        bytes
    }

    #[test]
    fn fragmented_payload_markers_are_not_ack_and_pre_ack_events_are_preserved() {
        let mut session = Session::new(None);
        for byte in hit(0x022B_2A02, ACTIVITY_PING) {
            session.send(&[byte]);
            thread::sleep(Duration::from_millis(2));
        }
        assert!(matches!(
            session.next(),
            ActivityMonitorEvent::Event(ActivityEvent::Hit {
                frequency_10hz: 0x022B_2A02,
                signal_raw: ACTIVITY_PING,
                ..
            })
        ));
        assert!(
            session.events.try_recv().is_err(),
            "payload 02 did not produce Ready"
        );
        session.send(&[0x2B]);
        assert!(matches!(
            session.next(),
            ActivityMonitorEvent::Event(ActivityEvent::Heartbeat { .. })
        ));
        session.ready();
        let mut bytes = hit(14_550_000, 43);
        bytes.push(0x2B);
        session.send(&bytes);
        assert!(matches!(
            session.next(),
            ActivityMonitorEvent::Event(ActivityEvent::Hit {
                frequency_10hz: 14_550_000,
                signal_raw: 43,
                ..
            })
        ));
        assert!(matches!(
            session.next(),
            ActivityMonitorEvent::Event(ActivityEvent::Heartbeat { .. })
        ));
        session.stop.store(true, Ordering::SeqCst);
        assert_eq!(
            session.finish().unwrap(),
            ActivityMonitorSummary {
                hit_count: 2,
                heartbeat_count: 2,
                truncated_frames: 0
            }
        );
        assert!(session.events.try_recv().is_err());
    }

    #[test]
    fn duration_starts_after_ack_not_at_port_open() {
        let duration = Duration::from_millis(100);
        let mut session = Session::new(Some(duration));
        thread::sleep(Duration::from_millis(120));
        let ack_sent = Instant::now();
        session.ready();
        assert_eq!(session.finish().unwrap(), ActivityMonitorSummary::default());
        assert!(ack_sent.elapsed() >= duration);
        assert!(ack_sent.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn handshake_cancellation_is_bounded_without_ready_or_cleanup_commands() {
        let mut session = Session::new(None);
        let started = Instant::now();
        session.stop.store(true, Ordering::SeqCst);
        assert_eq!(session.finish().unwrap(), ActivityMonitorSummary::default());
        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(session.events.try_recv().is_err());
    }

    #[test]
    fn partial_capture_cancellation_warns_once_without_inventing_a_hit() {
        let mut session = Session::new(None);
        session.ready();
        session.send(&[0x2A, 0x02, 0x2B]);
        thread::sleep(Duration::from_millis(30));
        let started = Instant::now();
        session.stop.store(true, Ordering::SeqCst);
        assert_eq!(
            session.finish().unwrap(),
            ActivityMonitorSummary {
                truncated_frames: 1,
                ..ActivityMonitorSummary::default()
            }
        );
        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(
            matches!(session.next(), ActivityMonitorEvent::Warning(message) if message.contains("2/5"))
        );
        assert!(session.events.try_recv().is_err());
    }

    #[test]
    fn handshake_failure_rejects_normal_ping_and_payload_02_and_reports_partial() {
        let mut session = Session::new(Some(Duration::ZERO));
        let mut bytes = vec![0x01, 0x45, 0x46];
        bytes.extend(hit(0x022B_2A02, ACTIVITY_PING));
        bytes.extend([0x2A, ACTIVITY_PING]);
        session.send(&bytes);
        assert!(matches!(
            session.next(),
            ActivityMonitorEvent::Event(ActivityEvent::Hit { .. })
        ));
        let started = Instant::now();
        let error = session.finish().unwrap_err();
        assert!(error.to_string().contains("response 02"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            matches!(session.next(), ActivityMonitorEvent::Warning(message) if message.contains("1/5"))
        );
        assert!(session.events.try_recv().is_err());
    }

    #[test]
    fn partial_timeout_warns_resets_and_allows_fresh_frames() {
        let mut session = Session::new(None);
        session.ready();
        session.send(&[0x2A, ACTIVITY_PING]);
        let started = Instant::now();
        assert!(
            matches!(session.next(), ActivityMonitorEvent::Warning(message) if message.contains("1/5"))
        );
        assert!(started.elapsed() >= DEFAULT_PARTIAL_TIMEOUT);
        session.send(&[0x2B]);
        assert!(matches!(
            session.next(),
            ActivityMonitorEvent::Event(ActivityEvent::Heartbeat { .. })
        ));
        session.send(&hit(1, 10));
        assert!(matches!(
            session.next(),
            ActivityMonitorEvent::Event(ActivityEvent::Hit {
                frequency_10hz: 1,
                signal_raw: 10,
                ..
            })
        ));
        session.stop.store(true, Ordering::SeqCst);
        assert_eq!(
            session.finish().unwrap(),
            ActivityMonitorSummary {
                hit_count: 1,
                heartbeat_count: 1,
                truncated_frames: 1
            }
        );
        assert!(session.events.try_recv().is_err());
    }

    #[test]
    fn duration_end_reports_partial_frame() {
        let mut session = Session::new(Some(Duration::from_millis(100)));
        session.ready();
        session.send(&[0x2A]);
        assert_eq!(session.finish().unwrap().truncated_frames, 1);
        assert!(
            matches!(session.next(), ActivityMonitorEvent::Warning(message) if message.contains("0/5"))
        );
        assert!(session.events.try_recv().is_err());
    }
}
