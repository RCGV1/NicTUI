use crate::app::activity::ActivityStatus;
use crate::app::{App, AppMode, MainTab};
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_ERROR, COLOR_HEADER, COLOR_PRIMARY,
    COLOR_SURFACE_3, COLOR_TEXT, COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(COLOR_BORDER))
        .bg(COLOR_HEADER);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    let compact = inner.width < 90;
    let top_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(if compact { 18 } else { 28 }),
            Constraint::Length(if compact { 12 } else { 20 }),
        ])
        .split(rows[0]);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " NicTUI ",
                Style::default()
                    .fg(COLOR_HEADER)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if app.simulator_mode {
                    if compact {
                        ""
                    } else {
                        " Radio Simulator | no RF"
                    }
                } else if compact {
                    ""
                } else {
                    " Live Radio Workbench"
                },
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
            ),
        ])),
        top_row[0],
    );

    let status = header_status(app);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {} ", status.0),
            Style::default()
                .fg(COLOR_HEADER)
                .bg(status.1)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(ratatui::layout::Alignment::Right),
        top_row[1],
    );

    let active_tab = match app.mode {
        AppMode::Main(tab) => tab,
        _ => app.last_main_tab,
    };
    let tab_label = tab_label(active_tab, compact);
    let tab_summary = tab_summary(app, active_tab, compact);

    let selected_port = app.selected_port_short_label();
    let state_summary = dirty_summary(app);
    let status_message = header_message(app, active_tab);
    let action_notice = status_message.to_ascii_lowercase();
    let detail_text = if [
        "disabled",
        "unsupported",
        "blocked",
        "failed",
        "invalid",
        "not applied",
        "not refreshed",
    ]
    .iter()
    .any(|word| action_notice.contains(word))
    {
        if compact {
            format!(" {tab_label} | {status_message}")
        } else {
            format!(" {tab_label} | {status_message} | {state_summary}")
        }
    } else if compact {
        format!(" {tab_label} | {tab_summary} | {state_summary}")
    } else if has_unsaved_state(app) {
        if status_message.trim().is_empty() {
            format!(" {tab_label}  |  {tab_summary}  |  {state_summary}  |  {selected_port}")
        } else {
            format!(
                " {tab_label}  |  {tab_summary}  |  {state_summary}  |  {selected_port}  |  {}",
                status_message
            )
        }
    } else if status_message.trim().is_empty() {
        format!(" {tab_label}  |  {tab_summary}  |  {selected_port}  |  {state_summary}")
    } else {
        format!(
            " {tab_label}  |  {tab_summary}  |  {selected_port}  |  {}  |  {state_summary}",
            status_message
        )
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " TAB ",
                Style::default()
                    .fg(COLOR_HEADER)
                    .bg(COLOR_SURFACE_3)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(detail_text, Style::default().fg(COLOR_TEXT)),
        ])),
        rows[1],
    );
}

fn header_message(app: &App, tab: MainTab) -> &str {
    // Activity's state summary and panel already cover these lifecycle notices.
    if tab == MainTab::Activity
        && matches!(
            app.status_message.as_str(),
            "Activity capture owns the radio. Stop capture before other radio actions."
                | "Activity capture owns the radio; stop capture and wait for cleanup first"
                | "Stopping Activity; waiting for radio cleanup."
                | "Activity stopped; radio released."
                | "Activity capture failed; radio released."
        )
    {
        ""
    } else {
        &app.status_message
    }
}

