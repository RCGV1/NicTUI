use crate::app::App;
use crate::channel_file::is_populated_channel;
use crate::protocol::{group_label, group_letter};
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_SELECTION_BG, COLOR_SELECTION_FG, COLOR_SURFACE_1,
    COLOR_SURFACE_2, COLOR_TEXT, COLOR_WARNING,
};
use crate::ui::views::ready_state::{ReadyStateContent, render_ready_state};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_channels_table(f: &mut Frame, app: &mut App, area: Rect) {
    if app.channels.is_empty() && app.deleted_channels.is_empty() {
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "Channels",
                card_title: "Ready To Load",
                heading: "No Channels Loaded",
                description: "r: read channels | i: import CSV/CHIRP | Shift+R: read all sections"
                    .into(),
                note: None,
            },
            &[],
        );
        return;
    }

    let compact = area.width < 100;
    let summary_height = if compact { 3 } else { 2 };
    let detail_height = 6;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Min(1),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let remaining = app
        .channels
        .iter()
        .filter(|ch| !app.deleted_channels.contains(&ch.channel_num))
        .collect::<Vec<_>>();
    let active_loaded = remaining
        .iter()
        .copied()
        .filter(|ch| is_populated_channel(ch))
        .collect::<Vec<_>>();
    let active_count = active_loaded.len();
    let empty_count = remaining.len().saturating_sub(active_count);
    let wide_count = active_loaded
        .iter()
        .filter(|ch| ch.bandwidth == "Wide")
        .count();
    let narrow_count = active_loaded
        .iter()
        .filter(|ch| ch.bandwidth == "Narrow")
        .count();
    let unknown_count = active_count.saturating_sub(wide_count + narrow_count);
    let fm_count = active_loaded
        .iter()
        .filter(|ch| ch.modulation == "FM")
        .count();

    let summary = Paragraph::new(format!(
        "{} loaded | {} active | {} empty | {} wide | {} narrow | {} FM{}",
        app.channels.len(),
        active_count,
        empty_count,
        wide_count,
        narrow_count,
        fm_count,
        if unknown_count > 0 {
            format!(" | {unknown_count} unknown BW")
        } else {
            String::new()
        }
    ))
    .style(Style::default().fg(COLOR_DIM))
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(COLOR_BORDER)),
    );
    f.render_widget(summary, chunks[0]);

    let header = if compact {
        Row::new(vec![
            Cell::from("CH").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Name").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("RX").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("TX").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Tone").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Mode").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Grp").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Row::new(vec![
            Cell::from("CH").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Name").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("RX Freq").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("TX Freq").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("RX Tone").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("TX Tone").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Pwr raw").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("BW").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Mod").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Groups").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    }
    .style(Style::default().bg(COLOR_SURFACE_2))
    .height(1);

    let rows = app.channels.iter().enumerate().map(|(i, ch)| {
        let is_deleted = app.deleted_channels.contains(&ch.channel_num);
        let is_inactive = !is_populated_channel(ch);
        let is_selected = Some(i) == app.channel_state.selected();

        let style = if is_deleted {
            Style::default()
                .fg(COLOR_DIM)
                .add_modifier(Modifier::CROSSED_OUT)
        } else if is_inactive {
            Style::default()
                .fg(COLOR_DIM)
                .add_modifier(Modifier::ITALIC)
        } else if is_selected {
            Style::default()
                .fg(COLOR_SELECTION_FG)
                .bg(COLOR_SELECTION_BG)
                .add_modifier(Modifier::BOLD)
        } else if i % 2 == 0 {
            Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1)
        } else {
            Style::default().fg(COLOR_TEXT)
        };

        let groups_str = format_groups(&ch.groups, &app.group_labels);
        let power = display_power(ch.power);

        let ch_num = if is_deleted {
            format!("{} (DEL)", ch.channel_num)
        } else if is_inactive {
            format!("{} (OFF)", ch.channel_num)
        } else {
            ch.channel_num.to_string()
        };

        let row = if compact {
            Row::new(vec![
                Cell::from(ch_num),
                Cell::from(truncate_line(&ch.name, 8)),
                Cell::from(ch.rx_freq.clone()),
                Cell::from(ch.tx_freq.clone()),
                Cell::from(truncate_line(&compact_tone(&ch.rx_tone, &ch.tx_tone), 7)),
                Cell::from(compact_mode(&ch.bandwidth, &ch.modulation)),
                Cell::from(truncate_line(&groups_str, 12)),
            ])
        } else {
            Row::new(vec![
                Cell::from(ch_num),
                Cell::from(truncate_line(&ch.name, 12)),
                Cell::from(ch.rx_freq.clone()),
                Cell::from(ch.tx_freq.clone()),
                Cell::from(truncate_line(&ch.rx_tone, 8)),
                Cell::from(truncate_line(&ch.tx_tone, 8)),
                Cell::from(power),
                Cell::from(ch.bandwidth.clone()),
                Cell::from(ch.modulation.clone()),
                Cell::from(truncate_line(&groups_str, 18)),
            ])
        };

        row.style(style)
    });

    let has_changes = app.channels_dirty || !app.deleted_channels.is_empty();
    let deleted_count = app.deleted_channels.len();
    let title = if has_changes {
        if deleted_count > 0 {
            format!(" Channels ({} local deletes; w writes) ", deleted_count)
        } else {
            " Channels (Unsaved local; w writes) ".to_string()
        }
    } else {
        " Channels ".to_string()
    };

    let table = Table::new(
        rows,
        if compact {
            vec![
                Constraint::Length(4),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Min(4),
            ]
        } else {
            vec![
                Constraint::Length(7),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(7),
                Constraint::Length(4),
                Constraint::Length(4),
                Constraint::Min(10),
            ]
        },
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .bg(COLOR_SURFACE_1)
            .border_style(if has_changes {
                Style::default().fg(COLOR_WARNING)
            } else {
                Style::default().fg(COLOR_BORDER)
            }),
    )
    .row_highlight_style(
        Style::default()
            .fg(COLOR_SELECTION_FG)
            .bg(COLOR_SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▸ ");

    f.render_stateful_widget(table, chunks[1], &mut app.channel_state);

    let detail = app
        .channel_state
        .selected()
        .and_then(|index| app.channels.get(index))
        .cloned();

    let detail_block = Block::default()
        .borders(Borders::ALL)
        .title(if chunks[2].width >= 60 {
            " Selected Channel | Power: raw PWM, not watts "
        } else {
            " Selected Channel "
        })
        .border_style(Style::default().fg(COLOR_BORDER))
        .bg(COLOR_SURFACE_1);
    let detail_inner = detail_block.inner(chunks[2]);
    f.render_widget(detail_block, chunks[2]);

    if let Some(channel) = detail {
        let groups = format_groups(&channel.groups, &app.group_labels);
        let power = display_power(channel.power);
        let name = if channel.name.is_empty() {
            "<unnamed>".to_string()
        } else {
            channel.name.clone()
        };
        if compact {
            let header_width = detail_inner.width.saturating_sub(9) as usize;
            let tone_width = detail_inner.width as usize;
            let detail_lines = vec![
                Line::from(vec![
                    Span::styled(
                        format!("CH {:03}", channel.channel_num),
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        truncate_line(&name, header_width),
                        Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(format!("RX {} | TX {}", channel.rx_freq, channel.tx_freq)),
                Line::from(truncate_line(
                    &format!(
                        "Tone {} | Pwr raw {}",
                        compact_tone(&channel.rx_tone, &channel.tx_tone),
                        power
                    ),
                    tone_width,
                )),
                Line::from(format!(
                    "{} {} | Group {}",
                    channel.bandwidth, channel.modulation, groups
                )),
            ];
            let detail_view = Paragraph::new(detail_lines)
                .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
                .wrap(ratatui::widgets::Wrap { trim: true });
            f.render_widget(detail_view, detail_inner);
        } else {
            let detail_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Ratio(3, 10),
                    Constraint::Ratio(4, 10),
                    Constraint::Ratio(3, 10),
                ])
                .split(detail_inner);

            let left = Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("CH {:03}", channel.channel_num),
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        truncate_line(&name, detail_chunks[0].width.saturating_sub(9) as usize),
                        Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    truncate_line(
                        &format!("{} {}", channel.bandwidth, channel.modulation),
                        detail_chunks[0].width as usize,
                    ),
                    Style::default().fg(COLOR_DIM),
                )),
            ])
            .style(Style::default().bg(COLOR_SURFACE_1));
            f.render_widget(left, detail_chunks[0]);

            let middle = Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("RX ", Style::default().fg(COLOR_DIM)),
                    Span::styled(channel.rx_freq.clone(), Style::default().fg(COLOR_TEXT)),
                    Span::raw("  "),
                    Span::styled("TX ", Style::default().fg(COLOR_DIM)),
                    Span::styled(channel.tx_freq.clone(), Style::default().fg(COLOR_TEXT)),
                ]),
                Line::from(vec![
                    Span::styled("Tones ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        truncate_line(
                            &format!("{} / {}", channel.rx_tone, channel.tx_tone),
                            detail_chunks[1].width.saturating_sub(6) as usize,
                        ),
                        Style::default().fg(COLOR_TEXT),
                    ),
                ]),
            ])
            .style(Style::default().bg(COLOR_SURFACE_1));
            f.render_widget(middle, detail_chunks[1]);

            let right = Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("Pwr raw ", Style::default().fg(COLOR_DIM)),
                    Span::styled(power, Style::default().fg(COLOR_TEXT)),
                ]),
                Line::from(vec![
                    Span::styled("Groups ", Style::default().fg(COLOR_DIM)),
                    Span::styled(
                        truncate_line(&groups, detail_chunks[2].width.saturating_sub(7) as usize),
                        Style::default().fg(COLOR_TEXT),
                    ),
                ]),
            ])
            .style(Style::default().bg(COLOR_SURFACE_1))
            .alignment(ratatui::layout::Alignment::Left);
            f.render_widget(right, detail_chunks[2]);
        }
    } else {
        let detail_bar = Paragraph::new("No channel selected")
            .style(Style::default().fg(COLOR_DIM).bg(COLOR_SURFACE_1))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(detail_bar, detail_inner);
    }
}

