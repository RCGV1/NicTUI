use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use nictui::{
    app::{App, AppMode, MainTab, activity::ActivityStatus},
    ui,
};
use ratatui::{Terminal, backend::TestBackend};

fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::ui(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_stopped(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while app.activity_active() && Instant::now() < deadline {
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.activity_active());
}

#[test]
fn activity_renders_responsively_and_keeps_manual_setup_order() {
    for (width, height) in [(80, 24), (100, 30), (160, 45)] {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Activity);
        let output = render(&mut app, width, height);
        for label in [
            "Activity [A]",
            "Stopped",
            "FrequencyMHz",
            "Hits",
            "MeanRaw",
            "LastSeen",
            "146.52000",
            "Heartbeats",
            "Latest raw",
            "s start",
            "e CSV",
        ] {
            assert!(
                output.contains(label),
                "missing {label} at {width}x{height}:\n{output}"
            );
        }
        assert!(output.contains("Start a VFO preset scan, then enable Activity Mode"));
        assert!(!output.contains("signal_raw"));
    }
    for (width, height) in [(1, 1), (8, 3), (20, 8), (40, 12), (60, 18)] {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Activity);
        render(&mut app, width, height);
    }
}

#[test]
fn activity_empty_state_distinguishes_scan_heartbeats_from_no_data() {
    for (width, height) in [(100, 30), (140, 38)] {
        for heartbeats in [0, 12] {
            let mut app = App::demo();
            app.mode = AppMode::Main(MainTab::Activity);
            app.start_activity();
            {
                let mut state = app.activity.lock().unwrap();
                *state = Default::default();
                state.heartbeat_count = heartbeats;
                state.status = if heartbeats > 0 {
                    ActivityStatus::Listening
                } else {
                    ActivityStatus::Waiting
                };
            }

            let output = render(&mut app, width, height);
            let running = "Radio scan is running. No signal hits received yet.";
            let waiting = "Waiting for radio activity. Check manual setup if no hits arrive.";
            let (expected, absent) = if heartbeats > 0 {
                (running, waiting)
            } else {
                (waiting, running)
            };
            assert!(
                output.contains(expected),
                "{heartbeats} heartbeats at {width}x{height}:\n{output}"
            );
            assert!(!output.contains(absent));
            assert!(output.contains(&format!("Hits 0  Heartbeats {heartbeats}  Latest raw --")));
            assert!(!output.contains("FrequencyMHz"));
            let state = app.activity.lock().unwrap();
            assert_eq!(state.hit_count, 0);
            assert_eq!(state.log.rows().count(), 0);
            assert!(state.latest_signal_raw.is_none());
            drop(state);
            app.stop_activity();
            wait_stopped(&mut app);
        }
    }
}

#[test]
fn activity_is_discoverable_in_sidebar_from_other_pages() {
    for (width, height) in [(80, 24), (100, 27), (100, 28), (100, 29)] {
        let mut app = App::demo();
        let output = render(&mut app, width, height);
        assert!(
            output.contains("A. Activity"),
            "{width}x{height}:\n{output}"
        );
        assert!(output.contains("0. Logs"));
        assert!(output.contains("1. Channels"));
    }
}

#[test]
fn activity_navigation_selection_start_stop_and_clear_are_safe() {
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Debug);
    app.next_tab();
    assert_eq!(app.mode, AppMode::Main(MainTab::Activity));
    app.next_tab();
    assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
    app.prev_tab();
    assert_eq!(app.mode, AppMode::Main(MainTab::Activity));
    app.handle_activity_key(KeyCode::Down);
    assert_eq!(app.activity_state.selected(), Some(0));
    app.handle_activity_key(KeyCode::Up);
    assert_eq!(app.activity_state.selected(), Some(2));
    app.handle_activity_key(KeyCode::Char('s'));
    assert!(app.activity_active());
    let session = app.activity.clone();
    app.start_activity();
    assert!(std::sync::Arc::ptr_eq(&session, &app.activity));
    app.clear_activity();
    assert_eq!(app.activity.lock().unwrap().log.rows().count(), 3);
    app.next_tab();
    assert!(app.activity_active());
    app.stop_activity();
    assert!(
        app.activity_active(),
        "stop request must not release ownership"
    );
    wait_stopped(&mut app);
    assert_eq!(app.activity.lock().unwrap().status, ActivityStatus::Stopped);
    app.clear_activity();
    assert_eq!(app.activity.lock().unwrap().log.rows().count(), 0);
    app.start_activity();
    assert!(!std::sync::Arc::ptr_eq(&session, &app.activity));
    session.lock().unwrap().hit_count = 900;
    assert_ne!(app.activity.lock().unwrap().hit_count, 900);
    app.stop_activity();
    wait_stopped(&mut app);
}

#[test]
fn capture_blocks_all_conflicting_radio_entry_points_until_completion() {
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Activity);
    app.start_activity();
    // The worker was started in hardware-free demo mode; guards are also tested
    // with demo protection removed so Activity alone must prevent physical I/O.
    app.demo_mode = false;
    app.channels_dirty = true;
    app.settings_dirty = true;
    app.dtmf_dirty = true;
    app.group_labels_dirty = true;
    app.scan_presets_dirty = true;
    app.band_plans_dirty = true;
    let port = app.protocol_port_name.clone();
    let index = app.selected_port_index;
    for action in [
        App::remote_on,
        App::start_read_channels,
        App::start_read_presets,
        App::start_read_bandplan,
        App::start_read_dtmf,
        App::start_read_settings,
        App::start_write_settings_and_reboot,
        App::start_write_dirty_channels,
        App::start_write_dtmf,
        App::start_write_scan_presets,
        App::start_write_band_plans,
        App::start_write_group_labels,
        App::start_read_codeplug,
        App::start_write_codeplug,
        App::start_bin_flash,
        App::select_port,
        App::refresh_ports,
        App::refresh_radio_targets_from_tui,
        App::start_ble_scan_from_tui,
        App::next_port,
        App::previous_port,
    ] {
        action(&mut app);
        assert!(app.activity_active());
        assert!(!app.remote_active);
        assert_eq!(app.protocol_port_name, port);
        assert_eq!(app.selected_port_index, index);
        assert!(
            app.status_message.contains("Activity"),
            "{}",
            app.status_message
        );
    }
    app.connect_to_port_by_name("must-not-open");
    app.start_ble_scan(true);
    app.send_key(0x0A);
    assert!(!app.remote_controls_ready());
    assert_eq!(app.protocol_port_name, port);
    app.stop_activity();
    app.remote_on();
    assert!(!app.remote_active);
    wait_stopped(&mut app);
}

#[test]
fn remote_or_an_existing_operation_prevents_capture_start() {
    for mode in [
        AppMode::Reading,
        AppMode::Writing,
        AppMode::BinFlashing,
        AppMode::Main(MainTab::Remote),
    ] {
        let mut app = App::demo();
        app.remote_active = matches!(mode, AppMode::Main(MainTab::Remote));
        app.mode = mode;
        app.start_activity();
        assert!(!app.activity_active());
    }
}

#[test]
fn csv_export_uses_shared_format_and_leaves_capture_running() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "nictui-activity-tui-{}-{}.csv",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut app = App::demo();
    app.start_activity();
    app.export_activity(&path).unwrap();
    let csv = std::fs::read_to_string(&path).unwrap();
    assert!(csv.starts_with("Frequency,Count,Signal\r\n"));
    assert!(csv.contains("146.52000,2,118.00"));
    assert!(app.activity_active());
    std::fs::remove_file(path).unwrap();
    app.stop_activity();
    wait_stopped(&mut app);
}
