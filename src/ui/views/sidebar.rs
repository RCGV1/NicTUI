use crate::app::App;
use crate::app::MainTab;
use crate::protocol::SETTINGS_METADATA;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG, COLOR_SIDEBAR,
    COLOR_SURFACE_2, COLOR_TEXT, COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Padding, Paragraph},
};

pub fn render_sidebar(f: &mut Frame, app: &App, area: Rect, active_tab: MainTab) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(COLOR_BORDER))
        .bg(COLOR_SIDEBAR);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(inner);
    let content_width = sections[1].width.saturating_sub(4) as usize;
    let session_width = sections[0].width.saturating_sub(4) as usize;

    let has_target = app.selected_port_candidate().is_some();
    let port_label = app.selected_port_short_label();
    let changes = [
        app.channels_dirty || !app.deleted_channels.is_empty(),
        app.settings_dirty,
        app.scan_presets_dirty,
        app.band_plans_dirty,
        app.dtmf_dirty,
        app.group_labels_dirty,
    ]
    .into_iter()
    .filter(|dirty| *dirty)
    .count();
    let transport_summary = app.selected_port_status();
    let dirty_label = if changes == 0 {
        "Clean".to_string()
    } else {
        format!("{changes} dirty")
    };
    let (transport_display, dirty_display) =
        fit_session_status(&transport_summary, &dirty_label, session_width);
    let overview = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " RADIO ",
                Style::default()
                    .fg(COLOR_SIDEBAR)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                if has_target { "target" } else { "none" },
                Style::default()
                    .fg(if has_target { COLOR_PRIMARY } else { COLOR_DIM })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            fit_word_boundary(&port_label, session_width),
            Style::default().fg(COLOR_TEXT),
        )),
        Line::from(vec![
            Span::styled(transport_display, Style::default().fg(COLOR_DIM)),
            Span::raw(if dirty_display.is_empty() || session_width < 8 {
                ""
            } else {
                "  "
            }),
            Span::styled(
                dirty_display,
                Style::default().fg(if changes == 0 {
                    COLOR_DIM
                } else {
                    COLOR_WARNING
                }),
            ),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Session ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SURFACE_2)
            .padding(Padding::horizontal(1)),
    );
    f.render_widget(overview, sections[0]);

    let tabs = vec![
        (
            1,
            MainTab::Channels,
            "Channels",
            sidebar_badge(app, MainTab::Channels),
        ),
        (
            2,
            MainTab::Settings,
            "Settings",
            sidebar_badge(app, MainTab::Settings),
        ),
        (
            3,
            MainTab::Scanning,
            "Scan Presets",
            sidebar_badge(app, MainTab::Scanning),
        ),
        (
            4,
            MainTab::MemoryGroups,
            "Memory Groups",
            sidebar_badge(app, MainTab::MemoryGroups),
        ),
        (
            5,
            MainTab::BandPlan,
            "Band",
            sidebar_badge(app, MainTab::BandPlan),
        ),
        (6, MainTab::DTMF, "DTMF", sidebar_badge(app, MainTab::DTMF)),
        (
            7,
            MainTab::Remote,
            "Remote",
            sidebar_badge(app, MainTab::Remote),
        ),
        (
            8,
            MainTab::Codeplug,
            "Codeplug",
            sidebar_badge(app, MainTab::Codeplug),
        ),
        (
            9,
            MainTab::BinFlash,
            "Flash",
            sidebar_badge(app, MainTab::BinFlash),
        ),
        (
            0,
            MainTab::Debug,
            "Logs",
            sidebar_badge(app, MainTab::Debug),
        ),
        (
            10,
            MainTab::Activity,
            "Activity",
            sidebar_badge(app, MainTab::Activity),
        ),
    ];

    let body_chunks = if sections[1].height >= 18 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(13), Constraint::Length(5)])
            .split(sections[1])
            .to_vec()
    } else {
        vec![sections[1]]
    };

    let items: Vec<ListItem> = tabs
        .into_iter()
        .map(|(index, tab, label, badge)| {
            let active = tab == active_tab;
            let style = if active {
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_TEXT)
            };
            let badge_style = if badge.ends_with('*') {
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD)
            } else if active {
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_DIM)
            };
            let (label_display, badge_display) =
                fit_sidebar_row(index, label, &badge, content_width);
            let row_label = if app.remote_active {
                label_display
            } else if tab == MainTab::Activity {
                format!("A. {label_display}")
            } else {
                format!("{index}. {label_display}")
            };
            let mut spans = vec![
                Span::styled(
                    if active { ">" } else { " " },
                    if active {
                        Style::default()
                            .fg(COLOR_SELECTION_FG)
                            .bg(COLOR_SELECTION_BG)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(COLOR_DIM)
                    },
                ),
                Span::raw(" "),
                Span::styled(row_label, style),
            ];
            if !badge_display.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(badge_display, badge_style));
            }
            let line = Line::from(spans);
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(if app.remote_active {
                " Views (Tab) "
            } else {
                " Navigate "
            })
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SIDEBAR)
            .padding(Padding::horizontal(1)),
    );
    f.render_widget(list, body_chunks[0]);

    if body_chunks.len() > 1 {
        let action_width = body_chunks[1].width.saturating_sub(4) as usize;
        let context = Paragraph::new(sidebar_context_lines(app, active_tab, action_width))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Actions ")
                    .border_style(Style::default().fg(COLOR_BORDER))
                    .bg(COLOR_SURFACE_2)
                    .padding(Padding::horizontal(1)),
            )
            .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_2))
            .wrap(ratatui::widgets::Wrap { trim: true });
        f.render_widget(context, body_chunks[1]);
    }
}

