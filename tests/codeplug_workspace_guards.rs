use std::{
    io,
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

use nictui::app::{App, AppEvent, AppMode, MainTab};
use serde_json::{Value, json};

type Edit = (&'static str, fn(&mut App));

fn edits() -> Vec<Edit> {
    vec![
        ("dirty channels", |app| app.channels_dirty = true),
        ("dirty settings", |app| app.settings_dirty = true),
        ("dirty scan presets", |app| app.scan_presets_dirty = true),
        ("dirty band plans", |app| app.band_plans_dirty = true),
        ("dirty DTMF", |app| app.dtmf_dirty = true),
        ("dirty group labels", |app| app.group_labels_dirty = true),
        ("deleted channels", |app| app.deleted_channels.push(7)),
        ("unflagged channels", |app| {
            app.channels[0].name = "LOCAL".into()
        }),
        ("unflagged settings", |app| {
            app.settings.as_mut().unwrap().step += 1
        }),
        ("unflagged scan presets", |app| {
            app.scan_presets[0].start_freq += 1
        }),
        ("unflagged band plans", |app| {
            app.band_plans[0].end_freq += 1
        }),
        ("unflagged DTMF", |app| app.dtmf_presets[0].digits.push(1)),
        ("unflagged group labels", |app| {
            app.group_labels[0] = "LOCAL".into()
        }),
        ("unflagged endianness", |app| {
            app.endian = nictui::protocol::Endianness::Little
        }),
        ("channel draft", |app| {
            app.pending_channel_edit = Some(app.channels[0].clone());
            app.pending_channel_edit.as_mut().unwrap().name = "DRAFT".into();
            app.mode = AppMode::EditChannel(0);
        }),
        ("settings draft", |app| {
            app.edit_buffer = "123".into();
            app.mode = AppMode::EditSetting(0);
        }),
        ("DTMF draft", |app| {
            app.pending_dtmf_edit = Some(app.dtmf_presets[0].clone());
            app.pending_dtmf_edit.as_mut().unwrap().digits.push(2);
            app.dtmf_edit_preset_idx = Some(0);
            app.mode = AppMode::EditDTMF(1);
        }),
        ("scan preset draft", |app| {
            app.editing_scan_preset = Some(app.scan_presets[0].clone());
            app.editing_scan_preset.as_mut().unwrap().start_freq += 2;
            app.mode = AppMode::EditScanPreset(0);
        }),
        ("band plan draft", |app| {
            app.editing_band_plan = Some(app.band_plans[0].clone());
            app.editing_band_plan.as_mut().unwrap().end_freq += 2;
            app.mode = AppMode::EditBandPlan(0);
        }),
        ("group label draft", |app| {
            app.editing_group_label_idx = Some(0);
            app.edit_buffer = "DRAFT".into();
            app.mode = AppMode::EditGroupLabel(0);
        }),
    ]
}

fn seeded() -> App {
    // Local fixtures only: App::demo does not discover or connect to a device.
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Codeplug);
    app.last_main_tab = MainTab::Codeplug;
    app.last_non_remote_tab = MainTab::Codeplug;
    app.demo_mode = false;
    app.simulator_mode = true;
    app.protocol_port_name = None;
    app.active_port_candidate = None;
    app.port_candidates.clear();
    app.ports.clear();
    app.serial_port_candidates.clear();
    app.ble_port_candidates.clear();
    app.channel_state.select(Some(2));
    app.preset_state.select(Some(1));
    app.selection_index = 3;
    app.edit_buffer = "retained buffer".into();
    assert!(app.codeplug_snapshot_is_current());
    app
}

fn retained(app: &App) -> Value {
    json!({
        "records": {
            "channels": app.channels,
            "settings": app.settings,
            "scan_presets": app.scan_presets,
            "band_plans": app.band_plans,
            "dtmf": app.dtmf_presets,
            "group_labels": app.group_labels,
            "deleted_channels": app.deleted_channels,
            "endian": app.endian,
        },
        "dirty": [app.channels_dirty, app.settings_dirty, app.scan_presets_dirty,
            app.band_plans_dirty, app.dtmf_dirty, app.group_labels_dirty],
        "drafts": {
            "channel": app.pending_channel_edit,
            "dtmf": app.pending_dtmf_edit,
            "dtmf_index": app.dtmf_edit_preset_idx,
            "scan_preset": app.editing_scan_preset,
            "band_plan": app.editing_band_plan,
            "group_index": app.editing_group_label_idx,
            "buffer": app.edit_buffer,
            "selection": app.selection_index,
        },
        "selected": [app.channel_state.selected(), app.settings_state.selected(),
            app.preset_state.selected(), app.bandplan_state.selected(),
            app.dtmf_state.selected(), app.scanning_group_state.selected()],
        "snapshot": app.codeplug_data,
        "snapshot_path": app.codeplug_path,
        "snapshot_baseline": app.codeplug_workspace_baseline,
    })
}

fn hold_tracked_completion(app: &mut App) -> AppEvent {
    // Codeplug reads delegate to this same tracked machinery. Explicit Read All
    // supports an in-memory demo source; ordinary demo Codeplug reads stay blocked.
    app.demo_mode = true;
    app.simulator_mode = false;
    app.start_read_all();
    assert_eq!(app.mode, AppMode::Reading, "tracked read must start");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let event = app
            .event_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("in-memory full read did not produce its completion");
        match event {
            event @ AppEvent::CodeplugDataLoaded { .. } => return event,
            AppEvent::Status(_) | AppEvent::Progress(_) | AppEvent::Log(_) => {}
            AppEvent::ReadAllFailed(error) => panic!("demo full read failed: {error}"),
            _ => panic!("unexpected event while staging a tracked full read"),
        }
    }
}

