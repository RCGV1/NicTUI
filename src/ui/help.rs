use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SURFACE_0, COLOR_TEXT,
    COLOR_WARNING,
};
use crate::app::{App, AppMode, MainTab};

fn popup(area: Rect) -> Rect {
    let width = area.width.saturating_sub(2).min(88);
    let height = area.height.saturating_sub(2).min(30);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn regions(area: Rect) -> [Rect; 3] {
    let inner = Block::bordered().inner(popup(area));
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(inner);
    [rows[0], rows[1], rows[2]]
}

pub fn help_page_rows(area: Rect) -> usize {
    usize::from(regions(area)[1].height).max(1)
}

pub(super) fn render_help(f: &mut Frame, app: &mut App) {
    let area = popup(f.area());
    if area.width < 4 || area.height < 4 {
        return;
    }
    let (title, lines) = content(app);
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Help: {title} | {} ", super::theme::VERSION))
            .border_style(Style::default().fg(COLOR_ACCENT))
            .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_0)),
        area,
    );
    let rows = regions(f.area());
    let environment = if app.simulator_mode {
        "Simulator only. Buttons cannot transmit RF; radio writes are disabled."
    } else if app.demo_mode {
        "Demo preview. Local editing is available; hardware I/O is disabled."
    } else {
        "Local edits are separate from the radio. Write commands apply them."
    };
    f.render_widget(
        Paragraph::new(environment)
            .style(Style::default().fg(COLOR_DIM))
            .wrap(Wrap { trim: true }),
        rows[0],
    );
    let lines = wrap_lines(lines, rows[1].width);
    let total = lines.len();
    let maximum = total
        .saturating_sub(usize::from(rows[1].height))
        .min(u16::MAX as usize) as u16;
    app.help.scroll = app.help.scroll.min(maximum);
    f.render_widget(Paragraph::new(lines).scroll((app.help.scroll, 0)), rows[1]);
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(
                    "Up/Down scroll | PgUp/PgDn | {}-{}/{}",
                    usize::from(app.help.scroll) + 1,
                    (usize::from(app.help.scroll) + usize::from(rows[1].height)).min(total),
                    total
                ),
                Style::default().fg(COLOR_PRIMARY),
            ),
            Line::styled(
                "Esc/F1 close | Ctrl+C quit safely",
                Style::default().fg(COLOR_DIM),
            ),
        ])
        .style(Style::default().bg(COLOR_SURFACE_0))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER)),
        ),
        rows[2],
    );
}

fn action(key: &str, description: &str, available: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<13} "),
            Style::default()
                .fg(if available { COLOR_PRIMARY } else { COLOR_DIM })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            description.to_owned(),
            Style::default().fg(if available { COLOR_TEXT } else { COLOR_DIM }),
        ),
    ])
}

fn heading(text: &str) -> Line<'static> {
    Line::styled(
        text.to_owned(),
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD),
    )
}

fn note(text: &str) -> Line<'static> {
    Line::styled(text.to_owned(), Style::default().fg(COLOR_WARNING))
}

fn wrap_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut wrapped = Vec::new();
    for line in lines {
        let glyphs: Vec<_> = line
            .spans
            .iter()
            .flat_map(|span| span.styled_graphemes(line.style))
            .map(|glyph| {
                (
                    glyph.symbol.to_owned(),
                    glyph.style,
                    Span::raw(glyph.symbol).width(),
                )
            })
            .collect();
        if glyphs.is_empty() {
            wrapped.push(Line::raw(""));
            continue;
        }
        let mut start = 0;
        while start < glyphs.len() {
            let mut end = start;
            let mut used = 0;
            let mut space = None;
            while end < glyphs.len() && used + glyphs[end].2 <= width {
                used += glyphs[end].2;
                if glyphs[end].0 == " " {
                    space = Some(end);
                }
                end += 1;
            }
            if end < glyphs.len() {
                end = space
                    .filter(|&index| index > start)
                    .map_or(end.max(start + 1), |index| index + 1);
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (symbol, style, _) in &glyphs[start..end] {
                if let Some(last) = spans.last_mut()
                    && last.style == *style
                {
                    last.content.to_mut().push_str(symbol);
                } else {
                    spans.push(Span::styled(symbol.clone(), *style));
                }
            }
            wrapped.push(Line::from(spans));
            start = end;
            while start < glyphs.len() && glyphs[start].0 == " " {
                start += 1;
            }
        }
    }
    wrapped
}

