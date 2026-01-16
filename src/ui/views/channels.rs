use crate::app::App;
use crate::ui::theme::{COLOR_PRIMARY, COLOR_WARNING};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_channels_table(f: &mut Frame, app: &mut App, area: Rect) {
    if app.channels.is_empty() && app.deleted_channels.is_empty() {
        let hint = Paragraph::new("\n\nNO CHANNELS LOADED\n\nPress 'r' to read from radio")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(hint, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let header = Row::new(vec![
        Cell::from("CH").style(Style::default().fg(Color::Yellow)),
        Cell::from("Name").style(Style::default().fg(Color::Yellow)),
        Cell::from("RX Freq").style(Style::default().fg(Color::Yellow)),
        Cell::from("TX Freq").style(Style::default().fg(Color::Yellow)),
        Cell::from("RX Tone").style(Style::default().fg(Color::Yellow)),
        Cell::from("TX Tone").style(Style::default().fg(Color::Yellow)),
        Cell::from("Power").style(Style::default().fg(Color::Yellow)),
        Cell::from("BW").style(Style::default().fg(Color::Yellow)),
        Cell::from("Mod").style(Style::default().fg(Color::Yellow)),
        Cell::from("Groups").style(Style::default().fg(Color::Yellow)),
    ])
    .style(Style::default().bg(Color::DarkGray))
    .height(1);

    let rows = app.channels.iter().enumerate().map(|(i, ch)| {
        let is_deleted = app.deleted_channels.contains(&ch.channel_num);
        let is_inactive = ch.position == 0;
        let is_selected = Some(i) == app.channel_state.selected();

        let style = if is_deleted {
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::CROSSED_OUT)
        } else if is_inactive {
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC)
        } else if is_selected {
            Style::default()
                .fg(Color::White)
                .bg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else if i % 2 == 0 {
            Style::default().bg(Color::Rgb(30, 30, 35))
        } else {
            Style::default()
        };

        let mut groups_str = String::new();
        for &g in ch.groups.iter() {
            if g != 0 && g != 0xFF {
                if g >= 1 && g <= 26 {
                    groups_str.push((b'A' + g - 1) as char);
                } else {
                    groups_str.push_str(&g.to_string());
                }
            }
        }
        if groups_str.is_empty() {
            groups_str = "-".to_string();
        }

        let ch_num = if is_deleted {
            format!("{} (DEL)", ch.channel_num)
        } else if is_inactive {
            format!("{} (OFF)", ch.channel_num)
        } else {
            ch.channel_num.to_string()
        };

        Row::new(vec![
            Cell::from(ch_num),
            Cell::from(ch.name.clone()),
            Cell::from(ch.rx_freq.clone()),
            Cell::from(ch.tx_freq.clone()),
            Cell::from(ch.rx_tone.clone()),
            Cell::from(ch.tx_tone.clone()),
            Cell::from(match ch.power {
                0 => "Off".to_string(),
                _ => ch.power.to_string(),
            }),
            Cell::from(ch.bandwidth.clone()),
            Cell::from(ch.modulation.clone()),
            Cell::from(groups_str),
        ])
        .style(style)
    });

    let has_changes = app.channels_dirty || !app.deleted_channels.is_empty();
    let deleted_count = app.deleted_channels.len();
    let title = if has_changes {
        if deleted_count > 0 {
            format!(" CHANNELS ({} DEL) ", deleted_count)
        } else {
            " CHANNELS (UNSAVED) ".to_string()
        }
    } else {
        " CHANNELS ".to_string()
    };

    let table = Table::new(
        rows,
        [
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
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(if has_changes {
                Style::default().fg(COLOR_WARNING)
            } else {
                Style::default().fg(Color::Cyan)
            }),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(40, 40, 80))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[0], &mut app.channel_state);
}