fn format_groups(groups: &[u8; 4], group_labels: &[String]) -> String {
    let group = groups
        .iter()
        .copied()
        .find(|group| *group != 0 && *group != 0xFF);

    match group {
        Some(group) => {
            if let Some(label) = group_label(group_labels, group) {
                label.to_string()
            } else if let Some(letter) = group_letter(group) {
                letter.to_string()
            } else {
                group.to_string()
            }
        }
        None => "-".to_string(),
    }
}

fn compact_tone(rx_tone: &str, tx_tone: &str) -> String {
    if rx_tone == tx_tone {
        rx_tone.to_string()
    } else {
        format!("{rx_tone}/{tx_tone}")
    }
}

fn compact_mode(bandwidth: &str, modulation: &str) -> String {
    match (bandwidth, modulation) {
        ("Wide", "FM") => "WFM".to_string(),
        ("Narrow", "FM") => "NFM".to_string(),
        _ => format!(
            "{} {}",
            match bandwidth {
                "Wide" => "W",
                "Narrow" => "N",
                _ => "?",
            },
            match modulation {
                "FM" => "FM",
                "AM" => "AM",
                "USB" => "USB",
                "Auto" => "Auto",
                _ => "?",
            }
        ),
    }
}

fn display_power(power: u8) -> String {
    match power {
        0 => "Off".to_string(),
        value => value.to_string(),
    }
}