fn tab_label(tab: MainTab, compact: bool) -> &'static str {
    match (tab, compact) {
        (MainTab::Channels, _) => "Channels",
        (MainTab::Settings, _) => "Settings",
        (MainTab::Scanning, true) => "Scan",
        (MainTab::Scanning, false) => "Scan Presets",
        (MainTab::MemoryGroups, true) => "Groups",
        (MainTab::MemoryGroups, false) => "Memory Groups",
        (MainTab::BandPlan, true) => "Band",
        (MainTab::BandPlan, false) => "Band Plan",
        (MainTab::DTMF, _) => "DTMF",
        (MainTab::Remote, _) => "Remote",
        (MainTab::Codeplug, _) => "Codeplug",
        (MainTab::BinFlash, true) => "Flash",
        (MainTab::BinFlash, false) => "BIN Flash",
        (MainTab::Debug, _) => "Logs",
        (MainTab::Activity, _) => "Activity [A]",
    }
}

fn header_status(app: &App) -> (&'static str, ratatui::style::Color) {
    if app.simulator_mode {
        return ("SIMULATOR", COLOR_WARNING);
    }
    match app.mode {
        AppMode::Reading => ("READING", COLOR_ACCENT),
        AppMode::Writing => ("WRITING", COLOR_WARNING),
        AppMode::BinFlashing => ("FLASHING", COLOR_WARNING),
        AppMode::EditChannel(_)
        | AppMode::EditSetting(_)
        | AppMode::EditDTMF(_)
        | AppMode::EditScanPreset(_)
        | AppMode::EditGroupLabel(_)
        | AppMode::EditBandPlan(_) => ("EDITING", COLOR_ACCENT),
        AppMode::DeleteChannelConfirm(_) => ("CONFIRM", COLOR_WARNING),
        AppMode::Error(_) => ("ERROR", COLOR_ERROR),
        _ if app.activity_active() => ("ACTIVITY", COLOR_WARNING),
        _ if app.remote_active => ("SESSION", COLOR_WARNING),
        _ if has_unsaved_state(app) => ("DIRTY", COLOR_WARNING),
        _ if app.selected_port_candidate().is_some() => ("TARGET", COLOR_PRIMARY),
        _ => ("NO TARGET", COLOR_DIM),
    }
}

fn tab_summary(app: &App, tab: MainTab, compact: bool) -> String {
    match tab {
        MainTab::Channels => {
            if app.channels.is_empty() {
                "empty".to_string()
            } else if compact {
                format!("{} ch", app.channels.len())
            } else {
                format!("{} loaded", app.channels.len())
            }
        }
        MainTab::Settings => {
            if app.settings.is_some() {
                format!("{} fields", crate::protocol::SETTINGS_METADATA.len())
            } else {
                "empty".to_string()
            }
        }
        MainTab::Scanning => {
            if app.scan_presets.is_empty() {
                "empty".to_string()
            } else if compact {
                format!("{} scan", app.scan_presets.len())
            } else {
                format!("{} presets", app.scan_presets.len())
            }
        }
        MainTab::MemoryGroups => {
            let named = app
                .group_labels
                .iter()
                .filter(|label| !label.trim().is_empty())
                .count();
            let populated = app
                .channels
                .iter()
                .filter(|channel| !app.deleted_channels.contains(&channel.channel_num))
                .enumerate()
                .fold(
                    [false; crate::protocol::GROUP_LABEL_COUNT],
                    |mut used, (_, channel)| {
                        for group in channel.groups {
                            if (1..=crate::protocol::GROUP_LABEL_COUNT as u8).contains(&group) {
                                used[(group - 1) as usize] = true;
                            }
                        }
                        used
                    },
                )
                .into_iter()
                .filter(|used| *used)
                .count();
            if named == 0 && populated == 0 {
                "empty".to_string()
            } else if compact {
                format!("{populated} grp | {named} lbl")
            } else {
                format!("{populated} used | {named} named")
            }
        }
        MainTab::BandPlan => {
            if app.band_plans.is_empty() {
                "empty".to_string()
            } else if compact {
                format!("{} plan", app.band_plans.len())
            } else {
                format!("{} plans", app.band_plans.len())
            }
        }
        MainTab::DTMF => {
            if app.dtmf_presets.is_empty() {
                "empty".to_string()
            } else if compact {
                format!("{} dtmf", app.dtmf_presets.len())
            } else {
                format!("{} presets", app.dtmf_presets.len())
            }
        }
        MainTab::Remote => {
            if app.simulator_mode {
                "local model | no RF".to_string()
            } else if app.remote_active {
                if compact {
                    "open".to_string()
                } else {
                    "session open".to_string()
                }
            } else {
                "idle".to_string()
            }
        }
        MainTab::Codeplug => {
            if app.codeplug_data.is_some() {
                "loaded".to_string()
            } else {
                "empty".to_string()
            }
        }
        MainTab::BinFlash => {
            if app.bin_firmware_data.is_some() {
                if compact {
                    "bin".to_string()
                } else {
                    "bin ready".to_string()
                }
            } else if compact {
                "empty".to_string()
            } else {
                "awaiting bin".to_string()
            }
        }
        MainTab::Debug => format!("{} logs", app.logs.len()),
        MainTab::Activity => {
            let state = app.activity.lock().unwrap();
            if state.stopping {
                "Stopping capture"
            } else {
                match state.status {
                    ActivityStatus::Ready => "Ready to capture",
                    ActivityStatus::Listening => "Receiving activity",
                    ActivityStatus::Waiting => "Waiting for activity",
                    ActivityStatus::Stopped => "Capture stopped",
                    ActivityStatus::Error => "Capture error; see panel",
                }
            }
            .to_string()
        }
    }
}