fn content(app: &App) -> (String, Vec<Line<'static>>) {
    let has_target = app.simulator_mode
        || app.protocol_port_name.is_some()
        || app.selected_port_candidate().is_some();
    let busy = app.remote_active
        || app.remote_tx.is_some()
        || app.activity_active()
        || app.ble_scan_in_progress;
    let ready = has_target && !app.demo_mode && !busy;
    let write_ready = ready && !app.simulator_mode;
    let mut lines = Vec::new();
    let title = match app.mode {
        AppMode::EditChannel(_)
        | AppMode::EditSetting(_)
        | AppMode::EditDTMF(_)
        | AppMode::EditScanPreset(_)
        | AppMode::EditGroupLabel(_)
        | AppMode::EditBandPlan(_) => {
            lines.push(heading("Edit this record"));
            match app.mode {
                AppMode::EditSetting(index) => {
                    match crate::protocol::SETTINGS_METADATA
                        .get(index)
                        .map(|meta| &meta.setting_type)
                    {
                        Some(
                            crate::protocol::SettingType::Enum(_)
                            | crate::protocol::SettingType::Boolean,
                        ) => {
                            lines.push(action("Up/Down", "Change the selected option", true));
                        }
                        Some(crate::protocol::SettingType::Numeric { .. }) => {
                            lines.extend([
                                action("Type", "Append to the numeric value", true),
                                action(
                                    "Backspace",
                                    "Remove the last character; clear before replacing",
                                    true,
                                ),
                            ]);
                        }
                        None => {}
                    }
                    lines.push(Line::raw(
                        "Cancel to return to the settings list and select another setting.",
                    ));
                }
                ref mode => {
                    if !matches!(mode, AppMode::EditGroupLabel(_)) {
                        lines.push(action(
                            "Up/Down/Tab",
                            "Choose a field (Shift+Tab goes back)",
                            true,
                        ));
                    }
                    if matches!(
                        mode,
                        AppMode::EditChannel(_)
                            | AppMode::EditScanPreset(_)
                            | AppMode::EditBandPlan(_)
                    ) {
                        lines.push(action("Left/Right", "Change an option", true));
                    }
                    lines.extend([
                        action("Type", "Append to a text or numeric field", true),
                        action(
                            "Backspace",
                            "Remove the last character; clear before replacing",
                            true,
                        ),
                    ]);
                }
            }
            lines.extend([
                action("Enter", "Validate and save to the local workspace", true),
                action("Esc", "Cancel this edit; keep the original record", true),
                note("Saving an editor does not write to the radio."),
                Line::raw("Close this help to continue exactly where you left off."),
            ]);
            "Editor".to_owned()
        }
        AppMode::DeleteChannelConfirm(_) => {
            lines.extend([
                heading("Delete a local channel"),
                action("Enter", "Confirm deletion in the workspace", true),
                action("Esc", "Keep the channel and return", true),
                note("Use u on Channels to undo a deletion before writing."),
            ]);
            "Delete Channel".to_owned()
        }
        AppMode::Reading | AppMode::Writing | AppMode::BinFlashing => {
            lines.extend([
                heading("Operation in progress"),
                note("Keep the connection open until cleanup finishes."),
                action(
                    "Ctrl+C",
                    "Queue a safe exit after the operation releases the radio",
                    true,
                ),
                Line::raw("Do not unplug the radio during writes or flashing."),
                Line::raw("Other commands are unavailable until this operation finishes."),
            ]);
            "Working".to_owned()
        }
        AppMode::Error(_) => {
            lines.extend([
                heading("Recover from an error"),
                action(
                    "Esc/Enter",
                    "Dismiss the error and return to the workspace",
                    true,
                ),
                Line::raw(
                    "Your unsaved workspace is kept. Check the selected target before retrying.",
                ),
                Line::raw("Open Logs for the full error details."),
            ]);
            "Error".to_owned()
        }
        AppMode::PortSelection => {
            lines.extend([
                heading("Choose a radio"),
                action(
                    "Up/Down",
                    "Select a discovered target",
                    !app.port_candidates.is_empty(),
                ),
                action(
                    "Enter",
                    "Open the selected target",
                    !app.port_candidates.is_empty(),
                ),
                action(
                    "r",
                    "Refresh available radio targets",
                    !app.simulator_mode && !app.demo_mode,
                ),
                action(
                    "b",
                    "Scan for Bluetooth radios",
                    !app.simulator_mode && !app.demo_mode,
                ),
                Line::raw("Turn the radio on. For USB, use its programming/data cable."),
                Line::raw("Bluetooth discovery can take several seconds."),
            ]);
            "Radio Targets".to_owned()
        }
        AppMode::Main(tab) => {
            let (name, mut commands) = match tab {
                MainTab::Channels => (
                    "Channels",
                    vec![
                        action(
                            "r",
                            "Read channels and group names",
                            ready && !app.channels_dirty && app.deleted_channels.is_empty(),
                        ),
                        action(
                            "Enter",
                            "Edit the selected channel locally",
                            app.channel_state
                                .selected()
                                .is_some_and(|i| i < app.channels.len()),
                        ),
                        action("n", "Create a local channel", true),
                        action(
                            "d / u",
                            "Delete selected channel / undo a deletion",
                            !app.channels.is_empty() || !app.deleted_channels.is_empty(),
                        ),
                        action("i / e", "Import / export a channel file", true),
                        action(
                            "w",
                            "Write changed channels to the radio",
                            write_ready && (app.channels_dirty || !app.deleted_channels.is_empty()),
                        ),
                    ],
                ),
                MainTab::Settings => (
                    "Settings",
                    vec![
                        action("r", "Read the radio settings", ready && !app.settings_dirty),
                        action(
                            "Enter",
                            "Edit the selected setting locally",
                            app.settings.is_some()
                                && app
                                    .settings_state
                                    .selected()
                                    .is_some_and(|i| i < crate::protocol::SETTINGS_METADATA.len()),
                        ),
                        action(
                            "w",
                            "Write settings, then reboot the radio",
                            write_ready && app.settings.is_some() && app.settings_dirty,
                        ),
                        note(
                            "A settings write reboots the radio; reconnect before the next operation.",
                        ),
                    ],
                ),
                MainTab::Scanning => (
                    "Scan Presets",
                    vec![
                        action(
                            "r",
                            "Read scan presets and group names",
                            ready && !app.scan_presets_dirty,
                        ),
                        action(
                            "Enter",
                            "Edit the selected scan preset locally",
                            app.preset_state
                                .selected()
                                .is_some_and(|i| i < app.scan_presets.len()),
                        ),
                        action(
                            "w",
                            "Write scan preset changes",
                            write_ready && !app.scan_presets.is_empty(),
                        ),
                        Line::raw("Frequencies and steps use the units shown in the editor."),
                    ],
                ),
                MainTab::MemoryGroups => (
                    "Memory Groups",
                    vec![
                        action("r", "Read group names", ready && !app.group_labels_dirty),
                        action("Enter", "Rename a group locally", true),
                        action(
                            "w",
                            "Write changed group names",
                            write_ready && app.group_labels_dirty,
                        ),
                        Line::raw("Renaming a group does not move channels between groups."),
                    ],
                ),
                MainTab::BandPlan => (
                    "Band Plan",
                    vec![
                        action("r", "Read band definitions", ready && !app.band_plans_dirty),
                        action(
                            "Enter",
                            "Edit the selected band locally",
                            app.bandplan_state
                                .selected()
                                .is_some_and(|i| i < app.band_plans.len()),
                        ),
                        action(
                            "w",
                            "Write band plan changes",
                            write_ready && !app.band_plans.is_empty(),
                        ),
                    ],
                ),
                MainTab::DTMF => (
                    "DTMF",
                    vec![
                        action("r", "Read DTMF presets", ready && !app.dtmf_dirty),
                        action(
                            "Enter",
                            "Edit the selected preset locally",
                            app.dtmf_state
                                .selected()
                                .is_some_and(|i| i < app.dtmf_presets.len()),
                        ),
                        action(
                            "w",
                            "Write DTMF preset changes",
                            write_ready && !app.dtmf_presets.is_empty(),
                        ),
                        Line::raw("Editing a DTMF preset does not transmit its tones."),
                    ],
                ),
                MainTab::Remote => (
                    "Remote",
                    vec![
                        action(
                            "o / p",
                            "Start / stop the Remote session",
                            has_target || app.demo_mode,
                        ),
                        action(
                            "Mouse",
                            "Click a key; hold and release PTT",
                            app.remote_controls_ready(),
                        ),
                        action(
                            "Space",
                            if app.remote_keyboard_release_supported {
                                "Hold PTT until key release"
                            } else {
                                "Tap PTT; use the mouse for a sustained hold"
                            },
                            app.remote_controls_ready(),
                        ),
                        action(
                            "m/u/d/e",
                            "Menu / Up / Down / Exit",
                            app.remote_controls_ready(),
                        ),
                        action("f / v", "Radio F / B buttons", app.remote_controls_ready()),
                        action("D", "Show or hide connection details", true),
                        note(if app.simulator_mode {
                            "These are simulated controls; no RF can be transmitted."
                        } else if app.demo_mode {
                            "Demo controls do not access a radio or transmit RF."
                        } else {
                            "PTT can transmit. Keypad actions may change saved radio state."
                        }),
                        Line::raw("Opening help releases any held key."),
                        Line::raw(
                            "While Remote is running, number keys operate the keypad, not view navigation.",
                        ),
                    ],
                ),
                MainTab::Activity => (
                    "Activity",
                    vec![
                        action(
                            "s",
                            "Start or stop Activity capture",
                            !app.remote_active && app.remote_tx.is_none(),
                        ),
                        action("Up/Down", "Select a captured frequency", true),
                        action("e", "Export captured results to CSV", true),
                        action(
                            "c",
                            "Clear results after stopping capture",
                            !app.activity_active(),
                        ),
                        Line::raw(
                            "Activity observes the radio's scan results. Stop capture before programming reads or writes.",
                        ),
                        note(if app.simulator_mode {
                            "Results come from the local simulator, not live RF."
                        } else {
                            "Attach an antenna and configure a scan range on the radio."
                        }),
                    ],
                ),
                MainTab::Codeplug => (
                    "Codeplug",
                    vec![
                        action(
                            "r",
                            "Read a full EEPROM snapshot",
                            ready && !app.configuration_has_unsaved_edits(),
                        ),
                        action("i", "Import a codeplug file", true),
                        action(
                            "e",
                            "Export the unchanged full snapshot",
                            app.codeplug_snapshot_is_current(),
                        ),
                        action(
                            "w",
                            "Restore the unchanged snapshot to a radio",
                            super::views::codeplug_write_block_reason(app).is_none() && write_ready,
                        ),
                        note(
                            "A full snapshot is separate from local table edits. Editing records does not merge them into snapshot bytes.",
                        ),
                    ],
                ),
                MainTab::BinFlash => (
                    "Firmware",
                    vec![
                        action("i", "Choose a firmware file for validation", true),
                        action(
                            "f",
                            "Flash loaded firmware over USB after bootloader entry",
                            write_ready
                                && app
                                    .bin_firmware_data
                                    .as_ref()
                                    .is_some_and(|data| !data.is_empty() && data.len() <= 0xF800)
                                && !app.ble_transport_selected()
                                && app.protocol_port_name.as_deref().is_some_and(|port| {
                                    crate::ble::parse_ble_device_uri(port).is_none()
                                }),
                        ),
                        note("Flashing is unavailable over Bluetooth and in simulator/demo modes."),
                        Line::raw("Follow the radio's bootloader instructions before flashing."),
                    ],
                ),
                MainTab::Debug => (
                    "Logs",
                    vec![
                        Line::raw(
                            "RX/TX describe transport data, not received or transmitted voice.",
                        ),
                        action("Up/Down", "Browse the recent history", true),
                        action("PgUp/PgDn", "Scroll a page", true),
                        action("End", "Return to live following", true),
                        action("f", "Cycle traffic and error filters", true),
                        action("c", "Clear local log history", true),
                        Line::raw(
                            "Filtering and clearing logs do not change the radio or workspace.",
                        ),
                    ],
                ),
            };
            lines.push(heading(name));
            lines.append(&mut commands);
            if tab != MainTab::Remote && tab != MainTab::Debug {
                lines.extend([
                    Line::raw(""),
                    heading("Read and edit safely"),
                    action(
                        "Shift+R",
                        "Read all configuration sections (Ctrl+R also works)",
                        ready && !app.configuration_has_unsaved_edits(),
                    ),
                ]);
                if !has_target {
                    lines.push(note("Select a radio before reading or writing."));
                } else if busy {
                    lines.push(note("Stop the active session and wait for cleanup before another radio operation."));
                }
                if app.configuration_has_unsaved_edits() {
                    lines.push(note(
                        "Read All (Shift+R) is unavailable while local edits are pending.",
                    ));
                }
            }
            name.to_owned()
        }
    };
    if matches!(app.mode, AppMode::Main(_) | AppMode::PortSelection) {
        lines.extend([
            Line::raw(""),
            heading("Move around"),
            action("Tab/Shift+Tab", "Next / previous view", true),
            action("a", "Open Activity", true),
            action(
                "0-9",
                "Jump to a view (outside an active Remote session)",
                true,
            ),
            action("Home/End", "First / last row in tables", true),
            action("PgUp/PgDn", "Move through long tables", true),
        ]);
    }
    lines.extend([
        Line::raw(""),
        heading("Help and exit"),
        action("F1 / ?", "Open this guide (? is text inside editors)", true),
        action(
            "Esc/F1/q",
            "Close help without changing your workspace",
            true,
        ),
        action(
            "Ctrl+C",
            "Quit safely; wait for radio cleanup if needed",
            true,
        ),
        Line::raw("Dimmed commands are unavailable in the current state."),
    ]);
    (title, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use ratatui::{Terminal, backend::TestBackend};

    fn text(app: &App) -> String {
        content(app)
            .1
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn enabled(app: &App, key: &str) -> bool {
        content(app)
            .1
            .iter()
            .find_map(|line| {
                let span = line.spans.first()?;
                (span.content.trim() == key).then_some(span.style.fg == Some(COLOR_PRIMARY))
            })
            .unwrap_or_else(|| panic!("Missing help action {key}"))
    }

    #[test]
    fn help_actions_always_separate_keys_from_descriptions() {
        for key in [
            "r",
            "Up/Down/Tab",
            "Tab/Shift+Tab",
            "Longer-than-key-column",
        ] {
            for available in [false, true] {
                let line = action(key, "Description", available);
                assert!(line.spans[0].content.ends_with(' '));
                assert!(line.to_string().contains(&format!("{key} ")));
                assert!(!line.to_string().contains(&format!("{key}Description")));
            }
        }
    }

    #[test]
    fn long_help_shortcuts_stay_separated_at_narrow_and_review_sizes() {
        for (width, height) in [(60, 20), (80, 24), (120, 36), (160, 48), (190, 80)] {
            let mut app = App::demo();
            app.mode = AppMode::Main(MainTab::Channels);
            let body = regions(Rect::new(0, 0, width, height))[1];
            let lines = wrap_lines(content(&app).1, body.width);
            assert!(
                lines
                    .iter()
                    .all(|line| line.width() <= usize::from(body.width))
            );
            app.help.scroll = lines
                .iter()
                .position(|line| line.to_string().contains("Tab/Shift+Tab"))
                .unwrap() as u16;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| render_help(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let text = (body.y..body.bottom())
                .map(|y| {
                    (body.x..body.right())
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("Tab/Shift+Tab Next / previous view"),
                "{width}x{height}:\n{text}"
            );
            assert!(!text.contains("Tab/Shift+TabNext"), "{text}");
        }
    }

    fn firmware_fixture() -> App {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::BinFlash);
        app.protocol_port_name = Some("fixture://usb".into());
        app.bin_file_path = Some("/local/fixture/firmware.bin".into());
        app.bin_firmware_data = Some(vec![0; 1234]);
        app
    }

    #[test]
    fn firmware_help_requires_valid_loaded_bytes_and_a_non_ble_target() {
        for size in [None, Some(0), Some(1), Some(63488), Some(63489)] {
            let mut app = firmware_fixture();
            app.bin_firmware_data = size.map(|size| vec![0; size]);
            assert_eq!(enabled(&app, "f"), matches!(size, Some(1..=63488)));
        }
        let mut app = firmware_fixture();
        app.bin_file_path = None;
        assert!(
            enabled(&app, "f"),
            "The handler uses loaded bytes, not the filename"
        );
        app.protocol_port_name = None;
        assert!(!enabled(&app, "f"));
        for port in ["ble://fixture", "ble:fixture"] {
            app.protocol_port_name = Some(port.into());
            assert!(!enabled(&app, "f"));
        }
        // A BLE bridge can have a serial path but is still not a USB flash target.
        let mut app = App::demo();
        app.demo_mode = false;
        app.mode = AppMode::Main(MainTab::BinFlash);
        app.protocol_port_name = Some("fixture://bridge".into());
        app.bin_firmware_data = Some(vec![0; 1234]);
        assert!(!enabled(&app, "f"));
        for (demo, simulated) in [(true, false), (false, true), (true, true)] {
            let mut app = firmware_fixture();
            app.demo_mode = demo;
            app.simulator_mode = simulated;
            assert!(!enabled(&app, "f"));
        }
    }

    #[test]
    fn remote_help_warns_about_transmission_only_for_physical_radios() {
        for (demo, simulated) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut app = App::new();
            app.mode = AppMode::Main(MainTab::Remote);
            app.demo_mode = demo;
            app.simulator_mode = simulated;
            let text = text(&app);
            assert_eq!(text.contains("PTT can transmit"), !demo && !simulated);
            assert_eq!(
                text.contains("may change saved radio state"),
                !demo && !simulated
            );
            if simulated {
                assert!(text.contains("no RF can be transmitted"));
            } else if demo {
                assert!(text.contains("Demo controls do not access a radio or transmit RF"));
            }
        }
    }

    #[test]
    fn settings_help_matches_dirty_write_guard_without_starting_io() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Settings);
        app.protocol_port_name = Some("fixture://no-io".into());
        app.settings = Some(crate::protocol::SettingsBlock::default());
        assert!(!enabled(&app, "w"));
        let before = app.workspace_snapshot();
        app.start_write_settings_and_reboot();
        assert_eq!(app.status_message, "No settings changes to write");
        assert_eq!(app.mode, AppMode::Main(MainTab::Settings));
        assert_eq!(app.workspace_snapshot(), before);
        app.settings_dirty = true;
        assert!(enabled(&app, "w"));
        app.settings = None;
        assert!(!enabled(&app, "w"));
    }

    #[test]
    fn settings_editor_help_matches_option_keys_and_single_setting_routing() {
        let mut app = App::demo();
        app.settings_state.select(Some(3));
        app.start_edit_setting();
        app.selection_index = 0;
        let guidance = text(&app);
        assert!(guidance.contains("Up/Down") && guidance.contains("Change the selected option"));
        assert!(!guidance.contains("Choose a field") && !guidance.contains("Left/Right"));
        assert!(!guidance.contains("Tab") && !guidance.contains("Replace the active"));
        app.handle_settings_editor_key(KeyCode::Down);
        assert_eq!(app.selection_index, 1);
        app.handle_settings_editor_key(KeyCode::Up);
        assert_eq!(app.selection_index, 0);
        for key in [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Tab,
            KeyCode::BackTab,
        ] {
            app.handle_settings_editor_key(key);
            assert_eq!(app.selection_index, 0);
            assert_eq!(app.mode, AppMode::EditSetting(3));
        }
        app.settings_state.select(Some(0));
        app.start_edit_setting();
        app.edit_buffer = "4".into();
        assert!(text(&app).contains("Append to the numeric value"));
        assert!(text(&app).contains("Backspace"));
        app.handle_settings_editor_key(KeyCode::Char('7'));
        assert_eq!(app.edit_buffer, "47");
        app.handle_settings_editor_key(KeyCode::Backspace);
        assert_eq!(app.edit_buffer, "4");
    }

    #[test]
    fn text_editor_help_describes_append_and_backspace_for_actual_handlers() {
        type EditorCase = (AppMode, fn(&mut App, KeyCode));
        let editors: [EditorCase; 5] = [
            (AppMode::EditChannel(0), App::handle_channel_editor_key),
            (AppMode::EditDTMF(0), App::handle_dtmf_editor_key),
            (
                AppMode::EditScanPreset(0),
                App::handle_scan_preset_editor_key,
            ),
            (AppMode::EditBandPlan(0), App::handle_bandplan_editor_key),
            (
                AppMode::EditGroupLabel(0),
                App::handle_group_label_editor_key,
            ),
        ];
        for (mode, handle) in editors {
            let mut app = App::demo();
            app.mode = mode.clone();
            app.edit_buffer = "1".into();
            let text = text(&app);
            assert!(text.contains("Append to a text or numeric field"));
            assert!(text.contains("Backspace") && text.contains("clear before replacing"));
            assert!(!text.contains("Replace the active"));
            if matches!(mode, AppMode::EditGroupLabel(_)) {
                assert!(!text.contains("Choose a field"));
            }
            if matches!(mode, AppMode::EditGroupLabel(_) | AppMode::EditDTMF(_)) {
                assert!(!text.contains("Left/Right"));
            }
            handle(&mut app, KeyCode::Char('2'));
            assert_eq!(app.edit_buffer, "12");
            handle(&mut app, KeyCode::Backspace);
            assert_eq!(app.edit_buffer, "1");
            assert_eq!(app.mode, mode);
        }
    }

    #[test]
    fn dirty_help_limits_read_protection_claim_to_read_all() {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Codeplug);
        app.channels_dirty = true;
        let text = text(&app);
        assert!(text.contains("Read All (Shift+R) is unavailable while local edits are pending"));
        assert!(!text.contains("protected from replacement by reads"));
        assert!(!enabled(&app, "Shift+R"));
    }

    #[test]
    fn contextual_help_covers_every_view_without_changing_workspace() {
        for tab in [
            MainTab::Channels,
            MainTab::Settings,
            MainTab::Scanning,
            MainTab::MemoryGroups,
            MainTab::BandPlan,
            MainTab::DTMF,
            MainTab::Remote,
            MainTab::Activity,
            MainTab::Codeplug,
            MainTab::BinFlash,
            MainTab::Debug,
        ] {
            for (width, height) in [(60, 20), (80, 24), (120, 36), (190, 80)] {
                let mut app = App::demo();
                app.simulator_mode = true;
                app.mode = AppMode::Main(tab);
                app.help.open = true;
                let before = serde_json::to_value(&app.channels).unwrap();
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| crate::ui::ui(f, &mut app)).unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("Help:"), "{tab:?}/{width}");
                assert!(text.contains("Esc/F1 close"), "{tab:?}/{width}");
                assert_eq!(serde_json::to_value(&app.channels).unwrap(), before);
                assert_eq!(app.mode, AppMode::Main(tab));
            }
        }
    }

    #[test]
    fn help_scroll_is_bounded_and_does_not_cancel_an_editor() {
        let mut app = App::demo();
        app.start_edit_channel();
        app.edit_buffer = "UNSAVED".into();
        app.help.open = true;
        app.help.scroll = u16::MAX;
        let mode = app.mode.clone();
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal.draw(|f| crate::ui::ui(f, &mut app)).unwrap();
        assert!(app.help.scroll < u16::MAX);
        assert_eq!(app.edit_buffer, "UNSAVED");
        assert_eq!(app.mode, mode);
    }

    #[test]
    fn target_error_and_busy_help_remain_readable_in_all_palettes() {
        for mode in [
            AppMode::PortSelection,
            AppMode::Reading,
            AppMode::Writing,
            AppMode::BinFlashing,
            AppMode::DeleteChannelConfirm(0),
            AppMode::Error("Connection unavailable".into()),
        ] {
            for palette in [
                crate::ui::TerminalPalette::TrueColor,
                crate::ui::TerminalPalette::Ansi256,
                crate::ui::TerminalPalette::Ansi16,
                crate::ui::TerminalPalette::Monochrome,
            ] {
                let mut app = App::demo();
                app.mode = mode.clone();
                app.help.open = true;
                app.edit_buffer = "UNSAVED".into();
                let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
                terminal
                    .draw(|frame| {
                        crate::ui::ui(frame, &mut app);
                        palette.apply(frame.buffer_mut());
                    })
                    .unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("Help:"));
                assert!(text.contains("Esc/F1 close"));
                assert_eq!(app.edit_buffer, "UNSAVED");
                assert_eq!(app.mode, mode);
            }
        }
    }
}
