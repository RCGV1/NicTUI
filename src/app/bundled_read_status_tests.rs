use super::*;

fn finish_bundle(app: &mut App, scan: bool, labels_fail: bool, split_updates: bool) {
    let (tx, rx) = mpsc::channel();
    let mut incoming = App::demo();
    incoming.channels[0].name = "RADIO".into();
    incoming.scan_presets[0].start_freq += 100;
    read_with_group_labels(
        &tx,
        if scan { "Scan presets" } else { "Channels" },
        |_| {
            Ok((
                if scan {
                    AppEvent::ReadPresetsComplete(incoming.scan_presets)
                } else {
                    AppEvent::ReadChannelsComplete(incoming.channels, Endianness::Little)
                },
                Endianness::Little,
                if labels_fail {
                    Err(anyhow::anyhow!("fixture label failure"))
                } else {
                    Ok(vec!["RADIO GROUP".into()])
                },
            ))
        },
        |event, _| event,
    )
    .unwrap();
    for event in rx.try_iter() {
        app.event_tx.send(event).unwrap();
        if split_updates {
            app.update();
        }
    }
    app.update();
}

#[test]
fn bundled_read_status_preserves_each_rejected_primary_guard() {
    for scan in [false, true] {
        for guard in 0..if scan { 2 } else { 3 } {
            for labels_fail in [false, true] {
                for split_updates in [false, true] {
                    let mut app = App::demo();
                    app.mode = AppMode::Reading;
                    match (scan, guard) {
                        (true, 0) => app.scan_presets_dirty = true,
                        (true, _) => app.editing_scan_preset = Some(app.scan_presets[0].clone()),
                        (false, 0) => app.channels_dirty = true,
                        (false, 1) => app.deleted_channels.push(198),
                        (false, _) => app.pending_channel_edit = Some(app.channels[0].clone()),
                    }
                    let channels = serde_json::to_value(&app.channels).unwrap();
                    let presets = serde_json::to_value(&app.scan_presets).unwrap();
                    let deletions = app.deleted_channels.clone();
                    finish_bundle(&mut app, scan, labels_fail, split_updates);
                    let section = if scan { "scan preset" } else { "channel" };
                    assert!(app.status_message.starts_with(&format!(
                        "Read not applied: unsaved {section} edits preserved"
                    )));
                    if labels_fail {
                        assert!(
                            app.status_message.contains(
                                "group names could not be refreshed: fixture label failure"
                            )
                        );
                    } else {
                        assert!(app.status_message.ends_with("group names loaded"));
                        assert_eq!(app.group_labels[0], "RADIO");
                    }
                    assert_eq!(serde_json::to_value(&app.channels).unwrap(), channels);
                    assert_eq!(serde_json::to_value(&app.scan_presets).unwrap(), presets);
                    assert_eq!(app.deleted_channels, deletions);
                    assert!(!matches!(app.mode, AppMode::Reading));
                    assert_eq!(app.progress, 1.0);
                }
            }
        }
    }
}

#[test]
fn bundled_read_status_preserves_group_warning_with_or_without_primary_rejection() {
    for scan in [false, true] {
        for primary_dirty in [false, true] {
            for group_draft in [false, true] {
                for labels_fail in [false, true] {
                    for split_updates in [false, true] {
                        let mut app = App::demo();
                        app.mode = AppMode::Reading;
                        app.channels_dirty = !scan && primary_dirty;
                        app.scan_presets_dirty = scan && primary_dirty;
                        app.group_labels[0] = "LOCAL GROUP".into();
                        app.group_labels_dirty = !group_draft;
                        app.editing_group_label_idx = group_draft.then_some(0);
                        app.edit_buffer = "LOCAL DRAFT".into();
                        let start_freq = app.scan_presets[0].start_freq;
                        finish_bundle(&mut app, scan, labels_fail, split_updates);
                        assert!(app.status_message.contains("unsaved group edits preserved"));
                        assert_eq!(app.group_labels[0], "LOCAL GROUP");
                        assert_eq!(app.group_labels_dirty, !group_draft);
                        assert_eq!(app.editing_group_label_idx, group_draft.then_some(0));
                        assert_eq!(app.edit_buffer, "LOCAL DRAFT");
                        assert!(!app.status_message.contains("and group names loaded"));
                        if labels_fail {
                            assert!(app.status_message.contains(
                                "group names could not be refreshed: fixture label failure"
                            ));
                        } else {
                            assert!(app.status_message.contains("Group names not refreshed"));
                        }
                        if primary_dirty {
                            assert!(app.status_message.starts_with("Read not applied:"));
                        } else if scan {
                            assert_eq!(app.scan_presets[0].start_freq, start_freq + 100);
                            assert!(app.status_message.starts_with("Scan presets loaded"));
                        } else {
                            assert_eq!(app.channels[0].name, "RADIO");
                            assert_eq!(app.endian, Endianness::Little);
                            assert!(app.status_message.starts_with("Channels loaded"));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn bundled_read_status_keeps_clean_success_and_partial_success_messages() {
    for scan in [false, true] {
        for labels_fail in [false, true] {
            for split_updates in [false, true] {
                let mut app = App::demo();
                app.mode = AppMode::Reading;
                finish_bundle(&mut app, scan, labels_fail, split_updates);
                let section = if scan { "Scan presets" } else { "Channels" };
                assert_eq!(
                    app.status_message,
                    if labels_fail {
                        format!(
                            "{section} loaded, but group names could not be refreshed: fixture label failure"
                        )
                    } else {
                        format!("{section} and group names loaded")
                    }
                );
            }
        }
    }
}

#[test]
fn bundled_read_status_does_not_latch_warnings_into_unrelated_statuses() {
    let mut app = App::demo();
    app.channels_dirty = true;
    app.group_labels_dirty = true;
    finish_bundle(&mut app, false, false, false);
    for status in [
        "Reading primary data",
        "Channels read complete: 3 loaded",
        "Scan presets saved",
        "Remote mode OFF",
    ] {
        app.event_tx.send(AppEvent::Status(status.into())).unwrap();
        app.update();
        assert_eq!(app.status_message, status);
    }
    app.channels_dirty = false;
    app.group_labels_dirty = false;
    finish_bundle(&mut app, false, false, true);
    assert_eq!(app.status_message, "Channels and group names loaded");
}