fn truncate_line(text: &str, max_len: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_len {
        text.to_string()
    } else if max_len <= 3 {
        text.chars().take(max_len).collect()
    } else {
        let mut shortened = text
            .chars()
            .take(max_len.saturating_sub(3))
            .collect::<String>();
        shortened.push_str("...");
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_availability_counts_and_row_styles_use_rx_not_position() {
        use crate::protocol::Channel;
        use ratatui::{Terminal, backend::TestBackend};

        let populated = Channel {
            channel_num: 1,
            name: "AVAILABLE".to_string(),
            rx_freq: "145.35000".to_string(),
            tx_freq: "0".to_string(),
            bandwidth: "Wide".to_string(),
            modulation: "FM".to_string(),
            ..Channel::default()
        };
        let mut app = App::new();
        app.channels = vec![
            populated.clone(),
            Channel {
                channel_num: 2,
                name: "EMPTY".to_string(),
                rx_freq: "0".to_string(),
                position: 1,
                bandwidth: "Narrow".to_string(),
                ..populated.clone()
            },
            Channel {
                channel_num: 3,
                name: "DELETED".to_string(),
                ..populated
            },
        ];
        app.deleted_channels = vec![3];
        app.channel_state.select(None);
        let mut previous: Option<ratatui::buffer::Buffer> = None;
        for position in [0, 1] {
            app.channels[0].position = position;
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal
                .draw(|frame| render_channels_table(frame, &mut app, frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let rows = (0..30)
                .map(|y| {
                    (0..120)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            assert!(
                rows[0].contains("3 loaded | 1 active | 1 empty"),
                "{}",
                rows[0]
            );
            assert!(rows[0].contains("1 wide | 0 narrow | 1 FM"), "{}", rows[0]);
            assert!(rows.iter().any(|row| row.contains("2 (OFF)")));
            assert!(rows.iter().any(|row| row.contains("3 (DEL)")));
            assert!(!rows.iter().any(|row| row.contains("1 (OFF)")));
            for (name, italic, crossed_out) in [
                ("AVAILABLE", false, false),
                ("EMPTY", true, false),
                ("DELETED", false, true),
            ] {
                let y = rows.iter().position(|row| row.contains(name)).unwrap() as u16;
                let x = (0..120)
                    .find(|&x| buffer[(x, y)].symbol() == &name[..1])
                    .unwrap();
                let style = &buffer[(x, y)];
                assert_eq!(style.modifier.contains(Modifier::ITALIC), italic, "{name}");
                assert_eq!(
                    style.modifier.contains(Modifier::CROSSED_OUT),
                    crossed_out,
                    "{name}"
                );
            }
            if let Some(previous) = previous.as_ref() {
                assert_eq!(buffer, previous);
            }
            previous = Some(buffer.clone());
        }
    }

    #[test]
    fn display_power_preserves_all_nonzero_raw_pwm_values() {
        assert_eq!(display_power(0), "Off");
        assert_eq!(display_power(255), "255");
        for power in 1..=255 {
            assert_eq!(display_power(power), power.to_string());
        }
    }

    #[test]
    fn truncate_line_keeps_cells_within_requested_width() {
        assert_eq!(truncate_line("Memory Group Alpha", 9), "Memory...");
        assert_eq!(truncate_line("ABC", 2), "AB");
        assert_eq!(truncate_line("ABC", 3), "ABC");
    }

    #[test]
    fn compact_details_keep_complete_frequencies_and_group_name() {
        use crate::protocol::Channel;
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = App::new();
        app.channels = vec![Channel {
            channel_num: 123,
            name: "Repeater".to_string(),
            rx_freq: "146.52000".to_string(),
            tx_freq: "146.52000".to_string(),
            rx_tone: "Off".to_string(),
            tx_tone: "Off".to_string(),
            bandwidth: "Wide".to_string(),
            modulation: "FM".to_string(),
            power: 7,
            groups: [1, 0, 0, 0],
            ..Channel::default()
        }];
        app.group_labels = vec!["Local".to_string()];
        app.channel_state.select(Some(0));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let area = crate::ui::views::main_layout::main_content_area(Rect::new(0, 0, 80, 24));
        terminal
            .draw(|frame| render_channels_table(frame, &mut app, area))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = (area.bottom() - 5..area.bottom())
            .flat_map(|y| (area.x..area.right()).map(move |x| buffer[(x, y)].symbol()))
            .collect::<String>();
        assert!(text.contains("RX 146.52000 | TX 146.52000"), "{text}");
        assert!(text.contains("Tone Off | Pwr raw 7"), "{text}");
        assert!(text.contains("Group Local"), "{text}");
    }

    #[test]
    fn maximum_raw_power_is_numeric_and_not_watts_at_requested_sizes() {
        use crate::protocol::Channel;
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = App::new();
        app.channels = vec![Channel {
            power: 255,
            ..Channel::default()
        }];
        app.channel_state.select(Some(0));
        for (width, height) in [(80, 24), (100, 30), (160, 45), (180, 50)] {
            let area =
                crate::ui::views::main_layout::main_content_area(Rect::new(0, 0, width, height));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_channels_table(frame, &mut app, area))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text = (area.y..area.bottom())
                .flat_map(|y| (area.x..area.right()).map(move |x| buffer[(x, y)].symbol()))
                .collect::<String>();
            assert!(text.contains("Pwr raw 255"), "{width}x{height}: {text}");
            assert!(
                text.contains("raw PWM, not watts"),
                "{width}x{height}: {text}"
            );
            assert!(!text.contains("Unset"));
        }
    }
}
