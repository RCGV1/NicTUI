//! Hardware-free UI/action matrix. Native file pickers are deliberately not invoked.
use std::{
    fs,
    net::TcpListener,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nictui::{
    app::{App, AppEvent, AppMode, MainTab},
    protocol::{Channel, Endianness, SETTINGS_METADATA},
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};

const SIZES: [(u16, u16); 4] = [(80, 24), (120, 36), (160, 48), (220, 80)];
const TABS: [MainTab; 11] = [
    MainTab::Channels,
    MainTab::Settings,
    MainTab::Scanning,
    MainTab::MemoryGroups,
    MainTab::BandPlan,
    MainTab::DTMF,
    MainTab::Remote,
    MainTab::Codeplug,
    MainTab::BinFlash,
    MainTab::Debug,
    MainTab::Activity,
];

thread_local! {
    // Retain ownership for the test's entire lifetime. Nothing accepts or services
    // this endpoint; rejected actions must not even establish a local connection.
    static SIM_ENDPOINT: TcpListener = TcpListener::bind("127.0.0.1:0").unwrap();
}

fn simulated_workspace() -> App {
    // Seed local fixtures, not a radio connection or a running simulator process.
    let mut app = App::demo();
    // Demo presentation strings are not constrained to EEPROM field widths.
    // These fixtures model editable, protocol-valid programming records.
    app.group_labels = nictui::protocol::normalize_group_labels(&app.group_labels);
    for preset in &mut app.scan_presets {
        preset.start_freq = 14_400_000 + u32::from(preset.index) * 10_000;
    }
    for plan in &mut app.band_plans {
        plan.start_freq = 14_400_000 + u32::from(plan.index) * 10_000;
        plan.end_freq = plan.start_freq + 10_000;
    }
    for preset in &mut app.dtmf_presets {
        preset.label = format!("DTMF {}", preset.index);
    }
    app.codeplug_data = Some(nictui::protocol::codeplug::create_codeplug(
        &app.channels,
        app.settings.as_ref().unwrap(),
        &app.scan_presets,
        &app.band_plans,
        &app.dtmf_presets,
        &app.group_labels,
        app.endian,
    ));
    app.codeplug_workspace_baseline = Some(
        serde_json::to_vec(&(
            &app.channels,
            &app.settings,
            &app.scan_presets,
            &app.band_plans,
            &app.dtmf_presets,
            &app.group_labels,
            &app.deleted_channels,
            app.endian,
        ))
        .unwrap(),
    );
    app.preset_state.select(Some(0));
    app.bandplan_state.select(Some(0));
    app.dtmf_state.select(Some(0));
    app.simulator_mode = true;
    app.demo_mode = false;
    app.protocol_port_name =
        Some(SIM_ENDPOINT.with(|listener| format!("sim://{}", listener.local_addr().unwrap())));
    app.active_port_candidate = None;
    app.port_candidates.clear();
    app.ports.clear();
    app.serial_port_candidates.clear();
    app.ble_port_candidates.clear();
    app.status_message.clear();
    app
}

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

fn workspace(app: &App) -> serde_json::Value {
    serde_json::json!({
        "channels": app.channels,
        "settings": app.settings,
        "scans": app.scan_presets,
        "bands": app.band_plans,
        "dtmf": app.dtmf_presets,
        "groups": app.group_labels,
        "deleted": app.deleted_channels,
        "endian": app.endian,
        "snapshot": app.codeplug_data,
        "dirty": [app.channels_dirty, app.settings_dirty, app.scan_presets_dirty,
                  app.band_plans_dirty, app.dtmf_dirty, app.group_labels_dirty],
    })
}

fn wait_until(app: &mut App, condition: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.update();
        if condition(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{:?}: {}",
            app.mode,
            app.status_message
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct Files(PathBuf);

impl Files {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "nictui-views-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_view_renders_loaded_and_empty_at_all_requested_sizes() {
    for empty in [false, true] {
        let mut app = simulated_workspace();
        if empty {
            app.channels.clear();
            app.settings = None;
            app.scan_presets.clear();
            app.band_plans.clear();
            app.dtmf_presets.clear();
            app.group_labels.fill(String::new());
            app.codeplug_data = None;
        }
        let before = workspace(&app);
        for tab in TABS {
            app.mode = AppMode::Main(tab);
            app.last_main_tab = tab;
            for (width, height) in SIZES {
                let output = render(&mut app, width, height);
                assert!(
                    output.contains("SIMULATOR"),
                    "{tab:?} {width}x{height}:\n{output}"
                );
                assert!(output.contains("Tab"), "navigation missing: {output}");
                let content_area =
                    ui::views::main_layout::main_content_area(Rect::new(0, 0, width, height));
                let content = output
                    .lines()
                    .skip(usize::from(content_area.y))
                    .take(usize::from(content_area.height))
                    .map(|line| {
                        line.chars()
                            .skip(usize::from(content_area.x))
                            .take(usize::from(content_area.width))
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !empty {
                    let expected = match tab {
                        MainTab::Channels => "Selected Channel",
                        MainTab::Settings => "RADIO SETTINGS",
                        MainTab::Scanning => "SCAN PRESETS",
                        MainTab::MemoryGroups => "MEMORY GROUPS",
                        MainTab::BandPlan => "BAND PLAN",
                        MainTab::DTMF => "DTMF PRESETS",
                        MainTab::Remote => "Simulated keypad",
                        MainTab::Codeplug => "EXPORT",
                        MainTab::BinFlash => "flashing unsupported",
                        MainTab::Debug => "Recent Activity",
                        MainTab::Activity => "Activity [A]",
                    };
                    assert!(
                        content.contains(expected),
                        "wrong/missing {tab:?} body at {width}x{height}: {content}"
                    );
                }
                if empty {
                    let expected = match tab {
                        MainTab::Channels | MainTab::MemoryGroups => Some("No Channels Loaded"),
                        MainTab::Settings => Some("No Settings Loaded"),
                        MainTab::Scanning => Some("No Scan Presets Loaded"),
                        MainTab::BandPlan => Some("No Band Plan Loaded"),
                        MainTab::DTMF => Some("No DTMF Loaded"),
                        _ => None,
                    };
                    if let Some(expected) = expected {
                        assert!(
                            output.contains(expected),
                            "{tab:?} {width}x{height}:\n{output}"
                        );
                    }
                }
                assert_eq!(workspace(&app), before, "render mutated {tab:?} workspace");
            }
        }
    }
}

#[test]
fn resize_reuses_terminal_without_losing_selection_or_workspace() {
    let mut app = simulated_workspace();
    app.channel_state.select(Some(40));
    let before = workspace(&app);
    let mut terminal = Terminal::new(TestBackend::new(220, 80)).unwrap();
    for (width, height) in SIZES.into_iter().rev().chain([(40, 12), (8, 3), (1, 1)]) {
        terminal.backend_mut().resize(width, height);
        terminal.autoresize().unwrap();
        for tab in TABS {
            app.mode = AppMode::Main(tab);
            terminal.draw(|frame| ui::ui(frame, &mut app)).unwrap();
        }
        assert_eq!(app.channel_state.selected(), Some(40));
        assert_eq!(workspace(&app), before);
    }
}

#[test]
fn operation_error_and_target_overlays_render_without_mutating_workspace() {
    let mut app = simulated_workspace();
    let before = workspace(&app);
    for mode in [
        AppMode::Reading,
        AppMode::Writing,
        AppMode::BinFlashing,
        AppMode::Error("Local simulator error; workspace unchanged".into()),
        AppMode::PortSelection,
    ] {
        app.mode = mode.clone();
        app.progress = 0.5;
        app.status_message = "Local simulator operation".into();
        for (width, height) in SIZES {
            let output = render(&mut app, width, height);
            let expected = match mode {
                AppMode::Reading | AppMode::Writing => "OPERATION IN PROGRESS",
                AppMode::BinFlashing => "FLASHING IN PROGRESS",
                AppMode::Error(_) => "workspace unchanged",
                AppMode::PortSelection => "BLE scanning are disabled",
                _ => unreachable!(),
            };
            assert!(
                output.contains(expected),
                "{mode:?} {width}x{height}: {output}"
            );
            if mode == AppMode::PortSelection {
                assert!(!output.contains("scans automatically"));
                assert!(!output.contains("press b"));
            }
            assert_eq!(workspace(&app), before);
        }
    }
}

#[test]
fn forward_and_reverse_tabs_visit_every_view_and_return_to_channels() {
    let mut app = simulated_workspace();
    let before = workspace(&app);
    for tab in TABS {
        assert_eq!(app.mode, AppMode::Main(tab));
        app.next_tab();
    }
    assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
    for tab in TABS.into_iter().rev() {
        app.prev_tab();
        assert_eq!(app.mode, AppMode::Main(tab));
        assert_eq!(app.last_main_tab, tab);
    }
    assert_eq!(workspace(&app), before);
}

fn table_selection(app: &App, tab: MainTab) -> Option<usize> {
    match tab {
        MainTab::Channels => app.channel_state.selected(),
        MainTab::Settings => app.settings_state.selected(),
        MainTab::Scanning => app.preset_state.selected(),
        MainTab::MemoryGroups => app.scanning_group_state.selected(),
        MainTab::BandPlan => app.bandplan_state.selected(),
        MainTab::DTMF => app.dtmf_state.selected(),
        MainTab::Activity => app.activity_state.selected(),
        _ => unreachable!(),
    }
}

#[test]
fn table_endpoints_and_pages_clamp_without_mutating_the_workspace() {
    let mut app = simulated_workspace();
    let before = workspace(&app);
    for tab in [
        MainTab::Channels,
        MainTab::Settings,
        MainTab::Scanning,
        MainTab::MemoryGroups,
        MainTab::BandPlan,
        MainTab::DTMF,
        MainTab::Activity,
    ] {
        app.mode = AppMode::Main(tab);
        let len = match tab {
            MainTab::Channels => app.channels.len(),
            MainTab::Settings => SETTINGS_METADATA.len(),
            MainTab::Scanning => app.scan_presets.len(),
            MainTab::MemoryGroups => nictui::protocol::GROUP_LABEL_COUNT,
            MainTab::BandPlan => app.band_plans.len(),
            MainTab::DTMF => app.dtmf_presets.len(),
            MainTab::Activity => app.activity.lock().unwrap().log.rows().count(),
            _ => unreachable!(),
        };
        assert!(len > 0);
        for (key, page, expected) in [
            (KeyCode::End, 10, len - 1),
            (KeyCode::PageDown, 10, len - 1),
            (KeyCode::Home, 10, 0),
            (KeyCode::PageUp, 10, 0),
            (KeyCode::PageDown, 2, 2.min(len - 1)),
            (KeyCode::PageDown, usize::MAX, len - 1),
            (KeyCode::PageUp, usize::MAX, 0),
            (KeyCode::PageDown, 0, 1.min(len - 1)),
        ] {
            assert!(app.handle_table_navigation(key, page));
            assert_eq!(
                table_selection(&app, tab),
                Some(expected),
                "{tab:?} {key:?}"
            );
        }
    }
    assert_eq!(workspace(&app), before);
}

#[test]
fn table_navigation_respects_empty_lists_stale_selection_and_overlays() {
    let mut app = simulated_workspace();
    app.channels.clear();
    app.settings = None;
    app.scan_presets.clear();
    app.band_plans.clear();
    app.dtmf_presets.clear();
    app.clear_activity();
    for tab in [
        MainTab::Channels,
        MainTab::Settings,
        MainTab::Scanning,
        MainTab::BandPlan,
        MainTab::DTMF,
        MainTab::Activity,
    ] {
        app.mode = AppMode::Main(tab);
        for key in [
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
        ] {
            assert!(app.handle_table_navigation(key, 10));
            assert_eq!(table_selection(&app, tab), None);
        }
    }
    app = simulated_workspace();
    app.channel_state.select(Some(usize::MAX));
    assert!(app.handle_table_navigation(KeyCode::PageDown, usize::MAX));
    assert_eq!(app.channel_state.selected(), Some(0));
    app.start_edit_channel();
    for mode in [
        app.mode.clone(),
        AppMode::DeleteChannelConfirm(0),
        AppMode::Reading,
        AppMode::Writing,
        AppMode::BinFlashing,
        AppMode::Error("fixture".into()),
    ] {
        app.mode = mode.clone();
        assert!(!app.handle_table_navigation(KeyCode::End, 10));
        assert_eq!(app.mode, mode);
        assert_eq!(app.channel_state.selected(), Some(0));
    }
    app.mode = AppMode::Main(MainTab::Channels);
    app.dialog_open = true;
    assert!(!app.handle_table_navigation(KeyCode::End, 10));
    assert_eq!(app.channel_state.selected(), Some(0));
}

#[test]
fn empty_lists_clear_stale_selections_and_remain_uneditable() {
    let mut app = simulated_workspace();
    app.channels.clear();
    app.scan_presets.clear();
    for selection in [0, 10, usize::MAX] {
        app.channel_state.select(Some(selection));
        app.next_channel();
        assert_eq!(app.channel_state.selected(), None);
        app.channel_state.select(Some(selection));
        app.prev_channel();
        assert_eq!(app.channel_state.selected(), None);
        app.preset_state.select(Some(selection));
        app.next_scanning_item();
        assert_eq!(app.preset_state.selected(), None);
        app.preset_state.select(Some(selection));
        app.prev_scanning_item();
        assert_eq!(app.preset_state.selected(), None);
    }
    app.start_edit_channel();
    app.start_edit_scan_preset();
    assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
    assert!(app.pending_channel_edit.is_none());
    assert!(app.editing_scan_preset.is_none());
}

#[test]
fn stale_port_and_group_selections_recover_without_discovery_or_overflow() {
    let mut app = simulated_workspace();
    for ports in [
        Vec::new(),
        vec!["local fixture 1".into(), "local fixture 2".into()],
    ] {
        app.ports = ports;
        for action in [App::next_port as fn(&mut App), App::previous_port] {
            app.selected_port_index = usize::MAX;
            action(&mut app);
            assert_eq!(app.selected_port_index, 0);
            assert!(!app.ble_scan_in_progress);
        }
    }
    app.scanning_group_state.select(Some(usize::MAX));
    app.ensure_group_selection();
    assert_eq!(app.scanning_group_state.selected(), Some(0));
}

#[test]
fn remote_mouse_targets_fit_main_layout_without_hitting_navigation_or_status() {
    // Hit-testing only: no press, transmit, connection or remote session is run.
    for (width, height) in SIZES {
        let area = Rect::new(0, 0, width, height);
        let content_area = ui::views::main_layout::main_content_area(area);
        let mut keys = std::collections::BTreeSet::new();
        for row in 0..height {
            for column in 0..width {
                let key = ui::remote_key_at_terminal(area, column, row);
                if !content_area.contains((column, row).into()) {
                    assert_eq!(key, None, "navigation/status hit at {column},{row}");
                }
                if let Some(key) = key {
                    keys.insert(key);
                }
            }
        }
        assert_eq!(keys, (1..=0x13).collect(), "{width}x{height}");
        assert_eq!(ui::remote_key_at_terminal(area, width, height), None);
    }
}

#[test]
fn memory_group_counts_and_previews_follow_local_deletions_without_duplicates() {
    let mut app = simulated_workspace();
    app.channels.truncate(2);
    app.channels[0].groups = [1, 1, 0, 0];
    app.channels[1].groups = [1, 2, 0, 0];
    let deleted_name = app.channels[1].name.clone();
    app.channel_state.select(Some(1));
    app.delete_selected_channel();
    app.scanning_group_state.select(Some(0));
    app.mode = AppMode::Main(MainTab::MemoryGroups);
    for (width, height) in SIZES {
        let output = render(&mut app, width, height);
        assert!(
            output.contains("1 used") || output.contains("1 populated groups"),
            "{output}"
        );
        assert!(output.contains("1 channels"), "{output}");
        assert!(
            !output.contains(&deleted_name),
            "deleted channel still previewed: {output}"
        );
        assert!(output.contains("A-O"), "invalid group range: {output}");
    }
}

#[test]
fn group_label_refresh_shows_names_without_channels_and_keeps_bundled_read_owned() {
    let mut app = simulated_workspace();
    app.channels.clear();
    app.group_labels.fill(String::new());
    app.last_main_tab = MainTab::Channels;
    app.mode = AppMode::Reading;
    app.event_tx
        .send(AppEvent::ReadGroupLabelsComplete(vec!["LOCAL".into()]))
        .unwrap();
    app.update();
    assert_eq!(
        app.mode,
        AppMode::Reading,
        "auxiliary labels must not complete a bundled read"
    );
    assert_eq!(app.group_labels[0], "LOCAL");
    assert!(app.channels.is_empty());
    app.mode = AppMode::Main(MainTab::MemoryGroups);
    for (width, height) in SIZES {
        let output = render(&mut app, width, height);
        assert!(output.contains("MEMORY GROUPS"), "{output}");
        assert!(
            output.contains("LOCAL"),
            "labels hidden without channels: {output}"
        );
        assert!(!output.contains("No Channels Loaded"), "{output}");
    }
}

#[test]
fn group_only_read_updates_labels_without_touching_channels_or_other_dirty_sections() {
    let mut app = simulated_workspace();
    app.channels_dirty = true;
    app.settings_dirty = true;
    app.scan_presets_dirty = true;
    app.band_plans_dirty = true;
    app.dtmf_dirty = true;
    app.deleted_channels.push(2);
    let before = workspace(&app);
    let channel_selection = app.channel_state.selected();
    app.scanning_group_state.select(Some(usize::MAX));
    app.mode = AppMode::Reading;
    app.last_main_tab = MainTab::MemoryGroups;
    app.event_tx
        .send(AppEvent::ReadGroupLabelsOnlyComplete(vec!["RADIO".into()]))
        .unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::Main(MainTab::MemoryGroups));
    assert_eq!(app.last_main_tab, MainTab::MemoryGroups);
    assert_eq!(app.last_non_remote_tab, MainTab::MemoryGroups);
    assert_eq!(app.progress, 1.0);
    assert_eq!(app.group_labels[0], "RADIO");
    assert_eq!(app.scanning_group_state.selected(), Some(0));
    assert_eq!(app.channel_state.selected(), channel_selection);
    let mut after = workspace(&app);
    after["groups"] = before["groups"].clone();
    assert_eq!(after, before, "labels-only read changed another workspace");
}

#[test]
fn group_only_read_preserves_dirty_names_and_open_drafts() {
    for draft in [false, true] {
        let mut app = simulated_workspace();
        if draft {
            app.start_edit_group_label();
            app.edit_buffer = "DRAFT".into();
        } else {
            app.group_labels_dirty = true;
            app.mode = AppMode::Reading;
        }
        let before = workspace(&app);
        let mode = app.mode.clone();
        app.start_read_group_labels();
        assert_eq!(app.mode, mode);
        assert!(app.status_message.contains("Group read blocked"));
        assert!(app.event_rx.try_recv().is_err());
        SIM_ENDPOINT.with(|listener| {
            listener.set_nonblocking(true).unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        });
        app.event_tx
            .send(AppEvent::ReadGroupLabelsOnlyComplete(vec!["RADIO".into()]))
            .unwrap();
        app.update();
        assert_eq!(workspace(&app), before);
        assert!(app.status_message.contains("edits preserved"));
        if draft {
            assert_eq!(app.mode, AppMode::EditGroupLabel(0));
            assert_eq!(app.edit_buffer, "DRAFT");
            assert_eq!(app.editing_group_label_idx, Some(0));
        } else {
            assert_eq!(app.mode, AppMode::Main(MainTab::MemoryGroups));
        }
    }
}

#[test]
fn all_editor_fields_render_and_cancel_without_mutating_workspace() {
    let mut app = simulated_workspace();
    let before = workspace(&app);
    for (start, key_handler, fields) in [
        (
            App::start_edit_channel as fn(&mut App),
            App::handle_channel_editor_key as fn(&mut App, KeyCode),
            11,
        ),
        (
            App::start_edit_scan_preset,
            App::handle_scan_preset_editor_key,
            8,
        ),
        (App::start_edit_bandplan, App::handle_bandplan_editor_key, 8),
        (
            App::start_edit_group_label,
            App::handle_group_label_editor_key,
            1,
        ),
    ] {
        start(&mut app);
        assert!(!matches!(app.mode, AppMode::Main(_)), "editor did not open");
        for field in 0..fields {
            assert!(
                matches!(app.mode,
                AppMode::EditChannel(i) | AppMode::EditScanPreset(i)
                | AppMode::EditBandPlan(i) | AppMode::EditGroupLabel(i) if i == field),
                "field {field}: {:?}: {}",
                app.mode,
                app.status_message
            );
            for (width, height) in SIZES {
                let output = render(&mut app, width, height);
                assert!(output.contains("Esc"), "cancel hint missing: {output}");
                assert_eq!(workspace(&app), before);
            }
            key_handler(&mut app, KeyCode::Tab);
        }
        key_handler(&mut app, KeyCode::Char('X'));
        key_handler(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, AppMode::Main(_)));
        assert!(app.status_message.ends_with("edit cancelled"));
        assert_eq!(workspace(&app), before);
    }
    for index in 0..SETTINGS_METADATA.len() {
        app.settings_state.select(Some(index));
        app.start_edit_setting();
        for (width, height) in SIZES {
            assert!(render(&mut app, width, height).contains("Esc"));
        }
        app.handle_settings_editor_key(KeyCode::Down);
        app.handle_settings_editor_key(KeyCode::Char('9'));
        app.handle_settings_editor_key(KeyCode::Esc);
        assert_eq!(app.status_message, "Setting edit cancelled");
        assert_eq!(workspace(&app), before);
    }
    app.dtmf_edit_preset_idx = Some(0);
    app.mode = AppMode::EditDTMF(0);
    app.update_dtmf_edit_buffer();
    for _ in 0..2 {
        for (width, height) in SIZES {
            assert!(render(&mut app, width, height).contains("Esc"));
        }
        app.handle_dtmf_editor_key(KeyCode::Tab);
    }
    app.handle_dtmf_editor_key(KeyCode::Char('X'));
    app.handle_dtmf_editor_key(KeyCode::Esc);
    assert_eq!(app.status_message, "DTMF edit cancelled");
    assert_eq!(workspace(&app), before);
    assert!(app.pending_channel_edit.is_none());
    assert!(app.pending_dtmf_edit.is_none());
    assert!(app.editing_scan_preset.is_none());
    assert!(app.editing_band_plan.is_none());
}

#[test]
fn local_channel_edit_create_delete_undo_respects_radio_slots() {
    let mut app = simulated_workspace();
    app.channels.truncate(2);
    app.channels[0].channel_num = 1;
    app.channels[1].channel_num = 3;
    app.add_new_channel(None);
    assert_eq!(app.channels.last().unwrap().channel_num, 2);
    assert_eq!(app.channel_state.selected(), Some(2));
    app.start_edit_channel();
    app.edit_buffer = "LOCAL".into();
    app.handle_channel_editor_key(KeyCode::Tab);
    app.edit_buffer = "146.52000".into();
    app.handle_channel_editor_key(KeyCode::Enter);
    assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
    assert_eq!(app.channels[2].name, "LOCAL");
    assert!(app.channels_dirty);
    let before_delete = app.channels.len();
    app.mode = AppMode::DeleteChannelConfirm(2);
    for (width, height) in SIZES {
        assert!(render(&mut app, width, height).contains("Esc"));
    }
    app.confirm_delete_channel(2);
    assert_eq!(app.deleted_channels, [2]);
    assert_eq!(app.channels.len(), before_delete);
    app.undelete_channel();
    assert!(app.deleted_channels.is_empty());

    app.channels = (1..=198)
        .map(|channel_num| Channel {
            channel_num,
            ..Channel::default()
        })
        .collect();
    app.channel_state.select(Some(197));
    let before = workspace(&app);
    app.add_new_channel(None);
    assert_eq!(workspace(&app), before);
    assert!(app.status_message.contains("198"));
    app.channels.retain(|channel| channel.channel_num != 2);
    app.deleted_channels.push(2);
    let before = workspace(&app);
    app.add_new_channel(None);
    assert_eq!(
        workspace(&app),
        before,
        "pending deletion must not conflict with new slot"
    );
}

#[test]
fn blocked_physical_actions_preserve_workspace_and_leave_navigation_usable() {
    let mut app = simulated_workspace();
    app.channels_dirty = true;
    app.settings_dirty = true;
    app.scan_presets_dirty = true;
    app.band_plans_dirty = true;
    app.dtmf_dirty = true;
    app.group_labels_dirty = true;
    let before = workspace(&app);
    let target = app.protocol_port_name.clone();
    for (name, action) in [
        ("refresh ports", App::refresh_ports as fn(&mut App)),
        ("refresh targets", App::refresh_radio_targets),
        ("refresh targets TUI", App::refresh_radio_targets_from_tui),
        ("BLE scan", App::start_ble_scan_from_tui),
        ("select port", App::select_port),
        (
            "settings write/reboot",
            App::start_write_settings_and_reboot,
        ),
        ("channels write", App::start_write_dirty_channels),
        ("DTMF write", App::start_write_dtmf),
        ("presets write", App::start_write_scan_presets),
        ("bands write", App::start_write_band_plans),
        ("groups write", App::start_write_group_labels),
        ("codeplug write", App::start_write_codeplug),
        ("flash", App::start_bin_flash),
    ] {
        app.mode = AppMode::Main(MainTab::Channels);
        action(&mut app);
        app.update();
        assert!(
            app.status_message
                .to_ascii_lowercase()
                .contains("simulator"),
            "{}",
            app.status_message
        );
        assert!(
            matches!(app.mode, AppMode::Main(_)),
            "{name}: {:?}: {}",
            app.mode,
            app.status_message
        );
        assert!(!app.ble_scan_in_progress);
        assert!(!app.dialog_open);
        assert!(!app.remote_active);
        assert_eq!(workspace(&app), before);
        assert_eq!(app.protocol_port_name, target);
        app.next_tab();
        assert_eq!(app.mode, AppMode::Main(MainTab::Settings));
        app.prev_tab();
        assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
    }
    SIM_ENDPOINT.with(|listener| {
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    });
}

#[test]
fn simulator_flash_and_codeplug_never_advertise_hardware_write_readiness() {
    let mut app = simulated_workspace();
    app.bin_firmware_data = Some(vec![0x55; 1024]);
    for (width, height) in SIZES {
        app.mode = AppMode::Main(MainTab::BinFlash);
        let output = render(&mut app, width, height);
        assert!(output.contains("flashing unsupported"), "{output}");
        assert!(!output.contains("Ready To Flash"));
        assert!(!output.contains("Hold PTT"));
        app.mode = AppMode::Main(MainTab::Codeplug);
        let output = render(&mut app, width, height);
        assert!(output.contains("WRITE BLOCKED"), "{output}");
        assert!(output.contains("simulator: writes unsupported"), "{output}");
    }
    app.mode = AppMode::Main(MainTab::Channels);
    app.start_ble_scan_from_tui();
    app.channels_dirty = true;
    app.settings_dirty = true;
    app.scan_presets_dirty = true;
    app.band_plans_dirty = true;
    app.dtmf_dirty = true;
    app.group_labels_dirty = true;
    for (width, height) in SIZES {
        let output = render(&mut app, width, height);
        assert!(
            output.contains("Simulator mode:"),
            "blocked notice hidden: {output}"
        );
        assert!(
            output.contains("writes disabled"),
            "unsupported write hint hidden: {output}"
        );
    }
}

#[test]
fn failed_reads_and_cancelled_file_selection_preserve_dirty_workspace() {
    let mut app = simulated_workspace();
    app.channels_dirty = true;
    app.settings_dirty = true;
    app.deleted_channels.push(2);
    let before = workspace(&app);
    // Deliver the same completion/status events as bounded local workers. Do not
    // create native dialogs or run a protocol session against a physical device.
    for tab in TABS {
        app.last_main_tab = tab;
        app.mode = AppMode::Reading;
        app.event_tx
            .send(AppEvent::Error(
                "Simulated read failure; workspace unchanged".into(),
            ))
            .unwrap();
        app.update();
        assert!(matches!(app.mode, AppMode::Error(_)));
        for (width, height) in SIZES {
            assert!(render(&mut app, width, height).contains("workspace unchanged"));
        }
        assert_eq!(workspace(&app), before);
        app.mode = AppMode::Main(tab);
        app.event_tx
            .send(AppEvent::Status("File selection cancelled.".into()))
            .unwrap();
        app.update();
        assert_eq!(app.mode, AppMode::Main(tab));
        assert_eq!(workspace(&app), before);
    }
}

#[test]
fn read_all_keyboard_chords_do_not_overwrite_dirty_workspace_or_repeat() {
    let mut app = simulated_workspace();
    app.channels_dirty = true;
    let before = workspace(&app);
    for tab in TABS {
        app.mode = AppMode::Main(tab);
        for (code, modifiers) in [
            (KeyCode::Char('R'), KeyModifiers::NONE),
            (KeyCode::Char('r'), KeyModifiers::SHIFT),
            (KeyCode::Char('r'), KeyModifiers::CONTROL),
            (KeyCode::Char('r'), KeyModifiers::ALT),
        ] {
            assert!(app.handle_read_all_key(KeyEvent::new(code, modifiers)));
            assert_eq!(app.mode, AppMode::Main(tab));
            assert_eq!(workspace(&app), before);
        }
        assert!(!app.handle_read_all_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)));
        app.status_message = "unchanged".into();
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            assert!(app.handle_read_all_key(KeyEvent::new_with_kind(
                KeyCode::Char('R'),
                KeyModifiers::NONE,
                kind
            )));
            assert_eq!(app.status_message, "unchanged");
        }
    }
    app.start_edit_channel();
    assert!(!app.handle_read_all_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)));
    assert!(app.handle_read_all_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)));
    assert_eq!(workspace(&app), before);
}

