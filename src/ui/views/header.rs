use crate::app::{App, AppMode, MainTab};
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_ERROR, COLOR_HEADER, COLOR_PRIMARY, COLOR_SUCCESS,
    COLOR_SURFACE_2, COLOR_SURFACE_3, COLOR_TEXT, COLOR_WARNING,
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
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
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
                if compact { "" } else { " Live Radio Workbench" },
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

    let selected_port = app
        .protocol_port_name
        .as_deref()
        .or_else(|| {
            app.selected_port_candidate()
                .map(|candidate| candidate.port_name.as_str())
        })
        .unwrap_or("not selected");
    let middle_text = if compact {
        format!(" {tab_label} | {}", compact_port_name(selected_port))
    } else {
        format!(
            " {tab_label}  |  {tab_summary}  |  {}",
            compact_port_name(selected_port)
        )
    };
    let bottom_text = if compact {
        format!(
            "{} | {}",
            tab_summary,
            if has_unsaved_state(app) {
                "dirty"
            } else {
                "clean"
            }
        )
    } else {
        format!(
            "{}  |  Port {}  |  {}",
            app.status_message,
            compact_port_name(selected_port),
            dirty_summary(app)
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
            Span::styled(middle_text, Style::default().fg(COLOR_TEXT)),
        ])),
        rows[1],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            bottom_text,
            Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_2),
        ))
        .wrap(ratatui::widgets::Wrap { trim: true }),
        rows[2],
    );
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
        (MainTab::Debug, _) => "Debug",
    }
}

fn header_status(app: &App) -> (&'static str, ratatui::style::Color) {
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
        _ if app.remote_active => ("REMOTE", COLOR_SUCCESS),
        _ if has_unsaved_state(app) => ("DIRTY", COLOR_WARNING),
        _ if app.protocol_port_name.is_some() => ("CONNECTED", COLOR_ACCENT),
        _ => ("NO RADIO", COLOR_DIM),
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
                if compact {
                    format!("{} set", crate::protocol::SETTINGS_METADATA.len())
                } else {
                    format!("{} fields", crate::protocol::SETTINGS_METADATA.len())
                }
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
                .enumerate()
                .fold([false; 16], |mut used, (_, channel)| {
                    for group in channel.groups {
                        if (1..=16).contains(&group) {
                            used[(group - 1) as usize] = true;
                        }
                    }
                    used
                })
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
            if app.remote_active {
                if compact {
                    "live".to_string()
                } else {
                    "live session".to_string()
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
    }
}

fn has_unsaved_state(app: &App) -> bool {
    app.channels_dirty
        || app.settings_dirty
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

fn compact_port_name(value: &str) -> String {
    value.rsplit('/').next().unwrap_or(value).to_string()
}
