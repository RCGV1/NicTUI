use crate::app::{App, AppMode};
use crate::protocol::EEPROM_SIZE;
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SUCCESS, COLOR_SURFACE_1, COLOR_TEXT,
    COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub fn render_codeplug_view(f: &mut Frame, app: &mut App, area: Rect) {
    let snapshot_reason = snapshot_block_reason(app);
    let write_reason = write_block_reason(app, snapshot_reason);
    let compact = area.width < 90;
    let shared_reason = compact && snapshot_reason.is_some() && snapshot_reason == write_reason;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(if shared_reason { 5 } else { 6 }),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    let full_snapshot = app
        .codeplug_data
        .as_ref()
        .is_some_and(|data| data.len() == EEPROM_SIZE);
    let settings_loaded = app.settings.is_some();
    let summary = Paragraph::new(format!(
        "{} channels | settings {} | snapshot {}",
        app.channels.len(),
        if settings_loaded { "loaded" } else { "empty" },
        if full_snapshot { "full" } else { "missing" }
    ))
    .style(Style::default().fg(COLOR_DIM))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(summary, chunks[0]);

    let mut readiness = vec![
        Line::from(vec![
            workspace_chip(
                "EXPORT",
                if snapshot_reason.is_none() {
                    "AVAILABLE"
                } else {
                    "BLOCKED"
                },
                if snapshot_reason.is_none() {
                    COLOR_SUCCESS
                } else {
                    COLOR_WARNING
                },
            ),
            Span::raw(" "),
            workspace_chip(
                "WRITE",
                if write_reason.is_none() {
                    "AVAILABLE"
                } else {
                    "BLOCKED"
                },
                if write_reason.is_none() {
                    COLOR_SUCCESS
                } else {
                    COLOR_WARNING
                },
            ),
        ]),
        Line::from(Span::styled(
            format!(
                "Export: {}",
                snapshot_reason.unwrap_or("original snapshot bytes; edits are not merged.")
            ),
            Style::default().fg(COLOR_TEXT),
        )),
        Line::from(Span::styled(
            format!(
                "Write: {}",
                write_reason.unwrap_or("unchanged snapshot; radio target selected.")
            ),
            Style::default().fg(COLOR_TEXT),
        )),
    ];
    if shared_reason {
        readiness.truncate(1);
        readiness.push(Line::from(format!(
            "Export / Write: {}",
            snapshot_reason.unwrap()
        )));
    }
    let hero = Paragraph::new(readiness)
        .style(Style::default().bg(COLOR_SURFACE_1))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(hero, chunks[1]);

    let body = if chunks[2].width < 100 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[2])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(38),
                Constraint::Percentage(34),
                Constraint::Percentage(28),
            ])
            .split(chunks[2])
    };

    let size_line = app
        .codeplug_data
        .as_ref()
        .map(|data| {
            format!(
                "{} bytes | {:.1} KB",
                data.len(),
                data.len() as f64 / 1024.0
            )
        })
        .unwrap_or_else(|| "No snapshot bytes".to_string());
    let mut source_lines = vec![
        detail_row(
            "State",
            if full_snapshot {
                "Full snapshot"
            } else {
                "No full snapshot"
            },
            if full_snapshot {
                COLOR_PRIMARY
            } else {
                COLOR_WARNING
            },
        ),
        detail_row("Size", size_line, COLOR_DIM),
        detail_row(
            "Path",
            app.codeplug_path.as_ref().map_or_else(
                || "No file path".to_string(),
                |path| {
                    if compact {
                        path.file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        path.to_string_lossy().into_owned()
                    }
                },
            ),
            COLOR_TEXT,
        ),
    ];
    if compact {
        source_lines.rotate_right(1);
    }
    let source = Paragraph::new(source_lines)
        .style(Style::default().bg(COLOR_SURFACE_1))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Snapshot ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(source, body[0]);

    let channel_count = app.channels.len().to_string();
    let deleted_count = app.deleted_channels.len().to_string();
    let memory = Paragraph::new(vec![
        detail_row(
            "Data",
            if full_snapshot {
                "Snapshot fields"
            } else {
                "Partial sections"
            },
            COLOR_TEXT,
        ),
        detail_row("Channels", channel_count, COLOR_TEXT),
        detail_row("Deleted", deleted_count, COLOR_TEXT),
        detail_row(
            "Settings",
            if settings_loaded {
                "Loaded"
            } else {
                "Not loaded"
            },
            if settings_loaded {
                COLOR_TEXT
            } else {
                COLOR_DIM
            },
        ),
    ])
    .style(Style::default().bg(COLOR_SURFACE_1))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Workspace ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(memory, body[1]);

    let outcome = Paragraph::new(vec![
        Line::from(Span::styled(
            "Export preserves the original full snapshot. Workspace edits are not merged.",
            Style::default().fg(COLOR_TEXT),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Best path ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(
                "Read the full radio or import .nfw. Section reads are not full backups.",
                Style::default().fg(COLOR_DIM),
            ),
        ]),
    ])
    .style(Style::default().bg(COLOR_SURFACE_1))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Outcome ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    if body.len() > 2 {
        f.render_widget(outcome, body[2]);
    }

    let mut hints = vec![
        render_shortcut("r"),
        Span::raw(" read full | "),
        render_shortcut("i"),
        Span::raw(" import .nfw"),
    ];
    if snapshot_reason.is_none() {
        hints.extend([
            Span::raw(" | "),
            render_shortcut("e"),
            Span::raw(" save .nfw"),
        ]);
    }
    if write_reason.is_none() {
        hints.extend([Span::raw(" | "), render_shortcut("w"), Span::raw(" write")]);
    }
    let footer = Paragraph::new(vec![
        Line::from(hints),
        Line::from("Section reads are not full snapshots."),
    ])
    .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(footer, chunks[3]);
}