fn section_completions(app: &App) -> Vec<(MainTab, AppEvent)> {
    vec![
        (
            MainTab::Channels,
            AppEvent::ReadChannelsComplete(Vec::new(), Endianness::Little),
        ),
        (MainTab::Scanning, AppEvent::ReadPresetsComplete(Vec::new())),
        (
            MainTab::BandPlan,
            AppEvent::ReadBandPlanComplete(Vec::new()),
        ),
        (MainTab::DTMF, AppEvent::ReadDTMFComplete(Vec::new())),
        (
            MainTab::Settings,
            AppEvent::ReadSettingsComplete(app.settings.clone().unwrap(), Endianness::Little),
        ),
    ]
}

#[test]
fn late_section_completions_cannot_overwrite_any_unsaved_records_or_drafts() {
    let mut app = simulated_workspace();
    app.channels_dirty = true;
    app.settings_dirty = true;
    app.scan_presets_dirty = true;
    app.band_plans_dirty = true;
    app.dtmf_dirty = true;
    app.group_labels_dirty = true;
    app.deleted_channels.push(1);
    let before = workspace(&app);
    for (tab, event) in section_completions(&app) {
        app.mode = AppMode::Reading;
        app.last_main_tab = tab;
        app.event_tx.send(event).unwrap();
        app.update();
        assert_eq!(workspace(&app), before, "{tab:?}");
        assert_eq!(app.mode, AppMode::Main(tab));
        assert!(app.status_message.contains("edits preserved"));
    }
    app.event_tx
        .send(AppEvent::ReadGroupLabelsComplete(vec!["RADIO".into(); 15]))
        .unwrap();
    app.update();
    assert_eq!(workspace(&app), before);
    assert!(app.status_message.contains("group edits preserved"));

    let mut app = simulated_workspace();
    let before = workspace(&app);
    app.start_edit_channel();
    app.edit_buffer = "DRAFT".into();
    app.event_tx
        .send(AppEvent::ReadChannelsComplete(
            Vec::new(),
            Endianness::Little,
        ))
        .unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::EditChannel(0));
    assert_eq!(app.edit_buffer, "DRAFT");
    assert!(app.pending_channel_edit.is_some());
    app.handle_channel_editor_key(KeyCode::Esc);
    app.start_edit_scan_preset();
    app.event_tx
        .send(AppEvent::ReadPresetsComplete(Vec::new()))
        .unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::EditScanPreset(0));
    assert!(app.editing_scan_preset.is_some());
    app.handle_scan_preset_editor_key(KeyCode::Esc);
    app.start_edit_bandplan();
    app.event_tx
        .send(AppEvent::ReadBandPlanComplete(Vec::new()))
        .unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::EditBandPlan(0));
    assert!(app.editing_band_plan.is_some());
    app.handle_bandplan_editor_key(KeyCode::Esc);
    app.dtmf_edit_preset_idx = Some(0);
    app.mode = AppMode::EditDTMF(0);
    app.update_dtmf_edit_buffer();
    app.event_tx
        .send(AppEvent::ReadDTMFComplete(Vec::new()))
        .unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::EditDTMF(0));
    assert!(app.pending_dtmf_edit.is_some());
    app.handle_dtmf_editor_key(KeyCode::Esc);
    app.start_edit_group_label();
    app.event_tx
        .send(AppEvent::ReadGroupLabelsComplete(vec!["RADIO".into(); 15]))
        .unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::EditGroupLabel(0));
    app.handle_group_label_editor_key(KeyCode::Esc);
    app.settings_state.select(Some(4));
    app.start_edit_setting();
    app.edit_buffer = "12.50".into();
    app.event_tx
        .send(AppEvent::ReadSettingsComplete(
            Default::default(),
            Endianness::Little,
        ))
        .unwrap();
    app.update();
    assert_eq!(app.mode, AppMode::EditSetting(4));
    assert_eq!(app.edit_buffer, "12.50");
    app.handle_settings_editor_key(KeyCode::Esc);
    assert_eq!(workspace(&app), before);
}

