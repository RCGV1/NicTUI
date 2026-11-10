use nictui::{
    app::{App, AppMode, MainTab},
    protocol::{GROUP_LABEL_COUNT, SETTINGS_METADATA, SettingType},
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};

const SIZES: &[(u16, u16)] = &[
    (60, 20),
    (80, 24),
    (100, 30),
    (120, 40),
    (160, 50),
    (190, 80),
];

// App construction and rendering only: no discovery, workers, event handlers or file dialogs.
fn render(app: &mut App, width: u16, height: u16, area: Option<Rect>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::ui(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let area = area.unwrap_or(Rect::new(0, 0, width, height));
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_area(width: u16, height: u16) -> Rect {
    ui::views::main_layout::main_content_area(Rect::new(0, 0, width, height))
}

#[test]
fn all_eleven_screens_render_empty_and_populated_at_every_size() {
    let screens = [
        (AppMode::Main(MainTab::Channels), "Channels"),
        (AppMode::Main(MainTab::Settings), "SETTINGS"),
        (AppMode::Main(MainTab::Scanning), "SCAN PRESETS"),
        (AppMode::Main(MainTab::MemoryGroups), "MEMORY GROUPS"),
        (AppMode::Main(MainTab::BandPlan), "BAND PLAN"),
        (AppMode::Main(MainTab::DTMF), "DTMF"),
        (AppMode::Main(MainTab::Codeplug), "EXPORT"),
        (AppMode::Main(MainTab::BinFlash), "Flash"),
        (AppMode::Main(MainTab::Activity), "Activity"),
        (AppMode::Main(MainTab::Debug), "Recent Activity"),
        (AppMode::PortSelection, "Radio Targets"),
    ];
    for &(width, height) in SIZES {
        for (mode, title) in &screens {
            for populated in [false, true] {
                let mut app = if populated { App::demo() } else { App::new() };
                app.demo_mode = true;
                app.mode = mode.clone();
                let area = if matches!(mode, AppMode::PortSelection) {
                    None
                } else {
                    Some(content_area(width, height))
                };
                let text = render(&mut app, width, height, area);
                assert!(
                    text.to_uppercase().contains(&title.to_uppercase()),
                    "{mode:?} populated={populated} {width}x{height}:\n{text}"
                );
                if matches!(mode, AppMode::PortSelection) {
                    assert!(text.contains("Enter"), "{text}");
                    assert!(
                        text.contains(if populated { "TD-H3" } else { "No targets" }),
                        "{text}"
                    );
                }
            }
        }
    }
}

#[test]
fn every_editor_field_keeps_input_and_save_cancel_visible() {
    let editors = [
        (AppMode::EditChannel(0), 11),
        (AppMode::EditSetting(0), SETTINGS_METADATA.len()),
        (AppMode::EditScanPreset(0), 8),
        (AppMode::EditBandPlan(0), 8),
        (AppMode::EditDTMF(0), 2),
        (AppMode::EditGroupLabel(0), GROUP_LABEL_COUNT),
    ];
    for &(width, height) in SIZES {
        for (mode, fields) in &editors {
            for field in 0..*fields {
                let mut app = App::demo();
                app.pending_channel_edit = Some(app.channels[0].clone());
                app.editing_scan_preset = Some(app.scan_presets[0].clone());
                app.editing_band_plan = Some(app.band_plans[0].clone());
                app.pending_dtmf_edit = Some(app.dtmf_presets[0].clone());
                app.dtmf_edit_preset_idx = Some(0);
                app.editing_group_label_idx = Some(field.min(GROUP_LABEL_COUNT - 1));
                app.mode = match mode {
                    AppMode::EditChannel(_) => AppMode::EditChannel(field),
                    AppMode::EditSetting(_) => AppMode::EditSetting(field),
                    AppMode::EditScanPreset(_) => AppMode::EditScanPreset(field),
                    AppMode::EditBandPlan(_) => AppMode::EditBandPlan(field),
                    AppMode::EditDTMF(_) => AppMode::EditDTMF(field),
                    AppMode::EditGroupLabel(_) => AppMode::EditGroupLabel(field),
                    _ => unreachable!(),
                };
                app.edit_buffer = "EDIT123".into();
                let text = render(&mut app, width, height, None);
                let choice_setting = matches!(mode, AppMode::EditSetting(_))
                    && SETTINGS_METADATA.get(field).is_some_and(|meta| {
                        !matches!(meta.setting_type, SettingType::Numeric { .. })
                    });
                if choice_setting {
                    assert!(
                        text.contains("Options"),
                        "{:?} {width}x{height}:\n{text}",
                        app.mode
                    );
                } else {
                    assert!(
                        text.contains("EDIT123"),
                        "{:?} {width}x{height}:\n{text}",
                        app.mode
                    );
                }
                for control in ["Enter", "Esc"] {
                    assert!(
                        text.contains(control),
                        "{:?} {width}x{height} missing {control}:\n{text}",
                        app.mode
                    );
                }
            }
        }
    }
}

#[test]
fn last_channel_group_option_scrolls_into_view() {
    for &(width, height) in SIZES {
        let mut app = App::demo();
        app.pending_channel_edit = Some(app.channels[0].clone());
        app.mode = AppMode::EditChannel(9);
        app.selection_index = GROUP_LABEL_COUNT;
        app.group_labels[GROUP_LABEL_COUNT - 1] = "LAST".into();
        let text = render(&mut app, width, height, None);
        assert!(text.contains("> LAST"), "{width}x{height}:\n{text}");
    }
}

#[test]
fn compact_settings_preserve_unknown_codes_and_do_not_claim_bluetooth_enabled() {
    for &(width, height) in SIZES {
        let mut app = App::demo();
        app.settings.as_mut().unwrap().bluetooth = 255;
        app.settings_state.select(Some(30));
        app.mode = AppMode::Main(MainTab::Settings);
        let text = render(&mut app, width, height, Some(content_area(width, height)));
        assert!(text.contains("Unknown (255)"), "{width}x{height}:\n{text}");
        assert!(!text.contains("BLE enabled"), "{text}");
        assert!(!text.contains("BLE is enabled"), "{text}");
        assert!(text.contains("Edit") && text.contains("Write"), "{text}");
    }
}

#[test]
fn validation_and_error_dialogs_keep_message_tail_and_exit_visible() {
    for &(width, height) in SIZES {
        for mode in [
            AppMode::EditChannel(5),
            AppMode::EditSetting(0),
            AppMode::EditScanPreset(7),
            AppMode::EditBandPlan(3),
            AppMode::EditDTMF(1),
        ] {
            let mut app = App::demo();
            app.pending_channel_edit = Some(app.channels[0].clone());
            app.editing_scan_preset = Some(app.scan_presets[0].clone());
            app.editing_band_plan = Some(app.band_plans[0].clone());
            app.pending_dtmf_edit = Some(app.dtmf_presets[0].clone());
            app.dtmf_edit_preset_idx = Some(0);
            let prefix = match mode {
                AppMode::EditSetting(_) => "Invalid setting:",
                AppMode::EditScanPreset(_) => "Invalid scan:",
                AppMode::EditBandPlan(_) => "Invalid band:",
                AppMode::EditChannel(_) => "Invalid power:",
                _ => "Invalid DTMF:",
            };
            app.status_message =
                format!("{prefix} value is outside the allowed range; use a smaller value. ENDMSG");
            app.mode = mode;
            let text = render(&mut app, width, height, None);
            assert!(
                text.contains("ENDMSG"),
                "{:?} {width}x{height}:\n{text}",
                app.mode
            );
        }
        let mut app = App::new();
        app.mode = AppMode::Error(
            "Unable to load the local file. Check the path and try importing again. ENDMSG".into(),
        );
        let text = render(&mut app, width, height, None);
        assert!(
            text.contains("ENDMSG") && text.contains("Esc"),
            "{width}x{height}:\n{text}"
        );
    }
}

#[test]
fn focused_values_and_actions_survive_dirty_and_unknown_data() {
    for &(width, height) in SIZES {
        for tab in [
            MainTab::Channels,
            MainTab::Settings,
            MainTab::Scanning,
            MainTab::MemoryGroups,
            MainTab::DTMF,
        ] {
            let mut app = App::demo();
            app.channels_dirty = true;
            app.settings_dirty = true;
            app.scan_presets_dirty = true;
            app.group_labels_dirty = true;
            app.dtmf_dirty = true;
            app.channels[0].rx_freq = "1000.00000".into();
            app.channels[0].tx_freq = "1300.00000".into();
            app.channels[0].bandwidth = "Raw (255)".into();
            app.channel_state.select(Some(0));
            app.scan_presets[0].modulation = 255;
            app.scan_presets[0].start_freq = 130_000_000;
            app.preset_state.select(Some(0));
            app.scanning_group_state.select(Some(0));
            app.dtmf_presets[0].digits = vec![1, 14, 15, 255];
            app.dtmf_state.select(Some(0));
            app.mode = AppMode::Main(tab);
            let text = render(&mut app, width, height, Some(content_area(width, height)));
            let expected: &[&str] = match tab {
                MainTab::Channels => &[
                    "1000.00000",
                    "1300.00000",
                    "Raw (255)",
                    "unknown BW",
                    "Unsaved",
                ],
                MainTab::Settings => &["Edit", "Write", "UNSAVED"],
                MainTab::Scanning => &["Raw (255)", "1300.00000", "Edit", "Write", "Unsaved"],
                MainTab::MemoryGroups => &["Rename", "labels", "presets", "Unsaved"],
                MainTab::DTMF => &["1*#[raw 255]", "Edit", "Write", "UNSAVED"],
                _ => unreachable!(),
            };
            for expected in expected {
                assert!(
                    text.to_lowercase().contains(&expected.to_lowercase()),
                    "{tab:?} {width}x{height} missing {expected}:\n{text}"
                );
            }
        }
    }
}

#[test]
fn long_logs_and_activity_notices_wrap_instead_of_losing_the_tail() {
    for &(width, height) in SIZES {
        for tab in [MainTab::Debug, MainTab::Activity] {
            let mut app = App::demo();
            let message =
                "Local file could not be saved. Check the directory permissions and retry. ENDMSG";
            app.logs.push_back(format!("RX: {message}"));
            app.activity.lock().unwrap().notice = message.into();
            app.mode = AppMode::Main(tab);
            let text = render(&mut app, width, height, Some(content_area(width, height)));
            assert!(text.contains("ENDMSG"), "{tab:?} {width}x{height}:\n{text}");
            if tab == MainTab::Activity {
                assert!(text.contains("s start"), "{text}");
            }
        }
    }
}

#[test]
fn firmware_workflow_keeps_last_step_and_loaded_filename_visible() {
    for &(width, height) in SIZES {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::BinFlash);
        app.bin_file_path = Some("/a/long/local/path/to/firmware-review.bin".into());
        app.bin_firmware_data = Some(vec![0; 1234]);
        let text = render(&mut app, width, height, Some(content_area(width, height)));
        for expected in ["4.", "firmware-review.bin", "1234", "disabled"] {
            assert!(
                text.contains(expected),
                "{width}x{height} missing {expected}:\n{text}"
            );
        }
        assert!(!text.contains("Ready To Flash"), "{text}");
    }
}

#[test]
fn progress_and_delete_overlays_keep_the_safety_controls() {
    for &(width, height) in SIZES {
        for mode in [
            AppMode::Reading,
            AppMode::Writing,
            AppMode::BinFlashing,
            AppMode::DeleteChannelConfirm(0),
        ] {
            let mut app = App::demo();
            app.mode = mode;
            app.progress = 0.42;
            app.status_message = "Processing local fixture".into();
            let text = render(&mut app, width, height, None);
            let expected = if matches!(app.mode, AppMode::DeleteChannelConfirm(_)) {
                "Esc to cancel"
            } else {
                "Esc: finish safely, then quit"
            };
            assert!(
                text.contains(expected),
                "{:?} {width}x{height}:\n{text}",
                app.mode
            );
            if !matches!(app.mode, AppMode::DeleteChannelConfirm(_)) {
                assert!(text.contains("42%"), "{text}");
            }
        }
    }
}