fn snapshot_block_reason(app: &App) -> Option<&'static str> {
    let Some(data) = &app.codeplug_data else {
        return Some("no full snapshot; read full radio or import .nfw.");
    };
    if data.len() != EEPROM_SIZE {
        return Some("snapshot must contain all 8192 EEPROM bytes.");
    }
    if app.codeplug_workspace_baseline.is_none() {
        return Some("snapshot baseline missing; reload a full snapshot.");
    }
    if !app.codeplug_snapshot_is_current() {
        return Some("workspace changed; write sections, then read full.");
    }
    None
}

pub(crate) fn codeplug_write_block_reason(app: &App) -> Option<&'static str> {
    write_block_reason(app, snapshot_block_reason(app))
}

pub(super) fn write_block_reason(
    app: &App,
    snapshot_reason: Option<&'static str>,
) -> Option<&'static str> {
    if app.simulator_mode {
        return Some("simulator: writes unsupported; local files only.");
    }
    if app.activity_active() {
        return Some("stop Activity; wait for cleanup.");
    }
    snapshot_reason.or_else(|| {
        if app.demo_mode {
            Some("demo mode; hardware writes disabled.")
        } else if app.remote_active || app.remote_tx.is_some() {
            Some("stop Remote and wait for cleanup.")
        } else if app.read_all_request.is_some()
            || matches!(
                app.mode,
                AppMode::Reading | AppMode::Writing | AppMode::BinFlashing
            )
        {
            Some("wait for the current radio operation.")
        } else if app.protocol_port_name.is_none() && app.selected_ble_device_id().is_none() {
            Some("select a radio target first.")
        } else {
            None
        }
    })
}

