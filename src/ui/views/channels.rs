use crate::app::App;
use crate::protocol::{group_label, group_letter};
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_SELECTION_BG, COLOR_SELECTION_FG, COLOR_SURFACE_1,
    COLOR_SURFACE_2, COLOR_TEXT, COLOR_WARNING,
};
use crate::ui::views::ready_state::{ReadyStateAction, ReadyStateContent, render_ready_state};
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
        let port_label = app.selected_port_short_label();
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "Channels",
                card_title: "Ready To Load",
                heading: "No Channels Loaded",
                description: format!(
                    "Selected {port_label}. Read the radio or import a file to populate the workspace."
                ),
                note: Some(
                    "Read the radio first for live changes, or import a file to edit offline.",
                ),
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

    let compact = area.width < 100;
    let summary_height = if compact { 3 } else { 2 };
    let detail_height = if area.width < 90 { 5 } else { 6 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Min(1),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let active_loaded = app
        .channels
        .iter()
        .filter(|ch| !app.deleted_channels.contains(&ch.channel_num))
        .collect::<Vec<_>>();
    let active_count = active_loaded.iter().filter(|ch| ch.position == 1).count();
    let parked_count = active_loaded.len().saturating_sub(active_count);
    let wide_count = active_loaded
        .iter()
        .filter(|ch| ch.bandwidth == "Wide")
        .count();
    let narrow_count = active_loaded.len().saturating_sub(wide_count);
    let fm_count = active_loaded
        .iter()
        .filter(|ch| ch.modulation == "FM")
        .count();

    let summary = Paragraph::new(format!(
        "{} loaded | {} active | {} parked | {} wide | {} narrow | {} FM",
        app.channels.len(),
        active_count,
        parked_count,
        wide_count,
        narrow_count,
        fm_count
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
            Cell::from("Power").style(
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
        let is_inactive = ch.position == 0;
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
            format!(" Channels ({} pending delete) ", deleted_count)
        } else {
            " Channels (Unsaved) ".to_string()
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
                Constraint::Length(9),
                Constraint::Length(9),
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
                Constraint::Length(6),
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
        .title(" Selected Channel ")
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
            let freq_width = detail_inner.width.saturating_sub(10) as usize;
            let tone_width = detail_inner.width.saturating_sub(21) as usize;
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
                Line::from(truncate_line(
                    &format!(
                        "{} {} | RX {} | TX {}",
                        channel.bandwidth, channel.modulation, channel.rx_freq, channel.tx_freq
                    ),
                    freq_width,
                )),
                Line::from(truncate_line(
                    &format!(
                        "Tone {} | Pwr {} | Group {}",
                        compact_tone(&channel.rx_tone, &channel.tx_tone),
                        power,
                        groups
                    ),
                    tone_width,
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
                    Span::styled("Power ", Style::default().fg(COLOR_DIM)),
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
        _ => format!("{}{}", bandwidth.chars().next().unwrap_or('-'), modulation),
    }
}

fn display_power(power: u8) -> String {
    match power {
        0 => "Off".to_string(),
        0xFF => "Unset".to_string(),
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
    fn display_power_uses_plain_language_for_sentinel_values() {
        assert_eq!(display_power(0), "Off");
        assert_eq!(display_power(0xFF), "Unset");
        assert_eq!(display_power(7), "7");
    }

    #[test]
    fn truncate_line_keeps_cells_within_requested_width() {
        assert_eq!(truncate_line("Memory Group Alpha", 9), "Memory...");
        assert_eq!(truncate_line("ABC", 2), "AB");
        assert_eq!(truncate_line("ABC", 3), "ABC");
    }
}
