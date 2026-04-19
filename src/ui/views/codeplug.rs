use crate::app::App;
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SUCCESS, COLOR_SURFACE_1, COLOR_TEXT,
    COLOR_WARNING,
};
use crate::ui::views::ready_state::{ReadyStateAction, ReadyStateContent, render_ready_state};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub fn render_codeplug_view(f: &mut Frame, app: &mut App, area: Rect) {
    if app.codeplug_data.is_none() && app.channels.is_empty() {
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "Codeplug",
                card_title: "Ready To Load",
                heading: "No Codeplug Loaded",
                description: "Read the radio or import a .nfw snapshot to populate the workspace."
                    .to_string(),
                note: None,
            },
            &[
                ReadyStateAction {
                    key: "r",
                    label: "read radio",
                },
                ReadyStateAction {
                    key: "i",
                    label: "import file",
                },
            ],
        );
        return;
    }

    let summary_height = if area.width < 90 { 3 } else { 2 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Length(if area.width < 90 { 5 } else { 4 }),
            Constraint::Min(10),
            Constraint::Length(if area.width < 90 { 5 } else { 4 }),
        ])
        .split(area);

    let file_loaded = app.codeplug_data.is_some();
    let settings_loaded = app.settings.is_some();
    let live_memory = !app.channels.is_empty() || settings_loaded;
    let summary = Paragraph::new(format!(
        "{} channels | {} deleted | settings {} | file {}",
        app.channels.len(),
        app.deleted_channels.len(),
        if settings_loaded { "loaded" } else { "empty" },
        if file_loaded { "loaded" } else { "memory only" }
    ))
    .style(Style::default().fg(COLOR_DIM))
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(summary, chunks[0]);

    let hero = Paragraph::new(vec![
        Line::from(Span::styled(
            "Codeplug Workspace",
            Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Keep the imported file, live radio memory, and export/write actions in one place.",
            Style::default().fg(COLOR_DIM),
        )),
        Line::from(vec![
            workspace_chip(
                "FILE",
                if file_loaded { "LOADED" } else { "MEMORY ONLY" },
                if file_loaded {
                    COLOR_SUCCESS
                } else {
                    COLOR_WARNING
                },
            ),
            Span::raw(" "),
            workspace_chip(
                "RADIO",
                if live_memory { "READY" } else { "EMPTY" },
                if live_memory {
                    COLOR_SUCCESS
                } else {
                    COLOR_WARNING
                },
            ),
            Span::raw(" "),
            workspace_chip(
                "WRITE",
                if file_loaded || live_memory {
                    "AVAILABLE"
                } else {
                    "BLOCKED"
                },
                if file_loaded || live_memory {
                    COLOR_SUCCESS
                } else {
                    COLOR_WARNING
                },
            ),
        ]),
    ])
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
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Min(6),
            ])
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

    let source_lines = if let Some(path) = &app.codeplug_path {
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
            .unwrap_or_else(|| "Size unavailable".to_string());
        let path_line = path.to_string_lossy().into_owned();
        vec![
            detail_row("State", "Imported snapshot", COLOR_SUCCESS),
            detail_row("Path", path_line, COLOR_TEXT),
            detail_row("Size", size_line, COLOR_DIM),
        ]
    } else {
        vec![
            detail_row("State", "No file imported", COLOR_WARNING),
            detail_row(
                "Source",
                "Workspace currently reflects live memory only",
                COLOR_DIM,
            ),
        ]
    };
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
            "Radio",
            if live_memory {
                "Workspace populated"
            } else {
                "Nothing loaded"
            },
            if live_memory {
                COLOR_SUCCESS
            } else {
                COLOR_WARNING
            },
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
            if file_loaded {
                "Export to save the current snapshot, or write it back to the radio."
            } else {
                "Read from the radio or import a file before exporting a new snapshot."
            },
            Style::default().fg(COLOR_TEXT),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Best path ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(
                "read or import, confirm the workspace, then export or write once.",
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
    f.render_widget(outcome, body[2]);

    let footer = Paragraph::new(Line::from(vec![
        render_shortcut("i"),
        Span::raw(" import | "),
        render_shortcut("e"),
        Span::raw(" export | "),
        render_shortcut("w"),
        Span::raw(" write"),
    ]))
    .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_1),
    );
    f.render_widget(footer, chunks[3]);
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