fn fit_session_status(transport: &str, dirty: &str, max_width: usize) -> (String, String) {
    if max_width == 0 {
        return (String::new(), String::new());
    }

    let transport = transport.trim();
    let dirty = dirty.trim();
    let total_width =
        transport.chars().count() + dirty.chars().count() + if dirty.is_empty() { 0 } else { 2 };
    if total_width <= max_width {
        return (transport.to_string(), dirty.to_string());
    }

    let transport = if let Some(rest) = transport.strip_prefix("BLE ready") {
        format!("BLE{rest}")
    } else if let Some(rest) = transport.strip_prefix("BLE target") {
        format!("BLE{rest}")
    } else if let Some(rest) = transport.strip_prefix("BLE reconnect needed") {
        format!("BLE reconnect{rest}")
    } else {
        transport.to_string()
    };
    let dirty = if dirty == "Clean" && transport.chars().count() + 7 > max_width {
        ""
    } else {
        dirty
    };
    let dirty_width = dirty.chars().count();
    if dirty_width >= max_width {
        return (String::new(), fit_word_boundary(dirty, max_width));
    }

    let gap = if dirty.is_empty() { 0 } else { 2 };
    let transport_width = max_width.saturating_sub(dirty_width + gap);
    if transport_width == 0 {
        return (String::new(), dirty.to_string());
    }

    // Never leave a partial RSSI value or detach its unit at the right edge.
    let transport = if transport.chars().count() > transport_width {
        transport
            .strip_suffix(" dBm")
            .and_then(|label| label.rsplit_once(' '))
            .filter(|(_, value)| value.parse::<i32>().is_ok())
            .map_or(transport.as_str(), |(state, _)| state)
    } else {
        transport.as_str()
    };
    (
        fit_word_boundary(transport, transport_width),
        dirty.to_string(),
    )
}