#[test]
fn clean_section_completions_load_data_and_clear_empty_selections() {
    let mut app = simulated_workspace();
    for (tab, event) in section_completions(&app) {
        app.last_main_tab = tab;
        app.mode = AppMode::Reading;
        app.event_tx.send(event).unwrap();
        app.update();
        assert_eq!(app.mode, AppMode::Main(tab));
        let selected = match tab {
            MainTab::Channels => app.channel_state.selected(),
            MainTab::Scanning => app.preset_state.selected(),
            MainTab::BandPlan => app.bandplan_state.selected(),
            MainTab::DTMF => app.dtmf_state.selected(),
            MainTab::Settings => {
                assert!(app.settings.is_some());
                assert_eq!(app.endian, Endianness::Little);
                assert_eq!(app.settings_state.selected(), Some(0));
                continue;
            }
            _ => unreachable!(),
        };
        assert_eq!(selected, None, "{tab:?}");
    }
}

#[test]
fn local_section_edits_commit_only_their_own_records_and_dirty_flags() {
    for tab in [
        MainTab::Settings,
        MainTab::Scanning,
        MainTab::MemoryGroups,
        MainTab::BandPlan,
        MainTab::DTMF,
    ] {
        let mut app = simulated_workspace();
        let before = workspace(&app);
        let changed = match tab {
            MainTab::Settings => {
                app.settings_state.select(Some(4));
                app.start_edit_setting();
                app.edit_buffer = "12.50".into();
                app.handle_settings_editor_key(KeyCode::Enter);
                assert_eq!(app.settings.as_ref().unwrap().step, 1250);
                "settings"
            }
            MainTab::Scanning => {
                app.start_edit_scan_preset();
                app.edit_buffer = "LOCAL".into();
                app.handle_scan_preset_editor_key(KeyCode::Enter);
                assert_eq!(app.scan_presets[0].label, "LOCAL");
                "scans"
            }
            MainTab::MemoryGroups => {
                app.start_edit_group_label();
                app.edit_buffer = "LOCAL".into();
                app.handle_group_label_editor_key(KeyCode::Enter);
                assert_eq!(app.group_labels[0], "LOCAL");
                "groups"
            }
            MainTab::BandPlan => {
                app.start_edit_bandplan();
                for _ in 0..3 {
                    app.handle_bandplan_editor_key(KeyCode::Tab);
                }
                app.edit_buffer = "128".into();
                app.handle_bandplan_editor_key(KeyCode::Enter);
                assert_eq!(app.band_plans[0].max_power, 128);
                "bands"
            }
            MainTab::DTMF => {
                app.dtmf_edit_preset_idx = Some(0);
                app.mode = AppMode::EditDTMF(0);
                app.update_dtmf_edit_buffer();
                app.edit_buffer = "LOCAL".into();
                app.handle_dtmf_editor_key(KeyCode::Tab);
                app.edit_buffer = "12*#".into();
                app.handle_dtmf_editor_key(KeyCode::Enter);
                assert_eq!(app.dtmf_presets[0].digits, [1, 2, 14, 15]);
                "dtmf"
            }
            _ => unreachable!(),
        };
        assert_eq!(app.mode, AppMode::Main(tab), "{}", app.status_message);
        let after = workspace(&app);
        for key in [
            "channels", "settings", "scans", "bands", "dtmf", "groups", "snapshot", "deleted",
            "endian",
        ] {
            if key == changed {
                assert_ne!(before[key], after[key]);
            } else {
                assert_eq!(before[key], after[key], "{tab:?} changed {key}");
            }
        }
        assert_eq!(
            after["dirty"],
            serde_json::json!([
                false,
                tab == MainTab::Settings,
                tab == MainTab::Scanning,
                tab == MainTab::BandPlan,
                tab == MainTab::DTMF,
                tab == MainTab::MemoryGroups,
            ])
        );
    }
}

