use crate::app::{App, AppMode, MainTab};
use crate::ui::render_shortcut;
use crate::ui::theme::VERSION;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SURFACE_0, COLOR_SURFACE_2, COLOR_TEXT,
    COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let left_width = area.width.clamp(18, 28);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left_width), Constraint::Min(0)])
        .split(area);
    let compact = chunks[0].width < 22;

    let tab_label = match app.mode {
        AppMode::Main(MainTab::Channels) => "Channels",
        AppMode::Main(MainTab::Settings) => "Settings",
        AppMode::Main(MainTab::Scanning) => {
            if compact {
                "Scan"
            } else {
                "Scan Presets"
            }
        }
        AppMode::Main(MainTab::MemoryGroups) => {
            if compact {
                "Groups"
            } else {
                "Memory Groups"
            }
        }
        AppMode::Main(MainTab::BandPlan) => {
            if compact {
                "Band"
            } else {
                "Band Plan"
            }
        }
        AppMode::Main(MainTab::DTMF) => "DTMF",
        AppMode::Main(MainTab::Remote) => "Remote",
        AppMode::Main(MainTab::Codeplug) => "Codeplug",
        AppMode::Main(MainTab::BinFlash) => {
            if compact {
                "Flash"
            } else {
                "BIN Flash"
            }
        }
        AppMode::Main(MainTab::Debug) => "Debug",
        AppMode::PortSelection => "Connect",
        AppMode::Reading => "Reading",
        AppMode::Writing => "Writing",
        AppMode::BinFlashing => "Flashing",
        AppMode::EditChannel(_) => "Edit Channel",
        AppMode::EditSetting(_) => "Edit Setting",
        AppMode::EditDTMF(_) => "Edit DTMF",
        AppMode::EditScanPreset(_) => "Edit Scan",
        AppMode::EditGroupLabel(_) => "Edit Group",
        AppMode::EditBandPlan(_) => {
            if compact {
                "Edit Band"
            } else {
                "Edit Band Plan"
            }
        }
        AppMode::DeleteChannelConfirm(_) => "Confirm Delete",
        AppMode::Error(_) => "Error",
    };

    let status = Paragraph::new(Line::from(vec![
        Span::styled(format!(" {tab_label} "), Style::default().fg(COLOR_PRIMARY)),
        Span::styled("•", Style::default().fg(COLOR_DIM)),
        Span::styled(format!(" {VERSION} "), Style::default().fg(COLOR_DIM)),
    ]))
    .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_0))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(status, chunks[0]);

    let hints = match app.mode {
        AppMode::Main(MainTab::Channels) => {
            let has_changes = app.channels_dirty || !app.deleted_channels.is_empty();
            let channels_empty = app.channels.is_empty() && app.deleted_channels.is_empty();
            let mut hints = if channels_empty {
                vec![
                    render_shortcut("r"),
                    Span::raw(" read radio | "),
                    render_shortcut("i"),
                    Span::raw(" import file | "),
                ]
            } else {
                vec![
                    render_shortcut("i"),
                    Span::raw(" import | "),
                    render_shortcut("r"),
                    Span::raw(" read | "),
                ]
            };
            if has_changes {
                hints.push(render_shortcut("w"));
                hints.push(Span::styled(
                    " SAVE ",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                hints.push(render_shortcut("w"));
                hints.push(Span::raw(" write "));
            }
            hints.push(Span::raw(" | "));
            hints.push(render_shortcut("n"));
            hints.push(Span::raw(if channels_empty {
                " new channel | "
            } else {
                " new | "
            }));
            hints.push(render_shortcut("d"));
            hints.push(Span::raw(" del "));
            if !app.deleted_channels.is_empty() {
                hints.push(Span::raw(" | "));
                hints.push(render_shortcut("u"));
                hints.push(Span::raw(" undel "));
            }
            Line::from(hints)
        }
        AppMode::Main(MainTab::Settings) => {
            let mut hints = vec![
                render_shortcut("r"),
                Span::raw(" read | "),
                render_shortcut("Enter"),
                Span::raw(" edit "),
            ];
            if app.settings_dirty {
                hints.push(Span::raw("| "));
                hints.push(render_shortcut("w"));
                hints.push(Span::styled(
                    " Save & Reboot ",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                hints.push(Span::raw("| "));
                hints.push(render_shortcut("w"));
                hints.push(Span::raw(" write "));
            }
            Line::from(hints)
        }
        AppMode::Main(MainTab::Scanning) => {
            let hints = vec![
                render_shortcut("r"),
                Span::raw(" read | "),
                render_shortcut("Enter"),
                Span::raw(" edit "),
            ];
            Line::from(hints)
        }
        AppMode::Main(MainTab::MemoryGroups) => {
            let mut hints = vec![
                render_shortcut("r"),
                Span::raw(" refresh | "),
                render_shortcut("Enter"),
                Span::raw(" rename "),
            ];
            if app.group_labels_dirty {
                hints.push(Span::raw("| "));
                hints.push(render_shortcut("w"));
                hints.push(Span::styled(
                    " SAVE NAMES ",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            Line::from(hints)
        }
        AppMode::Main(MainTab::BandPlan) => {
            Line::from(vec![render_shortcut("r"), Span::raw(" read ")])
        }
        AppMode::Main(MainTab::DTMF) => {
            let mut hints = vec![
                render_shortcut("r"),
                Span::raw(" read | "),
                render_shortcut("Enter"),
                Span::raw(" edit | "),
            ];
            if app.dtmf_dirty {
                hints.push(render_shortcut("w"));
                hints.push(Span::styled(
                    " SAVE ",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                hints.push(render_shortcut("w"));
                hints.push(Span::raw(" write "));
            }
            Line::from(hints)
        }
        AppMode::Main(MainTab::Remote) if app.remote_active => Line::from(vec![
            render_shortcut("o"),
            Span::raw(" start | "),
            render_shortcut("p"),
            Span::raw(" stop | "),
            render_shortcut("Up/Down"),
            Span::raw(" move | "),
            render_shortcut("Enter/Esc"),
            Span::raw(" menu / exit | "),
            render_shortcut("Tab"),
            Span::raw(" leave | "),
            render_shortcut("a/b"),
            Span::raw(" ptt | "),
            render_shortcut("f/v"),
            Span::raw(" light/vm "),
        ]),
        AppMode::Main(MainTab::Remote) => Line::from(vec![
            render_shortcut("o"),
            Span::raw(" start | "),
            render_shortcut("Esc"),
            Span::raw(" back | "),
            render_shortcut("Tab"),
            Span::raw(" next | "),
            render_shortcut("1-9"),
            Span::raw(" switch tabs "),
        ]),
        AppMode::Main(MainTab::Codeplug) => Line::from(vec![
            render_shortcut("i"),
            Span::raw(" import | "),
            render_shortcut("e"),
            Span::raw(" export | "),
            render_shortcut("w"),
            Span::raw(" write "),
        ]),
        AppMode::Main(MainTab::BinFlash) => Line::from(vec![
            render_shortcut("i"),
            Span::raw(" import | "),
            render_shortcut("f"),
            Span::raw(" flash "),
        ]),
        AppMode::Main(MainTab::Debug) => Line::from(vec![]),
        _ => Line::from(vec![]),
    };

    let hints_p = Paragraph::new(hints)
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_2))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(hints_p, chunks[1]);
}
