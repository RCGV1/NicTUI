use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::KeyCode;

use super::navigation::{next_item, prev_item};
use super::{App, AppMode};
use crate::activity::{ActivityEvent, ActivityLog, write_activity_csv};
use crate::activity_session::{ActivityMonitorEvent, ActivityMonitorOptions, monitor_activity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityStatus {
    Ready,
    Listening,
    Waiting,
    Stopped,
    Error,
}

impl ActivityStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Listening => "Listening",
            Self::Waiting => "Waiting",
            Self::Stopped => "Stopped",
            Self::Error => "Error",
        }
    }
}

#[derive(Clone)]
pub struct ActivitySnapshot {
    pub log: ActivityLog,
    pub status: ActivityStatus,
    pub hit_count: u64,
    pub heartbeat_count: u64,
    pub truncated_frames: u64,
    pub latest_signal_raw: Option<u8>,
    pub notice: String,
    pub stopping: bool,
    last_received: Option<Instant>,
    changed: bool,
    failure_detail: Option<String>,
}

impl Default for ActivitySnapshot {
    fn default() -> Self {
        Self {
            log: ActivityLog::new(4096),
            status: ActivityStatus::Ready,
            hit_count: 0,
            heartbeat_count: 0,
            truncated_frames: 0,
            latest_signal_raw: None,
            notice: String::new(),
            stopping: false,
            last_received: None,
            changed: true,
            failure_detail: None,
        }
    }
}

impl ActivitySnapshot {
    fn receive(&mut self, event: ActivityMonitorEvent) -> bool {
        if self.failure_detail.is_some() {
            return false;
        }
        match event {
            ActivityMonitorEvent::Ready => {
                self.status = ActivityStatus::Ready;
                self.last_received = Some(Instant::now());
            }
            ActivityMonitorEvent::Event(event) => {
                if let Err(error) = self.log.record(&event) {
                    self.status = ActivityStatus::Error;
                    self.notice =
                        "Capture stopped: log limit reached. Export or clear before restarting."
                            .into();
                    self.failure_detail = Some(error.to_string());
                    self.changed = true;
                    return false;
                }
                match &event {
                    ActivityEvent::Hit { signal_raw, .. } => {
                        self.hit_count = self.hit_count.saturating_add(1);
                        self.latest_signal_raw = Some(*signal_raw);
                    }
                    ActivityEvent::Heartbeat { .. } => {
                        self.heartbeat_count = self.heartbeat_count.saturating_add(1);
                    }
                }
                self.last_received = Some(Instant::now());
                self.status = ActivityStatus::Listening;
            }
            ActivityMonitorEvent::Warning(_) => {
                self.notice = "Incomplete activity data received; capture continues.".into();
            }
        }
        self.changed = true;
        true
    }
}

// A separate snapshot per session prevents late updates from entering a new capture.
pub(crate) struct ActivityWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<()>>>,
}

impl Drop for ActivityWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl App {
    pub fn activity_active(&self) -> bool {
        self.activity_worker.is_some()
    }

