use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

fn wait_for_read(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.read_all_request.is_some() && Instant::now() < deadline {
        app.update();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(app.read_all_request.is_none(), "Read All did not finish");
}

fn app_with_fake_port() -> App {
    let mut app = App::new();
    app.mode = AppMode::Main(MainTab::Settings);
    app.protocol_port_name = Some("test://read-all-no-hardware".into());
    app
}

fn mixed_snapshot() -> Vec<u8> {
    use crate::protocol::RadioProtocol;
    let demo = App::demo();
    let mut data = demo.codeplug_data.unwrap();
    data[240 * crate::protocol::BLOCK_SIZE] = 0x57;
    let settings =
        RadioProtocol::pack_settings_block(demo.settings.as_ref().unwrap(), Endianness::Little);
    data[0x1900..0x1900 + settings.len()].copy_from_slice(&settings);
    for preset in &demo.scan_presets {
        let offset = 0x1B00 + usize::from(preset.index) * 20;
        data[offset..offset + 20]
            .copy_from_slice(&RadioProtocol::pack_scan_preset(preset, Endianness::Little));
    }
    for plan in &demo.band_plans {
        let offset = 0x1A02 + usize::from(plan.index) * 10;
        let mut plan = plan.clone();
        plan.start_freq *= 100;
        plan.end_freq *= 100;
        data[offset..offset + 10]
            .copy_from_slice(&RadioProtocol::pack_bandplan(&plan, Endianness::Little));
    }
    data
}

#[test]
fn read_all_stages_one_mixed_endian_snapshot_and_applies_every_section_atomically() {
    use crate::protocol::codeplug::*;
    let mut app = app_with_fake_port();
    let data = mixed_snapshot();
    assert_eq!(infer_endianness_from_codeplug(&data), Endianness::Little);
    assert_eq!(
        infer_channel_endianness_from_codeplug(&data),
        Endianness::Big
    );
    let original = app.workspace_snapshot();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let bytes = data.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel();
    app.start_read_all_with(move |_, report| {
        counter.fetch_add(1, Ordering::SeqCst);
        report(ProgressEvent::Progress(1.0));
        started_tx.send(()).unwrap();
        finish_rx.recv().unwrap();
        Ok((bytes, Endianness::Little))
    });
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::Reading);
    assert_eq!(app.workspace_snapshot(), original);
    assert!(app.progress < 1.0);
    assert!(app.codeplug_data.is_none());
    finish_tx.send(()).unwrap();
    wait_for_read(&mut app);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.mode, AppMode::Main(MainTab::Settings));
    assert_eq!(app.endian, Endianness::Big);
    assert_eq!(app.settings.as_ref().unwrap().step, 500);
    assert_eq!(app.scan_presets[0].start_freq, 144_000);
    assert_eq!(app.band_plans[0].start_freq, 13_600_000);
    assert_eq!(app.channels[0].rx_freq, "145.33500");
    assert_eq!(
        serde_json::to_value(&app.channels).unwrap(),
        serde_json::to_value(extract_channels_from_codeplug(&data, Endianness::Big)).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&app.settings).unwrap(),
        serde_json::to_value(extract_settings_from_codeplug(&data, Endianness::Big)).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&app.scan_presets).unwrap(),
        serde_json::to_value(extract_scan_presets_from_codeplug(&data, Endianness::Big)).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&app.band_plans).unwrap(),
        serde_json::to_value(extract_band_plans_from_codeplug(&data, Endianness::Big)).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&app.dtmf_presets).unwrap(),
        serde_json::to_value(extract_dtmf_presets_from_codeplug(&data)).unwrap()
    );
    assert_eq!(app.group_labels, extract_group_labels_from_codeplug(&data));
    assert_eq!(app.codeplug_data.as_deref(), Some(data.as_slice()));
    assert!(app.codeplug_snapshot_is_current());
    assert!(!app.configuration_has_unsaved_edits());
    assert_eq!(app.progress, 1.0);
    assert_eq!(app.status_message, "All radio configuration loaded");
    for selected in [
        app.channel_state.selected(),
        app.settings_state.selected(),
        app.preset_state.selected(),
        app.scanning_group_state.selected(),
        app.bandplan_state.selected(),
        app.dtmf_state.selected(),
    ] {
        assert_eq!(selected, Some(0));
    }
}

#[test]
fn read_all_refuses_every_dirty_section_deletion_and_pending_edit_before_read() {
    let edits: &[fn(&mut App)] = &[
        |app| app.channels_dirty = true,
        |app| app.settings_dirty = true,
        |app| app.scan_presets_dirty = true,
        |app| app.group_labels_dirty = true,
        |app| app.band_plans_dirty = true,
        |app| app.dtmf_dirty = true,
        |app| app.deleted_channels.push(1),
        |app| app.pending_channel_edit = Some(app.channels[0].clone()),
        |app| app.pending_dtmf_edit = Some(app.dtmf_presets[0].clone()),
        |app| app.editing_scan_preset = Some(app.scan_presets[0].clone()),
        |app| app.editing_band_plan = Some(app.band_plans[0].clone()),
        |app| app.editing_group_label_idx = Some(0),
    ];
    for edit in edits {
        let mut app = App::demo();
        edit(&mut app);
        let before = app.workspace_snapshot();
        let raw = app.codeplug_data.clone();
        let baseline = app.codeplug_workspace_baseline.clone();
        app.start_read_all_with(|_, _| panic!("dirty read must not start"));
        assert_eq!(app.workspace_snapshot(), before);
        assert_eq!(app.codeplug_data, raw);
        assert_eq!(app.codeplug_workspace_baseline, baseline);
        assert!(app.status_message.contains("unsaved edits"));
        assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
        assert!(app.read_all_request.is_none());
        assert!(app.event_rx.try_recv().is_err());
    }
}