fn fit_sidebar_row(
    index: usize,
    label: &str,
    badge: &str,
    content_width: usize,
) -> (String, String) {
    let row_width = content_width.saturating_sub(2);
    let prefix = format!("{index}. ");
    let available = row_width.saturating_sub(prefix.len());

    if available == 0 {
        return (String::new(), String::new());
    }

    let badge = badge.trim();
    let badge_label = compact_sidebar_label(label, available.saturating_sub(badge.len() + 1));
    let show_badge = !badge.is_empty() && available > badge_label.chars().count() + badge.len();
    let label_width = if show_badge {
        available.saturating_sub(badge.len() + 1)
    } else {
        available
    };
    let label = if label.chars().count() > label_width {
        let compact = compact_sidebar_label(label, label_width);
        if compact.chars().count() <= label_width {
            compact
        } else {
            label
        }
    } else {
        label
    };

    (
        fit_word_boundary(label, label_width),
        if show_badge {
            fit_word_boundary(badge, badge.len())
        } else {
            String::new()
        },
    )
}

fn compact_sidebar_label(label: &str, max_width: usize) -> &str {
    match (label, max_width) {
        ("Scan Presets", _) => "Scan",
        ("Memory Groups", 0..=5) => "Grp",
        ("Memory Groups", _) => "Groups",
        _ => label,
    }
}

