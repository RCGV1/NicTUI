use nictui::{
    app::{App, AppMode, MainTab},
    ui::views::{self, main_layout::main_content_area},
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};

const SIZES: &[(u16, u16)] = &[(60, 20), (80, 24), (120, 36), (190, 80)];

// Rendering only: fixtures never start workers, discover devices, or open dialogs.
fn render(app: &mut App, tab: MainTab, width: u16, height: u16) -> String {
    app.mode = AppMode::Main(tab);
    let area = main_content_area(Rect::new(0, 0, width, height));
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            views::main_layout::render_main_layout(frame, app, frame.area(), tab);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn contains(text: &str, expected: &[&str]) {
    // Match wrapped text while retaining all words, so clipping still fails.
    let normalized = text.replace('\u{2502}', " ");
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    for expected in expected {
        assert!(normalized.contains(expected), "Missing {expected}:\n{text}");
    }
}

#[test]
fn empty_sections_show_case_sensitive_read_and_import_routes() {
    for &(width, height) in SIZES {
        for (tab, read) in [
            (MainTab::Channels, "r: read channels"),
            (MainTab::Settings, "r: read settings"),
            (MainTab::Scanning, "r: read scan presets"),
            (MainTab::BandPlan, "r: read band plan"),
            (MainTab::DTMF, "r: read DTMF presets"),
        ] {
            let text = render(&mut App::new(), tab, width, height);
            contains(&text, &[read, "Shift+R: read all sections"]);
            contains(
                &text,
                &[if tab == MainTab::Channels {
                    "i: import CSV/CHIRP"
                } else {
                    "8: import .nfw"
                }],
            );
            assert!(!text.contains("Enter edit"), "{text}");
        }
    }
}

#[test]
fn dirty_sections_identify_local_changes_and_radio_write_action() {
    for &(width, height) in SIZES {
        for tab in [
            MainTab::Channels,
            MainTab::Settings,
            MainTab::Scanning,
            MainTab::MemoryGroups,
            MainTab::BandPlan,
            MainTab::DTMF,
        ] {
            let mut app = App::demo();
            app.channels_dirty = true;
            app.settings_dirty = true;
            app.scan_presets_dirty = true;
            app.group_labels_dirty = true;
            app.band_plans_dirty = true;
            app.dtmf_dirty = true;
            let text = render(&mut app, tab, width, height).to_lowercase();
            contains(
                &text,
                &[
                    "unsaved local",
                    if matches!(tab, MainTab::Scanning | MainTab::BandPlan) {
                        "w write radio"
                    } else {
                        "w writes"
                    },
                ],
            );
        }
    }
}

#[test]
fn group_counts_do_not_claim_empty_membership_before_channels_load() {
    for &(width, height) in SIZES {
        let mut app = App::new();
        let text = render(&mut app, MainTab::MemoryGroups, width, height);
        contains(
            &text,
            &[
                "Channels not loaded",
                "Shift+R",
                "Read labels",
                "Write labels",
            ],
        );
        assert!(!text.contains("No channels assigned"), "{text}");
    }
}

#[test]
fn workspace_bluetooth_on_does_not_confirm_radio_state() {
    for &(width, height) in SIZES {
        let mut app = App::demo();
        app.settings_state.select(Some(30));
        app.settings_dirty = true;
        let text = render(&mut app, MainTab::Settings, width, height);
        contains(&text, &["Workspace BLE: On", "Write + reboot"]);
        assert!(!text.contains("BLE is enabled"), "{text}");
    }
}

#[test]
fn unknown_channel_modes_have_no_plausible_compact_success_label() {
    for &(width, height) in SIZES {
        let mut app = App::demo();
        app.channels[0].bandwidth = "Raw (255)".into();
        app.channels[0].modulation = "FM".into();
        app.channel_state.select(Some(0));
        let text = render(&mut app, MainTab::Channels, width, height);
        contains(&text, &["Raw (255)", "unknown BW"]);
        if width < 100 {
            contains(&text, &["? FM"]);
            assert!(!text.contains("RFM"), "{text}");
        }
        app.channels[0].bandwidth = "Wide".into();
        app.channels[0].modulation = "Raw (254)".into();
        let text = render(&mut app, MainTab::Channels, width, height);
        contains(&text, &["Raw (254)"]);
        if width < 100 {
            contains(&text, &["W ?"]);
        }
    }
}

#[test]
fn firmware_prerequisites_never_claim_verified_bootloader_readiness() {
    for &(width, height) in SIZES {
        for (size, port, reason) in [
            (0, Some("fixture://usb"), "1..=63488 bytes"),
            (63489, Some("fixture://usb"), "1..=63488 bytes"),
            (1234, None, "Select a USB radio target"),
            (1234, Some("ble://fixture"), "USB required"),
            (1234, Some("fixture://usb"), "Bootloader state not verified"),
        ] {
            let mut app = App::new();
            app.bin_file_path = Some("/local/fixture/firmware.bin".into());
            app.bin_firmware_data = Some(vec![0; size]);
            app.protocol_port_name = port.map(str::to_string);
            let text = render(&mut app, MainTab::BinFlash, width, height);
            contains(&text, &[reason, "firmware.bin", "4."]);
            assert!(!text.contains("Ready To Flash"), "{text}");
            if size == 1234 && port == Some("fixture://usb") {
                contains(&text, &["File + USB Selected", "bootloader entry"]);
            } else {
                contains(&text, &["f disabled"]);
            }
        }
    }
}

#[test]
fn snapshot_export_names_its_file_format_and_blocks_stale_workspace() {
    for &(width, height) in SIZES {
        let mut app = App::demo();
        let text = render(&mut app, MainTab::Codeplug, width, height);
        contains(
            &text,
            &["EXPORT AVAILABLE", "save .nfw", "edits are not merged"],
        );
        app.channels_dirty = true;
        let text = render(&mut app, MainTab::Codeplug, width, height);
        contains(
            &text,
            &[
                "EXPORT BLOCKED",
                "workspace changed",
                "write sections, then read full",
            ],
        );
        assert!(!text.contains("save .nfw"), "{text}");
    }
}

#[test]
fn remote_ownership_explains_blocked_workflows_until_cleanup() {
    for &(width, height) in SIZES {
        let mut app = App::demo();
        app.demo_mode = false;
        app.remote_active = true;
        app.protocol_port_name = Some("fixture://usb".into());
        app.port_candidates.clear();
        app.bin_firmware_data = Some(vec![0; 1234]);
        for tab in [MainTab::BinFlash, MainTab::Codeplug, MainTab::Activity] {
            let text = render(&mut app, tab, width, height);
            contains(&text.to_lowercase(), &["stop remote", "cleanup"]);
            assert!(!text.contains("WRITE AVAILABLE"), "{text}");
            assert!(!text.contains("Press s to start"), "{text}");
        }
        app.activity.lock().unwrap().stopping = true;
        let text = render(&mut app, MainTab::Activity, width, height);
        contains(&text, &["Stopping; wait for cleanup", "save CSV"]);
        assert!(!text.contains("Press s to start"), "{text}");
    }
}

#[test]
fn target_picker_explains_why_continue_is_inactive() {
    for &(width, height) in SIZES {
        let app = App::new();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| views::port_selection::render_port_selection(frame, &app, frame.area()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        contains(&text, &["Enter", "disabled: select target", "No targets"]);
    }
}

#[test]
fn remote_rendering_keeps_display_data_and_control_hit_mapping_unchanged() {
    let keys = [
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    ];
    for &(width, height) in SIZES {
        let mut app = App::demo();
        let pixels = |app: &App| {
            (0..128)
                .flat_map(|y| (0..128).map(move |x| app.remote_screen.display.pixel(x, y)))
                .collect::<Vec<_>>()
        };
        let before = pixels(&app);
        render(&mut app, MainTab::Remote, width, height);
        assert_eq!(pixels(&app), before);
        let area = main_content_area(Rect::new(0, 0, width, height));
        let mapped = (area.y..area.bottom())
            .flat_map(|y| {
                (area.x..area.right()).filter_map(move |x| {
                    views::main_layout::remote_key_at_terminal(Rect::new(0, 0, width, height), x, y)
                })
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(mapped, keys.into_iter().collect());
    }
}