fn has_unsaved_state(app: &App) -> bool {
    app.channels_dirty
        || app.settings_dirty
        || app.scan_presets_dirty
        || app.band_plans_dirty
        || app.dtmf_dirty
        || app.group_labels_dirty
        || !app.deleted_channels.is_empty()
}

fn dirty_summary(app: &App) -> String {
    let mut parts = Vec::new();
    if app.channels_dirty || !app.deleted_channels.is_empty() {
        parts.push("channels*");
    }
    if app.settings_dirty {
        parts.push("settings*");
    }
    if app.scan_presets_dirty {
        parts.push("scan*");
    }
    if app.band_plans_dirty {
        parts.push("band*");
    }
    if app.dtmf_dirty {
        parts.push("dtmf*");
    }
    if app.group_labels_dirty {
        parts.push("groups*");
    }

    if parts.is_empty() {
        "workspace clean".to_string()
    } else {
        format!("pending {}", parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn header_detail(app: &App, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 38)).unwrap();
        terminal
            .draw(|frame| render_header(frame, app, Rect::new(0, 0, width, 3)))
            .unwrap();
        (0..width)
            .map(|x| terminal.backend().buffer()[(x, 1)].symbol())
            .collect::<String>()
    }

    #[test]
    fn simulator_banner_survives_tabs_dirty_state_and_errors() {
        let mut app = App::demo();
        app.simulator_mode = true;
        app.channels_dirty = true;
        for mode in [
            AppMode::Main(MainTab::Remote),
            AppMode::Main(MainTab::Activity),
            AppMode::Reading,
            AppMode::Error("test failure".into()),
        ] {
            app.mode = mode;
            assert_eq!(header_status(&app).0, "SIMULATOR");
            for width in [40, 80, 160] {
                let mut terminal = Terminal::new(TestBackend::new(width, 3)).unwrap();
                terminal
                    .draw(|frame| render_header(frame, &app, frame.area()))
                    .unwrap();
                let top = (0..width)
                    .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
                    .collect::<String>();
                assert!(top.contains("SIMULATOR"), "{top}");
                assert!(!top.contains("Live Radio"), "{top}");
            }
        }
    }

    #[test]
    fn activity_waiting_header_fits_without_repeating_ownership() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Activity);
        app.protocol_port_name = Some("/dev/cu.usbserial-TEST".into());
        app.status_message =
            "Activity capture owns the radio. Stop capture before other radio actions.".into();
        app.activity.lock().unwrap().status = ActivityStatus::Waiting;

        for width in [80, 100, 140] {
            let detail = header_detail(&app, width);
            assert!(detail.contains("Activity [A]"), "{width}: {detail}");
            assert!(detail.contains("Waiting for activity"), "{width}: {detail}");
            assert!(detail.contains("workspace clean"), "{width}: {detail}");
            assert!(!detail.contains("owns the radio"), "{width}: {detail}");
            if width >= 90 {
                assert!(detail.contains("usbserial-TEST"), "{width}: {detail}");
            }
        }
        assert!(
            app.status_message
                .contains("Stop capture before other radio actions.")
        );
        let state = app.activity.lock().unwrap();
        assert_eq!(state.hit_count, 0);
        assert_eq!(state.heartbeat_count, 0);
    }

    #[test]
    fn activity_header_reports_each_state_and_stopping() {
        let app = App::new();
        for (status, expected) in [
            (ActivityStatus::Ready, "Ready to capture"),
            (ActivityStatus::Listening, "Receiving activity"),
            (ActivityStatus::Waiting, "Waiting for activity"),
            (ActivityStatus::Stopped, "Capture stopped"),
            (ActivityStatus::Error, "Capture error; see panel"),
        ] {
            app.activity.lock().unwrap().status = status;
            assert_eq!(tab_summary(&app, MainTab::Activity, false), expected);
            assert_eq!(tab_summary(&app, MainTab::Activity, true), expected);
        }
        app.activity.lock().unwrap().stopping = true;
        assert_eq!(
            tab_summary(&app, MainTab::Activity, false),
            "Stopping capture"
        );
    }

    #[test]
    fn activity_header_preserves_dirty_state_and_action_feedback() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Activity);
        app.scan_presets_dirty = true;
        app.status_message =
            "Activity capture owns the radio; stop capture and wait for cleanup first".into();
        let detail = header_detail(&app, 140);
        assert!(detail.contains("pending scan*"));
        assert!(!detail.contains("owns the radio"));
        assert!(!detail.trim_end().ends_with('|'));

        for message in [
            "Activity CSV saved.",
            "Could not save Activity CSV. Check the destination and try again.",
            "Select a radio target before starting Activity.",
        ] {
            app.status_message = message.into();
            assert_eq!(header_message(&app, MainTab::Activity), message);
            assert!(header_detail(&app, 180).contains(message));
        }
    }

    #[test]
    fn activity_lifecycle_notice_filter_does_not_change_other_tabs() {
        let mut app = App::new();
        for message in [
            "Activity capture owns the radio. Stop capture before other radio actions.",
            "Activity capture owns the radio; stop capture and wait for cleanup first",
            "Stopping Activity; waiting for radio cleanup.",
            "Activity stopped; radio released.",
            "Activity capture failed; radio released.",
        ] {
            app.status_message = message.into();
            assert_eq!(header_message(&app, MainTab::Activity), "");
            assert_eq!(header_message(&app, MainTab::Channels), message);
            assert_eq!(header_message(&app, MainTab::Remote), message);
        }
    }

    #[test]
    fn open_remote_session_is_not_a_green_control_verdict() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        assert_eq!(header_status(&app), ("SESSION", COLOR_WARNING));
        assert_eq!(tab_summary(&app, MainTab::Remote, true), "open");
    }

    #[test]
    fn scan_and_band_changes_are_unsaved_workspace_state() {
        let mut app = App::new();
        app.scan_presets_dirty = true;
        assert!(has_unsaved_state(&app));
        assert_eq!(dirty_summary(&app), "pending scan*");
        assert_eq!(header_status(&app), ("DIRTY", COLOR_WARNING));
        app.band_plans_dirty = true;
        assert_eq!(dirty_summary(&app), "pending scan* band*");
        app.scan_presets_dirty = false;
        assert_eq!(dirty_summary(&app), "pending band*");
        app.band_plans_dirty = false;
        assert_eq!(dirty_summary(&app), "workspace clean");
    }
}
