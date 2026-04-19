use crate::app::App;
use crate::app::MainTab;
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

    let connected = app.protocol_port_name.is_some();
    let port_label = app
        .protocol_port_name
        .as_deref()
        .or_else(|| {
            app.selected_port_candidate()
                .map(|candidate| candidate.port_name.as_str())
        })
        .map(|port| port.rsplit('/').next().unwrap_or(port))
        .unwrap_or("not selected");
    let changes = [
        app.channels_dirty || !app.deleted_channels.is_empty(),
        app.settings_dirty,
        app.dtmf_dirty,
        app.group_labels_dirty,
    ]
    .into_iter()
    .filter(|dirty| *dirty)
    .count();
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
                if connected { "live" } else { "idle" },
                Style::default()
                    .fg(if connected { COLOR_SUCCESS } else { COLOR_DIM })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(port_label, Style::default().fg(COLOR_TEXT))),
        Line::from(Span::styled(
            if changes == 0 {
                "Clean".to_string()
            } else {
                format!("{changes} dirty")
            },
            Style::default().fg(if changes == 0 {
                COLOR_DIM
            } else {
                COLOR_WARNING
            }),
        )),
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
            "Debug",
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
        let context = Paragraph::new(sidebar_context_lines(app, active_tab))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Focus ")
                    .border_style(Style::default().fg(COLOR_BORDER))
                    .bg(COLOR_SURFACE_2)
                    .padding(Padding::horizontal(1)),
            )
            .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_2))
            .wrap(ratatui::widgets::Wrap { trim: true });
        f.render_widget(context, body_chunks[1]);
    }
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
        let compact = compact_sidebar_label(label);
        if compact.chars().count() <= label_width {
            compact
        } else {
            label
        }
    } else {
        label
    };

    (
        truncate_text(label, label_width),
        if show_badge {
            truncate_text(badge, badge.len())
        } else {
            String::new()
        },
    )
}

fn compact_sidebar_label(label: &str) -> &str {
    match label {
        "Scan Presets" => "Scan",
        "Memory Groups" => "Groups",
        _ => label,
    }
}

fn truncate_text(value: &str, max_width: usize) -> String {
    value.chars().take(max_width).collect()
}

fn sidebar_badge(app: &App, tab: MainTab) -> String {
    match tab {
        MainTab::Channels => {
            let mut badge = if app.channels.is_empty() {
                "--".to_string()
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
                String::new()
            } else {
                "--".to_string()
            };
            if app.settings_dirty {
                badge = "*".to_string();
            }
            badge
        }
        MainTab::Scanning => {
            if app.scan_presets.is_empty() {
                "--".to_string()
            } else {
                app.scan_presets.len().to_string()
            }
        }
        MainTab::MemoryGroups => {
            let mut badge = app
                .group_labels
                .iter()
                .filter(|label| !label.trim().is_empty())
                .count()
                .to_string();
            if app.group_labels_dirty {
                badge.push('*');
            }
            badge
        }
        MainTab::BandPlan => {
            if app.band_plans.is_empty() {
                "--".to_string()
            } else {
                app.band_plans.len().to_string()
            }
        }
        MainTab::DTMF => {
            let mut badge = if app.dtmf_presets.is_empty() {
                "--".to_string()
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
                "--".to_string()
            }
        }
        MainTab::BinFlash => {
            if app.bin_firmware_data.is_some() {
                "bin".to_string()
            } else {
                "--".to_string()
            }
        }
        MainTab::Debug => String::new(),
    }
}

fn sidebar_context_lines(app: &App, active_tab: MainTab) -> Vec<Line<'static>> {
    match active_tab {
        MainTab::Channels => app
            .channel_state
            .selected()
            .and_then(|index| app.channels.get(index))
            .map(|channel| {
                vec![
                    Line::from(Span::styled(
                        if channel.name.is_empty() {
                            format!("CH {:03}", channel.channel_num)
                        } else {
                            format!("CH {:03} {}", channel.channel_num, channel.name)
                        },
                        Style::default()
                            .fg(COLOR_PRIMARY)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        format!(
                            "{} {} / {}",
                            channel.modulation, channel.rx_freq, channel.tx_freq
                        ),
                        Style::default().fg(COLOR_TEXT),
                    )),
                    Line::from(Span::styled(
                        "Enter edit | R read",
                        Style::default().fg(COLOR_DIM),
                    )),
                ]
            })
            .unwrap_or_else(|| {
                vec![
                    Line::from(Span::styled(
                        "No selection",
                        Style::default().fg(COLOR_DIM).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        "Read or import channels",
                        Style::default().fg(COLOR_TEXT),
                    )),
                    Line::from(Span::styled(
                        "R reads radio",
                        Style::default().fg(COLOR_DIM),
                    )),
                ]
            }),
        MainTab::Settings => vec![
            Line::from(Span::styled(
                "Settings",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                if app.settings.is_some() {
                    "Enter edits"
                } else {
                    "Read settings first"
                },
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                "R reads  •  W writes",
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
                    "No presets loaded"
                } else {
                    "Enter edits"
                },
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled("R refreshes", Style::default().fg(COLOR_DIM))),
        ],
        _ => vec![
            Line::from(Span::styled(
                "Workspace",
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Tab keys switch views",
                Style::default().fg(COLOR_TEXT),
            )),
            Line::from(Span::styled(
                "Tab / 1-9 navigate",
                Style::default().fg(COLOR_DIM),
            )),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::fit_sidebar_row;

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
}