fn fit_word_boundary(value: &str, max_width: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_width {
        return value.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width <= 2 {
        return value.chars().take(max_width).collect();
    }

    let marker = "..";
    let room = max_width.saturating_sub(marker.len());
    let mut fitted = String::new();

    for word in value.split_whitespace() {
        let next_width =
            fitted.chars().count() + usize::from(!fitted.is_empty()) + word.chars().count();
        if next_width > room {
            break;
        }
        if !fitted.is_empty() {
            fitted.push(' ');
        }
        fitted.push_str(word);
    }

    if fitted.is_empty() {
        fitted = value.chars().take(room).collect();
    }
    format!("{}{}", fitted.trim_end(), marker)
}

fn sidebar_badge(app: &App, tab: MainTab) -> String {
    match tab {
        MainTab::Channels => {
            let mut badge = if app.channels.is_empty() {
                String::new()
            } else {
                app.channels.len().to_string()
            };
            if app.channels_dirty || !app.deleted_channels.is_empty() {
                badge.push('*');
            }
            badge
        }
        MainTab::Settings => {
            let mut badge = if app.settings.is_some() {
                SETTINGS_METADATA.len().to_string()
            } else {
                String::new()
            };
            if app.settings_dirty {
                badge.push('*');
            }
            badge
        }
        MainTab::Scanning => {
            let mut badge = if app.scan_presets.is_empty() {
                String::new()
            } else {
                app.scan_presets.len().to_string()
            };
            if app.scan_presets_dirty {
                badge.push('*');
            }
            badge
        }
        MainTab::MemoryGroups => {
            let named_groups = app
                .group_labels
                .iter()
                .filter(|label| !label.trim().is_empty())
                .count();
            let mut badge = if named_groups == 0 {
                String::new()
            } else {
                named_groups.to_string()
            };
            if app.group_labels_dirty {
                badge.push('*');
            }
            badge
        }
        MainTab::BandPlan => {
            let mut badge = if app.band_plans.is_empty() {
                String::new()
            } else {
                app.band_plans.len().to_string()
            };
            if app.band_plans_dirty {
                badge.push('*');
            }
            badge
        }
        MainTab::DTMF => {
            let mut badge = if app.dtmf_presets.is_empty() {
                String::new()
            } else {
                app.dtmf_presets.len().to_string()
            };
            if app.dtmf_dirty {
                badge.push('*');
            }
            badge
        }
        MainTab::Remote => {
            if app.remote_active {
                "open".to_string()
            } else {
                "idle".to_string()
            }
        }
        MainTab::Codeplug => {
            if app.codeplug_data.is_some() {
                "nfw".to_string()
            } else {
                String::new()
            }
        }
        MainTab::BinFlash => {
            if app.bin_firmware_data.is_some() {
                "bin".to_string()
            } else {
                String::new()
            }
        }
        MainTab::Debug => String::new(),
        MainTab::Activity => if app.activity_active() { "live" } else { "" }.into(),
    }
}

fn sidebar_context_lines(app: &App, active_tab: MainTab, width: usize) -> Vec<Line<'static>> {
    match active_tab {
        MainTab::Activity => vec![
            Line::from("Activity capture"),
            Line::from(if app.activity_active() {
                "s stop / e CSV"
            } else {
                "s start / e CSV"
            }),
            Line::from("Up/Down selects row"),
        ],
        MainTab::Channels => vec![
            Line::from(Span::styled(
                "Channels",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                if app.channels.is_empty() {
                    fit_word_boundary("Read radio or import", width)
                } else {
                    fit_word_boundary("Enter edits row", width)
                },
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                fit_word_boundary("R read  N new  D delete", width),
                Style::default().fg(COLOR_DIM),
            )),
        ],
        MainTab::Settings => vec![
            Line::from(Span::styled(
                "Settings",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                if app.settings.is_some() {
                    fit_word_boundary("Enter edits field", width)
                } else {
                    fit_word_boundary("Read settings first", width)
                },
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                fit_word_boundary("R read  W write", width),
                Style::default().fg(COLOR_DIM),
            )),
        ],
        MainTab::Scanning => vec![
            Line::from(Span::styled(
                "Scan Presets",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                if app.scan_presets.is_empty() {
                    fit_word_boundary("No presets loaded", width)
                } else {
                    fit_word_boundary("Enter edits", width)
                },
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                fit_word_boundary(
                    if app.scan_presets_dirty {
                        "w write changes"
                    } else {
                        "r read"
                    },
                    width,
                ),
                Style::default().fg(if app.scan_presets_dirty {
                    COLOR_WARNING
                } else {
                    COLOR_DIM
                }),
            )),
        ],
        MainTab::BandPlan => vec![
            Line::from(Span::styled(
                "Band Plan",
                Style::default().fg(COLOR_PRIMARY).bold(),
            )),
            Line::from(Span::styled(
                fit_word_boundary(
                    if app.band_plans.is_empty() {
                        "Read band plan first"
                    } else {
                        "Enter edits"
                    },
                    width,
                ),
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                fit_word_boundary(
                    if app.band_plans_dirty {
                        "w write changes"
                    } else {
                        "r read"
                    },
                    width,
                ),
                Style::default().fg(if app.band_plans_dirty {
                    COLOR_WARNING
                } else {
                    COLOR_DIM
                }),
            )),
        ],
        _ => vec![
            Line::from(Span::styled(
                "Workspace",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                fit_word_boundary("Tab switches views", width),
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                fit_word_boundary(
                    if app.remote_active {
                        "D details  p stop"
                    } else {
                        "0-9 navigate"
                    },
                    width,
                ),
                Style::default().fg(COLOR_DIM),
            )),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::{fit_session_status, fit_sidebar_row, fit_word_boundary, sidebar_badge};
    use crate::app::{App, MainTab};

    #[test]
    fn sidebar_row_keeps_channel_count_visible() {
        let (label, badge) = fit_sidebar_row(1, "Channels", "197", 20);
        assert_eq!(label, "Channels");
        assert_eq!(badge, "197");
    }

    #[test]
    fn sidebar_row_keeps_remote_idle_visible() {
        let (label, badge) = fit_sidebar_row(6, "Remote", "idle", 20);
        assert_eq!(label, "Remote");
        assert_eq!(badge, "idle");
    }

    #[test]
    fn sidebar_row_compacts_labels_without_half_words() {
        let (label, badge) = fit_sidebar_row(4, "Memory Groups", "", 10);
        assert_eq!(label, "Grp");
        assert_eq!(badge, "");
    }

    #[test]
    fn word_boundary_fit_prefers_whole_words() {
        assert_eq!(fit_word_boundary("BLE reconnect needed", 14), "BLE..");
        assert_eq!(fit_word_boundary("Clean", 4), "Cl..");
    }

    #[test]
    fn session_status_preserves_reconnect_warning_before_clean_badge() {
        let (transport, dirty) = fit_session_status("BLE reconnect needed -69 dBm", "Clean", 18);
        assert_eq!(transport, "BLE reconnect");
        assert_eq!(dirty, "");
    }

    #[test]
    fn session_status_compacts_ble_before_truncating_rssi() {
        for state in ["ready", "target"] {
            for rssi in [-58, -128] {
                for width in [26, 23, 17, 15] {
                    let (transport, dirty) =
                        fit_session_status(&format!("BLE {state} {rssi} dBm"), "Clean", width);
                    assert!(transport.contains(&format!("{rssi} dBm")), "{transport}");
                    let gap = usize::from(!dirty.is_empty()) * 2;
                    assert!(transport.len() + gap + dirty.len() <= width);
                }
            }
        }
        let (transport, dirty) = fit_session_status("BLE ready -58 dBm", "1 dirty", 15);
        assert_eq!(transport, "BLE");
        assert_eq!(dirty, "1 dirty");
    }

    #[test]
    fn sidebar_session_retains_rssi_unit_at_requested_terminal_sizes() {
        use ratatui::{Terminal, backend::TestBackend, layout::Rect};
        let app = App::demo();
        for (width, height, sidebar) in [(180, 50, 28), (100, 30, 22), (80, 24, 20)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    super::render_sidebar(
                        frame,
                        &app,
                        Rect::new(0, 3, sidebar, height - 5),
                        MainTab::Remote,
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let line = (0..sidebar)
                .map(|x| buffer[(x, 6)].symbol())
                .collect::<String>();
            assert!(line.contains("BLE"));
            assert!(line.contains("-58 dBm"), "{width}x{height}: {line}");
            assert!(!line.contains(".."), "{width}x{height}: {line}");
        }
    }

    #[test]
    fn empty_sidebar_badges_do_not_render_placeholders() {
        let app = App::new();
        assert_eq!(sidebar_badge(&app, MainTab::Channels), "");
        assert_eq!(sidebar_badge(&app, MainTab::Settings), "");
        assert_eq!(sidebar_badge(&app, MainTab::MemoryGroups), "");
        assert_eq!(sidebar_badge(&app, MainTab::Codeplug), "");
    }

    #[test]
    fn scan_and_band_badges_keep_unsaved_markers() {
        use crate::protocol::{BandPlan, ScanPreset};
        let mut app = App::new();
        app.scan_presets.push(ScanPreset::default());
        app.band_plans.push(BandPlan::default());
        app.scan_presets_dirty = true;
        app.band_plans_dirty = true;
        assert_eq!(sidebar_badge(&app, MainTab::Scanning), "1*");
        assert_eq!(sidebar_badge(&app, MainTab::BandPlan), "1*");
        app.scan_presets_dirty = false;
        app.band_plans_dirty = false;
        assert_eq!(sidebar_badge(&app, MainTab::Scanning), "1");
        assert_eq!(sidebar_badge(&app, MainTab::BandPlan), "1");
    }

    #[test]
    fn short_sidebar_keeps_all_ten_views_visible() {
        use ratatui::{Terminal, backend::TestBackend, layout::Rect};
        let mut app = App::new();
        for remote_active in [false, true] {
            app.remote_active = remote_active;
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|frame| {
                    super::render_sidebar(frame, &app, Rect::new(0, 3, 20, 19), MainTab::Remote)
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text = (3..22)
                .flat_map(|y| (0..20).map(move |x| buffer[(x, y)].symbol()))
                .collect::<String>();
            for label in [
                "Channels", "Settings", "Scan", "Groups", "Band", "DTMF", "Remote", "Codeplug",
                "Flash", "Logs",
            ] {
                assert!(text.contains(label), "missing {label}: {text}");
            }
            if remote_active {
                assert!(text.contains("Views (Tab)"));
                assert!(!text.contains("1. Channels"));
            }
        }
    }
}