fn workspace_chip<'a>(label: &'a str, value: &'a str, color: ratatui::style::Color) -> Span<'a> {
    Span::styled(
        format!(" {label} {value} "),
        Style::default()
            .fg(crate::ui::theme::COLOR_SURFACE_0)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn detail_row(
    label: impl Into<String>,
    value: impl Into<String>,
    value_color: ratatui::style::Color,
) -> Line<'static> {
    let label = label.into();
    let value = value.into();
    Line::from(vec![
        Span::styled(
            format!("{label:<9}"),
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value, Style::default().fg(value_color)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppEvent, MainTab};
    use crate::protocol::{Channel, SettingsBlock};
    use ratatui::{Terminal, backend::TestBackend};

    fn current_snapshot() -> App {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Codeplug);
        app.protocol_port_name = Some("test://no-hardware".to_string());
        app.codeplug_data = Some(vec![0; EEPROM_SIZE]);
        app.codeplug_workspace_baseline = app.workspace_snapshot();
        app
    }

    #[test]
    fn help_write_guard_requires_a_current_snapshot_and_available_radio() {
        let mut app = current_snapshot();
        assert!(codeplug_write_block_reason(&app).is_none());
        app.codeplug_data = None;
        assert!(
            codeplug_write_block_reason(&app)
                .unwrap()
                .contains("no full snapshot")
        );
        for block in [
            |app: &mut App| app.channels_dirty = true,
            |app: &mut App| app.codeplug_workspace_baseline = None,
            |app: &mut App| app.remote_active = true,
            |app: &mut App| app.protocol_port_name = None,
            |app: &mut App| app.simulator_mode = true,
            |app: &mut App| app.demo_mode = true,
        ] {
            let mut app = current_snapshot();
            block(&mut app);
            assert!(codeplug_write_block_reason(&app).is_some());
        }
    }

    fn render_text(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let area = crate::ui::views::main_layout::main_content_area(Rect::new(0, 0, width, height));
        terminal
            .draw(|frame| render_codeplug_view(frame, app, area))
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

    #[test]
    fn partial_workspace_never_claims_snapshot_export_or_write_available() {
        let mut app = App::new();
        app.channels = vec![Channel::default()];
        app.settings = Some(SettingsBlock::default());
        for (width, height) in [(60, 20), (80, 24), (100, 30), (160, 45), (190, 80)] {
            let text = render_text(&mut app, width, height);
            assert!(text.contains("EXPORT BLOCKED"), "{text}");
            assert!(text.contains("WRITE BLOCKED"), "{text}");
            assert!(text.contains("no full snapshot"), "{text}");
            assert!(text.contains("read full"), "{text}");
            assert!(text.contains(" Snapshot "), "{text}");
            assert!(text.contains(" Workspace "), "{text}");
            assert!(text.contains("Channels"), "{text}");
            assert!(
                text.contains("Section reads are not full snapshots."),
                "{text}"
            );
            assert!(!text.contains("AVAILABLE"));
            assert!(!text.contains("RADIO READY"));
        }
    }

    #[test]
    fn snapshot_eligibility_matches_app_guard_for_every_dirty_scope() {
        let edits: &[fn(&mut App)] = &[
            |app| app.channels_dirty = true,
            |app| app.settings_dirty = true,
            |app| app.dtmf_dirty = true,
            |app| app.group_labels_dirty = true,
            |app| app.deleted_channels.push(1),
            |app| app.channels.push(Channel::default()),
            |app| app.settings = Some(SettingsBlock::default()),
            |app| app.group_labels[0] = "Edit".to_string(),
            |app| app.codeplug_workspace_baseline = None,
            |app| {
                app.codeplug_data
                    .as_mut()
                    .unwrap()
                    .pop()
                    .map(|_| ())
                    .unwrap()
            },
            |app| app.codeplug_data = None,
        ];
        for edit in edits {
            let mut app = current_snapshot();
            assert!(app.codeplug_snapshot_is_current());
            assert!(snapshot_block_reason(&app).is_none());
            edit(&mut app);
            assert!(!app.codeplug_snapshot_is_current());
            let reason = snapshot_block_reason(&app);
            assert!(reason.is_some());
            assert!(write_block_reason(&app, reason).is_some());
            let text = render_text(&mut app, 80, 24);
            assert!(!text.contains("AVAILABLE"), "{text}");
        }
    }

    #[test]
    fn full_snapshot_load_event_enables_only_eligible_actions() {
        let mut app = App::new();
        app.protocol_port_name = Some("test://no-hardware".to_string());
        app.event_tx
            .send(AppEvent::CodeplugDataLoaded {
                path: "Radio snapshot".into(),
                data: vec![0; EEPROM_SIZE],
                channels: Vec::new(),
                settings: None,
                scan_presets: Vec::new(),
                group_labels: Vec::new(),
            })
            .unwrap();
        app.update();
        assert!(app.codeplug_snapshot_is_current());
        for (width, height) in [(60, 20), (80, 24), (100, 30), (160, 45), (190, 80)] {
            let text = render_text(&mut app, width, height);
            assert!(text.contains("EXPORT AVAILABLE"), "{text}");
            assert!(text.contains("WRITE AVAILABLE"), "{text}");
            assert!(text.contains("read full"), "{text}");
            assert!(text.contains("Radio snapshot"), "{text}");
        }
        app.remote_active = true;
        let text = render_text(&mut app, 80, 24);
        assert!(text.contains("EXPORT AVAILABLE"));
        assert!(text.contains("WRITE BLOCKED"));
        assert!(text.contains("stop Remote"));
        app.remote_active = false;
        app.protocol_port_name = None;
        assert!(write_block_reason(&app, None).is_some());
        app.demo_mode = true;
        assert!(write_block_reason(&app, None).unwrap().contains("demo"));
    }

    #[test]
    fn empty_and_tiny_snapshot_views_render_without_false_capabilities() {
        let mut app = App::new();
        for (width, height) in [(0, 0), (1, 1), (20, 8), (60, 19)] {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|frame| render_codeplug_view(frame, &mut app, Rect::new(0, 0, width, height)))
                .unwrap();
        }
        let text = render_text(&mut app, 80, 24);
        assert!(text.contains("EXPORT BLOCKED"));
        assert!(text.contains("WRITE BLOCKED"));
        assert!(!text.contains("read radio"));
    }
}
