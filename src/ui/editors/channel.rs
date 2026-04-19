use super::common::{
    CHANNEL_EDITOR_HEIGHT, CHANNEL_EDITOR_WIDTH, begin_editor, render_option_popup,
};
use crate::app::{App, AppMode};
use crate::protocol::{Channel, GROUP_LABEL_COUNT, group_label, group_letter};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub(crate) fn render_channel_editor(f: &mut Frame, app: &App) {
    let Some(ch) = app.pending_channel_edit.as_ref() else {
        return;
    };

    let (area, inner_area) = begin_editor(
        f,
        format!(" Channel {} Editor ", ch.channel_num),
        CHANNEL_EDITOR_WIDTH,
        CHANNEL_EDITOR_HEIGHT,
    );

    let current_field_idx = if let AppMode::EditChannel(idx) = app.mode {
        idx
    } else {
        0
    };

    let power_str = if ch.power == 0 {
        "Off".to_string()
    } else {
        ch.power.to_string()
    };

    let fields = [
        ("Name", ch.name.clone(), false),
        ("RX Frequency", ch.rx_freq.clone(), false),
        ("TX Frequency", ch.tx_freq.clone(), false),
        ("RX Tone", ch.rx_tone.clone(), false),
        ("TX Tone", ch.tx_tone.clone(), false),
        ("Power", power_str, false),
        (
            "Active",
            if ch.position == 1 { "On" } else { "Off" }.to_string(),
            true,
        ),
        ("Bandwidth", ch.bandwidth.clone(), true),
        ("Modulation", ch.modulation.clone(), true),
        (
            "Scan Group",
            primary_group_label(ch, &app.group_labels),
            true,
        ),
        ("Index", ch.channel_num.to_string(), false),
    ];
    let current_field = fields[current_field_idx].0;
    let current_value = &fields[current_field_idx].1;
    let (help_title, help_lines) = channel_field_help_lines(ch, current_field_idx);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(2)])
        .split(inner_area);

    let body = if use_stacked_channel_editor(vertical[0].width) {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(7), Constraint::Min(10)])
            .split(vertical[0])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(31), Constraint::Min(28)])
            .split(vertical[0])
    };

    let mut detail_lines = vec![
        Line::from(vec![
            Span::styled("Editing ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(current_field, Style::default().fg(COLOR_TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Selected ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(current_value.to_string()),
        ]),
        Line::default(),
        Line::from(Span::styled(
            help_title,
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    detail_lines.extend(help_lines);
    detail_lines.push(Line::default());
    detail_lines.push(Line::from(if fields[current_field_idx].2 {
        "Use ←/→ to change the selected option."
    } else {
        "Type to update the value directly."
    }));

    let details = Paragraph::new(detail_lines)
        .style(Style::default().fg(COLOR_TEXT))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Details "));
    f.render_widget(details, body[0]);

    let rows = fields.iter().enumerate().map(|(i, (label, value, _))| {
        let style = if i == current_field_idx {
            Style::default()
                .fg(COLOR_SELECTION_FG)
                .bg(COLOR_SELECTION_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_DIM)
        };
        let display_value = if i == current_field_idx {
            format!("> {} <", app.edit_buffer)
        } else {
            value.clone()
        };
        Row::new(vec![
            Cell::from(*label).style(style),
            Cell::from(display_value).style(style),
        ])
    });

    let label_width = if body[1].width < 42 { 14 } else { 16 };
    let table = Table::new(rows, [Constraint::Length(label_width), Constraint::Min(10)])
        .block(Block::default().borders(Borders::ALL).title(" Fields "))
        .row_highlight_style(
            Style::default()
                .fg(COLOR_SELECTION_FG)
                .bg(COLOR_SELECTION_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" ");
    f.render_widget(table, body[1]);

    if fields[current_field_idx].2 {
        let group_options = group_option_labels(&app.group_labels);
        let options: Vec<_> = match current_field_idx {
            6 => vec!["Off".to_string(), "On".to_string()],
            7 => vec!["Wide".to_string(), "Narrow".to_string()],
            8 => vec![
                "FM".to_string(),
                "AM".to_string(),
                "USB".to_string(),
                "LSB".to_string(),
                "CW".to_string(),
            ],
            9 => group_options,
            _ => vec![],
        };
        render_option_popup(f, area, &options, app.selection_index);
    }

    let footer = Paragraph::new("↑/↓/Tab: Field | ←/→: Option | Enter: Save | Esc: Cancel")
        .style(Style::default().fg(COLOR_DIM))
        .alignment(ratatui::layout::Alignment::Center)
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(footer, vertical[1]);
}

fn use_stacked_channel_editor(width: u16) -> bool {
    width < 68
}

fn channel_field_help_lines(_ch: &Channel, field_idx: usize) -> (&'static str, Vec<Line<'static>>) {
    match field_idx {
        0 => (
            "Channel Name",
            vec![Line::from(
                "Name shown in the channel list. Keep it short and readable.",
            )],
        ),
        1 => (
            "RX Frequency",
            vec![Line::from(
                "Receive frequency in MHz, for example 146.52000.",
            )],
        ),
        2 => (
            "TX Frequency",
            vec![Line::from(
                "Transmit frequency in MHz. Match RX for simplex or offset for repeaters.",
            )],
        ),
        3 => (
            "RX Tone",
            vec![Line::from(
                "Enter the receive tone value or OFF when no receive tone is needed.",
            )],
        ),
        4 => (
            "TX Tone",
            vec![Line::from(
                "Enter the transmit tone value or OFF to transmit carrier only.",
            )],
        ),
        5 => (
            "Power",
            vec![Line::from(
                "Numeric transmit power value. Use 0 to disable transmit on this channel.",
            )],
        ),
        6 => (
            "Active",
            vec![Line::from(
                "On keeps the channel available. Off parks it without deleting it.",
            )],
        ),
        7 => (
            "Bandwidth",
            vec![Line::from("Choose wide or narrow channel bandwidth.")],
        ),
        8 => (
            "Modulation",
            vec![Line::from(
                "Choose the transmit and receive modulation mode.",
            )],
        ),
        9 => (
            "Scan Groups",
            vec![
                Line::from("Pick the scan group for this channel."),
                Line::from("Go to Memory Groups to rename group names."),
            ],
        ),
        10 => (
            "Index",
            vec![Line::from(
                "Memory slot number used when the channel is written back to the radio.",
            )],
        ),
        _ => ("Field", vec![]),
    }
}

fn group_option_labels(labels: &[String]) -> Vec<String> {
    let mut options = Vec::with_capacity(GROUP_LABEL_COUNT + 1);
    options.push("None".to_string());
    for group in 1..=GROUP_LABEL_COUNT as u8 {
        options.push(group_slot_label(group, labels));
    }
    options
}

fn group_slot_label(group: u8, labels: &[String]) -> String {
    if group == 0 || group == 0xFF {
        "None".to_string()
    } else if let Some(label) = group_label(labels, group) {
        label.to_string()
    } else if let Some(letter) = group_letter(group) {
        letter.to_string()
    } else {
        group.to_string()
    }
}

fn primary_group(ch: &Channel) -> u8 {
    ch.groups
        .iter()
        .copied()
        .find(|group| (1..=GROUP_LABEL_COUNT as u8).contains(group))
        .unwrap_or(0)
}

fn primary_group_label(ch: &Channel, labels: &[String]) -> String {
    group_slot_label(primary_group(ch), labels)
}

#[cfg(test)]
mod tests {
    use super::{
        group_option_labels, group_slot_label, primary_group, primary_group_label,
        use_stacked_channel_editor,
    };
    use crate::protocol::Channel;

    #[test]
    fn desktop_channel_editor_stays_side_by_side() {
        assert!(!use_stacked_channel_editor(70));
        assert!(!use_stacked_channel_editor(82));
    }

    #[test]
    fn narrow_channel_editor_stacks() {
        assert!(use_stacked_channel_editor(60));
    }

    #[test]
    fn group_slot_label_uses_saved_name_when_available() {
        let mut labels = vec![String::new(); 16];
        labels[0] = "Ham".to_string();

        assert_eq!(group_slot_label(1, &labels), "Ham");
        assert_eq!(group_slot_label(2, &labels), "B");
        assert_eq!(group_slot_label(0, &labels), "None");
    }

    #[test]
    fn group_option_labels_include_none_and_all_groups() {
        let options = group_option_labels(&vec![String::new(); 16]);
        assert_eq!(options.len(), 17);
        assert_eq!(options[0], "None");
    }

    #[test]
    fn primary_group_uses_first_non_empty_group() {
        let channel = Channel {
            groups: [1, 0, 2, 0xFF],
            ..Channel::default()
        };
        assert_eq!(primary_group(&channel), 1);
    }

    #[test]
    fn primary_group_label_uses_saved_name() {
        let mut labels = vec![String::new(); 16];
        labels[1] = "Dispatch".to_string();
        let channel = Channel {
            groups: [0, 2, 3, 0],
            ..Channel::default()
        };

        assert_eq!(primary_group_label(&channel, &labels), "Dispatch");
    }
}