#[test]
fn temporary_csv_codeplug_and_firmware_file_actions_are_local_and_lossless() {
    let files = Files::new();
    let mut app = simulated_workspace();
    let csv = files.path("channels.csv");
    app.export_csv(csv.clone()).unwrap();
    let mut imported = simulated_workspace();
    imported.start_import_channels(csv);
    assert!(
        matches!(imported.mode, AppMode::Main(MainTab::Channels)),
        "{:?}",
        imported.mode
    );
    assert_eq!(
        serde_json::to_value(&imported.channels).unwrap(),
        serde_json::to_value(&app.channels).unwrap()
    );
    assert_eq!(imported.group_labels, app.group_labels);
    assert!(imported.channels_dirty);
    let before = workspace(&imported);
    imported.start_import_channels(files.path("missing.csv"));
    assert!(matches!(imported.mode, AppMode::Error(_)));
    assert_eq!(workspace(&imported), before);

    let codeplug = files.path("snapshot.nfw");
    app.start_export_codeplug(codeplug.clone());
    wait_until(&mut app, |app| {
        app.status_message.contains("exported byte-for-byte")
    });
    let bytes = app.codeplug_data.clone().unwrap();
    app.start_import_codeplug(codeplug);
    wait_until(&mut app, |app| {
        matches!(app.mode, AppMode::Main(MainTab::Codeplug))
    });
    assert_eq!(app.codeplug_data.as_ref().unwrap(), &bytes);
    assert!(app.codeplug_snapshot_is_current());
    app.channels_dirty = true;
    let refused = files.path("must-not-exist.nfw");
    app.start_export_codeplug(refused.clone());
    assert!(app.status_message.contains("Export blocked"));
    assert!(!refused.exists());
    let before = workspace(&app);
    app.start_import_codeplug(files.path("missing.nfw"));
    wait_until(&mut app, |app| matches!(app.mode, AppMode::Error(_)));
    assert_eq!(workspace(&app), before);

    let firmware = files.path("local.bin");
    fs::write(&firmware, [0x42, 0x13, 0x37]).unwrap();
    app.start_load_bin_firmware(firmware.clone());
    wait_until(&mut app, |app| {
        app.bin_file_path.as_ref() == Some(&firmware)
    });
    assert_eq!(
        app.bin_firmware_data.as_deref(),
        Some([0x42, 0x13, 0x37].as_slice())
    );
    app.start_load_bin_firmware(files.path("missing.bin"));
    wait_until(&mut app, |app| matches!(app.mode, AppMode::Error(_)));
    assert_eq!(
        app.bin_firmware_data.as_deref(),
        Some([0x42, 0x13, 0x37].as_slice())
    );
    assert_eq!(workspace(&app), before);
    assert_eq!(app.endian, Endianness::Big);
}
