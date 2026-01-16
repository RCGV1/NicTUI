use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_scanning_page(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let border_style = Style::default().fg(Color::Cyan);

    if app.scan_presets.is_empty() {
        let hint = Paragraph::new("NO SCAN PRESETS\n\nPress 'r' to read from radio")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" SCAN PRESETS ")
                    .border_style(border_style),
            );
        f.render_widget(hint, chunks[0]);
    } else {
        let header = Row::new(vec![
            Cell::from("#").style(Style::default().fg(Color::Yellow)),
            Cell::from("Label").style(Style::default().fg(Color::Yellow)),
            Cell::from("Start Freq").style(Style::default().fg(Color::Yellow)),
            Cell::from("Range").style(Style::default().fg(Color::Yellow)),
            Cell::from("Step").style(Style::default().fg(Color::Yellow)),
            Cell::from("Persist").style(Style::default().fg(Color::Yellow)),
            Cell::from("Resume").style(Style::default().fg(Color::Yellow)),
            Cell::from("Mod").style(Style::default().fg(Color::Yellow)),
        ])
        .style(Style::default().bg(Color::DarkGray))
        .height(1);

        let rows = app.scan_presets.iter().enumerate().map(|(i, sp)| {
            let is_selected = Some(i) == app.preset_state.selected();

            let style = if is_selected {
                Style::default()
                    .bg(Color::Rgb(50, 50, 80))
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if i % 2 == 0 {
                Style::default().bg(Color::Rgb(30, 30, 35))
            } else {
                Style::default()
            };

            let mod_str = match sp.modulation {
                1 => "AM",
                2 => "USB",
                _ => "FM",
            };

            Row::new(vec![
                sp.index.to_string(),
                sp.label.clone(),
                format!("{:.5}", sp.start_freq as f64 / 100000.0),
                format!("{} MHz", sp.range),
                format!("{} Hz", sp.step),
                format!("{}s", sp.persist),
                format!("{}s", sp.resume),
                mod_str.to_string(),
            ])
            .style(style)
        });

        let table = Table::new(
            rows,
            [
                Constraint::Length(4),
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(6),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" SCAN PRESETS (Press Enter to edit) ")
                .border_style(border_style),
        )
        .highlight_symbol(">> ");

        f.render_stateful_widget(table, chunks[0], &mut app.preset_state);
    }

    render_group_list(f, app, chunks[1]);
}

fn render_group_list(f: &mut Frame, app: &App, area: Rect) {
    let border_style = Style::default().fg(Color::Cyan);

    let group_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    if app.channels.is_empty() {
        let hint = Paragraph::new("NO CHANNELS\n\nPress 'r' to read from radio")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" MEMORY GROUPS (A-O) ")
                    .border_style(border_style),
            );
        f.render_widget(hint, group_chunks[0]);
        return;
    }

    let mut group_counts = [0u16; 26];
    for ch in &app.channels {
        for &g in ch.groups.iter() {
            if g >= 1 && g <= 26 {
                group_counts[(g - 1) as usize] += 1;
            }
        }
    }

    let mut group_rows = Vec::new();
    for i in 0..26 {
        let letter = (b'A' + i) as char;
        let count = group_counts[i as usize];
        let is_empty = count == 0;

        let style = if is_empty {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };

        let channels_in_group: Vec<String> = app
            .channels
            .iter()
            .filter(|ch| ch.groups.contains(&(i + 1)))
            .take(5)
            .map(|ch| format!("{}", ch.channel_num))
            .collect();
        let channels_str = if channels_in_group.is_empty() {
            String::new()
        } else {
            format!("CH: {}", channels_in_group.join(", "))
        };

        let row_channels = channels_str.clone();
        group_rows.push(
            Row::new(vec![
                format!("Group {}", letter),
                format!("{} channels", count),
                row_channels,
            ])
            .style(style)
            .height(if channels_str.is_empty() { 1 } else { 2 }),
        );
    }

    let table = Table::new(
        group_rows,
        [
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Min(10),
        ],
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" MEMORY GROUPS (A-O) ")
            .border_style(border_style),
    );

    f.render_widget(table, group_chunks[0]);
}