#[test]
fn read_all_blocks_busy_discovery_remote_cleanup_and_unselected_radio() {
    for mode in [
        AppMode::Reading,
        AppMode::Writing,
        AppMode::BinFlashing,
        AppMode::PortSelection,
        AppMode::EditSetting(0),
    ] {
        let mut app = app_with_fake_port();
        app.mode = mode.clone();
        app.start_read_all_with(|_, _| panic!("busy read must not start"));
        assert_eq!(app.mode, mode);
        assert!(app.read_all_request.is_none());
    }
    for conflict in 0..4 {
        let mut app = app_with_fake_port();
        match conflict {
            0 => app.remote_active = true,
            1 => app.remote_tx = Some(mpsc::channel().0),
            2 => app.ble_scan_in_progress = true,
            _ => app.dialog_open = true,
        }
        app.start_read_all_with(|_, _| panic!("owned read must not start"));
        assert!(app.read_all_request.is_none());
        assert_eq!(app.mode, AppMode::Main(MainTab::Settings));
    }
    let mut app = App::new();
    app.mode = AppMode::Main(MainTab::Channels);
    app.start_read_all_with(|_, _| panic!("no target must not start"));
    assert!(app.status_message.contains("Select a radio"));
}

#[test]
fn read_all_preserves_workspace_raw_snapshot_and_baseline_on_failure_or_truncation() {
    for failure in 0..3 {
        let mut app = App::demo();
        app.demo_mode = false;
        app.protocol_port_name = Some("test://no-hardware".into());
        app.active_port_candidate = None;
        app.port_candidates.clear();
        let before = app.workspace_snapshot();
        let raw = app.codeplug_data.clone();
        let baseline = app.codeplug_workspace_baseline.clone();
        app.start_read_all_with(move |_, report| {
            report(ProgressEvent::Progress(1.0));
            match failure {
                0 => anyhow::bail!("mid-read failure"),
                1 => anyhow::bail!("Failed to resume radio after read"),
                _ => Ok((vec![0xFF; 8191], Endianness::Big)),
            }
        });
        wait_for_read(&mut app);
        assert_eq!(app.workspace_snapshot(), before);
        assert_eq!(app.codeplug_data, raw);
        assert_eq!(app.codeplug_workspace_baseline, baseline);
        assert!(!app.configuration_has_unsaved_edits());
        assert!(matches!(app.mode, AppMode::Error(_)));
        assert!(app.status_message.contains("workspace unchanged"));
        assert!(app.progress < 1.0);
    }
}

#[test]
fn read_all_owns_operation_until_completion_and_refuses_late_workspace_changes() {
    for mutate in [false, true] {
        let mut app = app_with_fake_port();
        let (finish_tx, finish_rx) = mpsc::channel();
        let data = mixed_snapshot();
        app.start_read_all_with(move |_, _| {
            finish_rx.recv().unwrap();
            Ok((data, Endianness::Little))
        });
        app.start_read_all_with(|_, _| panic!("duplicate read must not start"));
        assert!(!app.hardware_action_allowed());
        app.start_import_channels(PathBuf::from("must-not-be-opened.csv"));
        app.start_import_codeplug(PathBuf::from("must-not-be-opened.nfw"));
        app.event_tx
            .send(AppEvent::Error("unrelated late error".into()))
            .unwrap();
        app.event_tx
            .send(AppEvent::LoadCSV(PathBuf::from("must-not-be-opened.csv")))
            .unwrap();
        app.event_tx
            .send(AppEvent::WriteComplete(WriteScope::Channels))
            .unwrap();
        app.update();
        assert_eq!(app.mode, AppMode::Reading);
        assert!(!app.hardware_action_allowed());
        app.start_activity();
        app.remote_on();
        assert!(!app.activity_active());
        assert!(!app.remote_active);
        assert!(app.remote_tx.is_none());
        if mutate {
            app.group_labels[0] = "NEW EDIT".into();
        }
        let before = app.workspace_snapshot();
        finish_tx.send(()).unwrap();
        wait_for_read(&mut app);
        assert_eq!(app.mode, AppMode::Main(MainTab::Settings));
        if mutate {
            assert_eq!(app.workspace_snapshot(), before);
            assert!(app.codeplug_data.is_none());
            assert!(app.status_message.contains("edits preserved"));
        } else {
            assert!(app.codeplug_snapshot_is_current());
        }
    }
}

#[test]
fn read_all_demo_runs_without_transport_and_activity_ownership_includes_stopping() {
    let mut app = App::demo();
    app.protocol_port_name = Some("/must/not/open/this/port".into());
    app.channels.clear();
    app.settings = None;
    app.start_read_all();
    wait_for_read(&mut app);
    assert!(!app.channels.is_empty());
    assert!(app.settings.is_some());
    assert!(app.codeplug_snapshot_is_current());
    assert!(app.status_message.contains("no hardware I/O"));
    app.start_activity();
    for stopping in [false, true] {
        if stopping {
            app.stop_activity();
        }
        app.start_read_all_with(|_, _| panic!("Activity owns the radio"));
        assert!(app.read_all_request.is_none());
        assert!(app.status_message.contains("Activity capture owns"));
    }
}
