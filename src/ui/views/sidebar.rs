use crate::app::App;
use crate::app::MainTab;
use crate::protocol::SETTINGS_METADATA;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG, COLOR_SIDEBAR,
    COLOR_SUCCESS, COLOR_SURFACE_2, COLOR_TEXT, COLOR_WARNING,
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
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(inner);
    let content_width = sections[1].width.saturating_sub(2) as usize;
    let session_width = sections[0].width.saturating_sub(4) as usize;

    let has_target = app.selected_port_candidate().is_some();
    let port_label = app.selected_port_short_label();
    let changes = [
        app.channels_dirty || !app.deleted_channels.is_empty(),
        app.settings_dirty,
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
                if has_target { "ready" } else { "idle" },
                Style::default()
                    .fg(if has_target { COLOR_SUCCESS } else { COLOR_DIM })
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
    ];

    let body_chunks = if sections[1].height >= 14 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(5)])
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
                Span::styled(format!("{index}. {label_display}"), style),
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
            .title(" Navigate ")
            .border_style(Style::default().fg(COLOR_BORDER))
            .bg(COLOR_SIDEBAR)
            .padding(Padding::symmetric(1, 1)),
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

    let dirty = dirty.trim();
    let dirty_width = dirty.chars().count();
    if dirty_width >= max_width {
        return (String::new(), fit_word_boundary(dirty, max_width));
    }

    let gap = if dirty.is_empty() { 0 } else { 2 };
    let transport_width = max_width.saturating_sub(dirty_width + gap);
    if transport_width == 0 {
        return (String::new(), dirty.to_string());
    }

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
    let show_badge = !badge.is_empty() && available > badge.len() + 4;
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
            if app.scan_presets.is_empty() {
                String::new()
            } else {
                app.scan_presets.len().to_string()
            }
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
            if app.band_plans.is_empty() {
                String::new()
            } else {
                app.band_plans.len().to_string()
            }
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
                "live".to_string()
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
    }
}

fn sidebar_context_lines(app: &App, active_tab: MainTab, width: usize) -> Vec<Line<'static>> {
    match active_tab {
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
                fit_word_boundary("R refresh", width),
                Style::default().fg(COLOR_DIM),
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
                fit_word_boundary("1-9 navigate", width),
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
    fn session_status_preserves_clean_label() {
        let (transport, dirty) = fit_session_status("BLE reconnect needed -69 dBm", "Clean", 18);
        assert_eq!(transport, "BLE..");
        assert_eq!(dirty, "Clean");
    }

    #[test]
    fn empty_sidebar_badges_do_not_render_placeholders() {
        let app = App::new();
        assert_eq!(sidebar_badge(&app, MainTab::Channels), "");
        assert_eq!(sidebar_badge(&app, MainTab::Settings), "");
        assert_eq!(sidebar_badge(&app, MainTab::MemoryGroups), "");
        assert_eq!(sidebar_badge(&app, MainTab::Codeplug), "");
    }
}