fn assert_no_connection(listener: &TcpListener, case: &str) {
    // Only our ephemeral loopback listener is inspected; no connection is serviced.
    let deadline = Instant::now() + Duration::from_millis(60);
    loop {
        match listener.accept() {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Ok((stream, _)) => {
                drop(stream);
                panic!("{case}: blocked full read connected to the owned endpoint");
            }
            Err(error) => panic!("{case}: inspecting owned listener failed: {error}"),
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn ordinary_full_read_rejects_every_dirty_domain_unflagged_edit_and_draft_before_connect() {
    for (case, edit) in edits() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut app = seeded();
        app.protocol_port_name = Some(format!("sim://{}", listener.local_addr().unwrap()));
        edit(&mut app);
        let before = retained(&app);
        let mode = app.mode.clone();
        let progress = app.progress;

        app.start_read_codeplug();

        assert_no_connection(&listener, case);
        assert_eq!(app.mode, mode, "{case}: blocked read changed mode");
        assert_eq!(
            app.progress, progress,
            "{case}: blocked read reset progress"
        );
        assert!(
            retained(&app) == before,
            "{case}: blocked read changed local state"
        );
    }
}

#[test]
fn queued_tracked_full_read_preserves_edits_made_after_it_was_queued() {
    // Main blocks editor entry while a read is active. Draft rejection is covered
    // before start; here exercise late dirty flags and record/baseline changes.
    for (case, edit) in edits()
        .into_iter()
        .filter(|(case, _)| !case.ends_with("draft"))
    {
        let mut app = seeded();
        let completion = hold_tracked_completion(&mut app);
        // The worker's terminal event has been captured before any edit occurs.
        // Requeue it without updating, so the real request remains active.
        app.event_tx.send(completion).unwrap();
        edit(&mut app);
        let before = retained(&app);

        assert!(app.update());

        assert!(
            retained(&app) == before,
            "{case}: queued read overwrote local state"
        );
        assert_eq!(
            app.mode,
            AppMode::Main(MainTab::Codeplug),
            "{case}: rejected completion did not return to Codeplug"
        );
        assert!(
            app.status_message.to_ascii_lowercase().contains("preserv"),
            "{case}: skipped read did not explain edit preservation: {}",
            app.status_message,
        );
    }
}

#[test]
fn unchanged_tracked_read_applies_and_returns_to_codeplug_with_or_without_a_baseline() {
    for has_baseline in [true, false] {
        let mut app = seeded();
        if !has_baseline {
            app.codeplug_workspace_baseline = None;
        }
        let completion = hold_tracked_completion(&mut app);
        let expected_data = match &completion {
            AppEvent::CodeplugDataLoaded { data, .. } => data.clone(),
            _ => unreachable!(),
        };
        app.event_tx.send(completion).unwrap();

        app.update();

        assert_eq!(app.mode, AppMode::Main(MainTab::Codeplug));
        assert_eq!(app.progress, 1.0);
        assert_eq!(app.codeplug_data.as_ref(), Some(&expected_data));
        assert!(app.codeplug_snapshot_is_current());
        assert!(
            app.status_message
                .to_ascii_lowercase()
                .contains("all radio configuration loaded")
        );
    }
}

#[test]
fn failed_tracked_full_read_does_not_clear_edits_or_make_the_old_snapshot_current() {
    for (case, edit) in edits()
        .into_iter()
        .filter(|(case, _)| !case.ends_with("draft"))
    {
        let mut app = seeded();
        // Replace the captured success with failure while retaining the real request.
        drop(hold_tracked_completion(&mut app));
        edit(&mut app);
        let before = retained(&app);
        assert!(
            !app.codeplug_snapshot_is_current(),
            "{case}: fixture must be changed"
        );
        app.event_tx
            .send(AppEvent::ReadAllFailed(
                "Failed to read full snapshot: injected transport failure".into(),
            ))
            .unwrap();

        app.update();

        assert!(
            matches!(app.mode, AppMode::Error(ref error) if error.contains("injected transport failure"))
        );
        assert!(
            retained(&app) == before,
            "{case}: failed read changed local state"
        );
        assert!(
            !app.codeplug_snapshot_is_current(),
            "{case}: failed read falsely marked snapshot current"
        );
    }
}