    pub fn start_activity(&mut self) {
        if self.activity_active() {
            self.status_message =
                "Activity still owns the radio; stop and wait before restarting.".into();
            return;
        }
        if self.remote_active
            || self.remote_tx.is_some()
            || self.ble_scan_in_progress
            || matches!(
                self.mode,
                AppMode::Reading | AppMode::Writing | AppMode::BinFlashing
            )
        {
            self.status_message =
                "Wait for the current radio operation to finish before Activity.".into();
            return;
        }
        let port = if self.demo_mode {
            String::new()
        } else {
            let Some(port) = self.active_protocol_port() else {
                self.status_message = "Select a radio target before starting Activity.".into();
                return;
            };
            port
        };
        self.activity = Arc::new(Mutex::new(ActivitySnapshot {
            status: ActivityStatus::Waiting,
            ..Default::default()
        }));
        self.activity_state.select(None);
        if self.demo_mode {
            self.populate_demo_activity();
            self.activity.lock().unwrap().status = ActivityStatus::Listening;
        }
        let snapshot = self.activity.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let demo = self.demo_mode;
        let handle = thread::spawn(move || {
            if demo {
                while !worker_stop.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(10));
                }
                return Ok(());
            }
            let options = ActivityMonitorOptions {
                duration: None,
                stop_signal: Some(worker_stop.clone()),
            };
            let summary = monitor_activity(&port, &options, |event| {
                if !snapshot.lock().unwrap().receive(event) {
                    worker_stop.store(true, Ordering::SeqCst);
                }
            })?;
            let mut state = snapshot.lock().unwrap();
            state.truncated_frames = summary.truncated_frames;
            state.changed = true;
            Ok(())
        });
        self.activity_worker = Some(ActivityWorker {
            stop,
            handle: Some(handle),
        });
        self.status_message =
            "Activity capture owns the radio. Stop capture before other radio actions.".into();
    }

    pub fn stop_activity(&mut self) {
        if let Some(worker) = &self.activity_worker {
            worker.stop.store(true, Ordering::SeqCst);
            let mut state = self.activity.lock().unwrap();
            state.stopping = true;
            state.changed = true;
            self.status_message = "Stopping Activity; waiting for radio cleanup.".into();
        }
    }

    pub(crate) fn poll_activity(&mut self) -> bool {
        let finished = self
            .activity_worker
            .as_ref()
            .and_then(|worker| worker.handle.as_ref())
            .is_some_and(JoinHandle::is_finished);
        if finished {
            let mut worker = self.activity_worker.take().unwrap();
            let result = worker.handle.take().unwrap().join();
            let detail = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("{error:#}")),
                Err(_) => Some("Activity worker exited unexpectedly".into()),
            };
            let mut state = self.activity.lock().unwrap();
            let aggregation_failed = state.failure_detail.is_some();
            let detail = state.failure_detail.take().or(detail);
            let success = detail.is_none();
            state.status = if success {
                ActivityStatus::Stopped
            } else {
                ActivityStatus::Error
            };
            state.stopping = false;
            state.changed = true;
            if !success && !aggregation_failed {
                state.notice =
                    "Capture failed. Check the connection and manual radio setup.".into();
            } else if success
                && state.notice == "Incomplete activity data received; capture continues."
            {
                state.notice = "Capture stopped; incomplete activity data was discarded.".into();
            }
            drop(state);
            if let Some(detail) = detail {
                let detail: String = detail
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(512)
                    .collect();
                self.log(&format!("Activity capture failed: {detail}"));
            }
            self.status_message = if success {
                "Activity stopped; radio released."
            } else {
                "Activity capture failed; radio released."
            }
            .into();
        }
        let mut state = self.activity.lock().unwrap();
        if self.activity_active()
            && !state.stopping
            && state
                .last_received
                .is_some_and(|last| last.elapsed() >= Duration::from_secs(5))
            && matches!(
                state.status,
                ActivityStatus::Ready | ActivityStatus::Listening
            )
        {
            state.status = ActivityStatus::Waiting;
            state.changed = true;
        }
        let changed = std::mem::take(&mut state.changed);
        let len = state.log.rows().count();
        if self
            .activity_state
            .selected()
            .is_some_and(|selected| selected >= len)
        {
            self.activity_state.select(None);
        }
        changed
    }

    pub fn clear_activity(&mut self) {
        if self.activity_active() {
            self.status_message =
                "Stop Activity and wait for cleanup before clearing the log.".into();
            return;
        }
        self.activity = Default::default();
        self.activity_state = Default::default();
        self.status_message = "Activity log cleared.".into();
    }

    pub fn export_activity(&self, path: &Path) -> Result<()> {
        let log = self.activity.lock().unwrap().log.clone();
        write_activity_csv(path, &log)
    }

    pub fn pick_activity_export_file(&mut self) {
        let mut path = None;
        self.show_save_dialog(
            "Export Activity CSV",
            "activity.csv",
            &[("CSV", &["csv"])],
            |selected| path = selected,
        );
        if let Some(path) = path {
            self.status_message = match self.export_activity(&path) {
                Ok(()) => "Activity CSV saved.".into(),
                Err(_) => {
                    "Could not save Activity CSV. Check the destination and try again.".into()
                }
            };
        }
    }

    pub fn handle_activity_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('s') | KeyCode::Enter => {
                if self.activity_active() {
                    self.stop_activity();
                } else {
                    self.start_activity();
                }
            }
            KeyCode::Char('c') => self.clear_activity(),
            KeyCode::Char('e') => self.pick_activity_export_file(),
            KeyCode::Up | KeyCode::Down => {
                let len = self.activity.lock().unwrap().log.rows().count();
                if key == KeyCode::Up {
                    prev_item(len, &mut self.activity_state);
                } else {
                    next_item(len, &mut self.activity_state);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn populate_demo_activity(&mut self) {
        let mut state = self.activity.lock().unwrap();
        for (frequency_10hz, signal_raw) in [
            (14_652_000, 112),
            (44_650_000, 87),
            (14_652_000, 124),
            (16_255_000, 96),
        ] {
            state.receive(ActivityMonitorEvent::Event(ActivityEvent::Hit {
                timestamp: chrono::Utc::now(),
                frequency_10hz,
                signal_raw,
            }));
        }
        state.receive(ActivityMonitorEvent::Event(ActivityEvent::Heartbeat {
            timestamp: chrono::Utc::now(),
        }));
        state.status = ActivityStatus::Stopped;
        state.notice = "Demo sample data; no hardware I/O.".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(frequency_10hz: u32) -> ActivityMonitorEvent {
        ActivityMonitorEvent::Event(ActivityEvent::Hit {
            timestamp: chrono::Utc::now(),
            frequency_10hz,
            signal_raw: 100,
        })
    }

    #[test]
    fn capacity_failure_preserves_log_and_counts_only_recorded_hits() {
        let mut state = ActivitySnapshot {
            log: ActivityLog::new(1),
            ..Default::default()
        };
        assert!(state.receive(hit(14_652_000)));
        assert!(!state.receive(hit(44_650_000)));
        assert_eq!(state.hit_count, 1);
        assert_eq!(state.log.rows().count(), 1);
        assert_eq!(state.status, ActivityStatus::Error);
        assert!(state.notice.contains("log limit"));
        assert!(!state.receive(hit(14_652_000)));
        assert_eq!(state.hit_count, 1);
    }

    #[test]
    fn repeated_hits_are_coalesced_without_app_events() {
        let mut state = ActivitySnapshot::default();
        for _ in 0..100_000 {
            assert!(state.receive(hit(14_652_000)));
        }
        assert_eq!(state.hit_count, 100_000);
        assert_eq!(state.log.rows().count(), 1);
        assert_eq!(state.log.rows().next().unwrap().count, 100_000);
    }

    #[test]
    fn idle_capture_becomes_waiting_and_heartbeat_restores_listening() {
        let mut app = App::demo();
        app.start_activity();
        app.activity.lock().unwrap().last_received = Some(Instant::now() - Duration::from_secs(6));
        assert!(app.poll_activity());
        assert_eq!(app.activity.lock().unwrap().status, ActivityStatus::Waiting);
        app.activity
            .lock()
            .unwrap()
            .receive(ActivityMonitorEvent::Event(ActivityEvent::Heartbeat {
                timestamp: chrono::Utc::now(),
            }));
        assert_eq!(
            app.activity.lock().unwrap().status,
            ActivityStatus::Listening
        );
    }

    #[test]
    fn worker_failure_releases_port_and_logs_one_sanitized_detail() {
        let mut app = App::new();
        app.activity
            .lock()
            .unwrap()
            .receive(ActivityMonitorEvent::Warning("partial data".into()));
        app.activity_worker = Some(ActivityWorker {
            stop: Arc::new(AtomicBool::new(false)),
            handle: Some(thread::spawn(|| {
                anyhow::bail!("Receive failed\n\u{1b}[31m connection lost")
            })),
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.activity_active() && Instant::now() < deadline {
            app.poll_activity();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.activity_active());
        assert_eq!(app.activity.lock().unwrap().status, ActivityStatus::Error);
        assert!(
            app.activity
                .lock()
                .unwrap()
                .notice
                .contains("Check the connection")
        );
        assert_eq!(app.logs.len(), 1);
        assert!(app.logs[0].contains("Receive failed"));
        assert!(!app.logs[0].chars().any(char::is_control));
        app.poll_activity();
        assert_eq!(app.logs.len(), 1);
    }
}
