use crate::app::App;
use crate::protocol::{GROUP_LABEL_COUNT, group_display};
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG,
    COLOR_SURFACE_1, COLOR_SURFACE_2, COLOR_TEXT,
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

pub fn render_memory_groups_page(f: &mut Frame, app: &mut App, area: Rect) {
    let summary_height = if area.width < 90 { 3 } else { 2 };
    let detail_height = if area.width < 90 { 6 } else { 5 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Min(1),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let group_counts = compute_group_counts(app);
    let populated_groups = group_counts.iter().filter(|&&count| count > 0).count();
    let named_groups = app
        .group_labels
        .iter()
        .filter(|label| !label.trim().is_empty())
        .count();

    let summary_text = if area.width < 90 {
        format!("{populated_groups} used | {named_groups} named | Enter rename")
    } else {
        format!(
            "{} populated groups | {} named groups | Enter renames selected group",
            populated_groups, named_groups
        )
    };
    let summary = Paragraph::new(summary_text)
        .style(Style::default().fg(COLOR_DIM))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(summary, chunks[0]);

    render_group_list(f, app, chunks[1], &group_counts, area.width < 90);

    let detail = Paragraph::new(selected_group_lines(app, &group_counts, area.width < 90))
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Focus ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(detail, chunks[2]);
}

fn render_group_list(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    group_counts: &[u16; GROUP_LABEL_COUNT],
    compact: bool,
) {
    let border_style = Style::default().fg(COLOR_PRIMARY);

    if app.channels.is_empty() {
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "Memory Groups (A-P)",
                card_title: "Ready To Load",
                heading: "No Channels Loaded",
                description: "Read the radio to load channels and group names into the workspace."
                    .to_string(),
                note: None,
            },
            &[ReadyStateAction {
                key: "r",
                label: "read radio",
            }],
        );
        return;
    }

    let rows = (0..GROUP_LABEL_COUNT).map(|i| {
        let count = group_counts[i];
        let style = if Some(i) == app.scanning_group_state.selected() {
            Style::default()
                .bg(COLOR_SELECTION_BG)
                .fg(COLOR_SELECTION_FG)
                .add_modifier(Modifier::BOLD)
        } else if count == 0 {
            Style::default().fg(COLOR_DIM)
        } else if i % 2 == 0 {
            Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1)
        } else {
            Style::default().fg(COLOR_TEXT)
        };

        let preview = channels_for_group(app, (i + 1) as u8, if compact { 2 } else { 4 });
        Row::new(vec![
            group_display((i + 1) as u8, &app.group_labels),
            count.to_string(),
            if preview.is_empty() {
                "-".to_string()
            } else {
                preview.join(", ")
            },
        ])
        .style(style)
    });

    let header = Row::new(vec![
        Cell::from("Group").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("Count").style(Style::default().fg(COLOR_ACCENT)),
        Cell::from("Preview").style(Style::default().fg(COLOR_ACCENT)),
    ])
    .style(Style::default().bg(COLOR_SURFACE_2))
    .height(1);

    let table = Table::new(
        rows,
        [
            Constraint::Length(if compact { 12 } else { 14 }),
            Constraint::Length(8),
            Constraint::Min(if compact { 8 } else { 10 }),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" MEMORY GROUPS (A-P) ")
            .border_style(border_style)
            .bg(COLOR_SURFACE_1),
    )
    .row_highlight_style(
        Style::default()
            .fg(COLOR_SELECTION_FG)
            .bg(COLOR_SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, area, &mut app.scanning_group_state);
}

fn selected_group_lines(
    app: &App,
    group_counts: &[u16; GROUP_LABEL_COUNT],
    compact: bool,
) -> Vec<Line<'static>> {
    let selected = app
        .scanning_group_state
        .selected()
        .unwrap_or(0)
        .min(GROUP_LABEL_COUNT - 1);
    let group_num = (selected + 1) as u8;
    let preview = channels_for_group(app, group_num, 8);

    vec![
        Line::from(vec![
            Span::styled(
                format!(" Group {} ", group_display(group_num, &app.group_labels)),
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} channels", group_counts[selected])),
        ]),
        Line::from(if preview.is_empty() {
            "No channels assigned".to_string()
        } else if compact {
            format!("CH {}", preview.join(", "))
        } else {
            format!("Channels {}", preview.join(", "))
        }),
        Line::from(
            if let Some(label) = app
                .group_labels
                .get(selected)
                .filter(|label| !label.trim().is_empty())
            {
                if compact {
                    format!("Label: {}", label)
                } else {
                    format!("Stored label: {}", label)
                }
            } else if compact {
                "Label: <unnamed>".to_string()
            } else {
                "Stored label: <unnamed>".to_string()
            },
        ),
        Line::from(vec![
            render_shortcut("Enter"),
            Span::raw(": Rename | "),
            render_shortcut("w"),
            Span::raw(": Write labels | "),
            render_shortcut("r"),
            Span::raw(": Refresh | "),
            render_shortcut("3"),
            Span::raw(": Scan presets"),
        ]),
    ]
}

fn compute_group_counts(app: &App) -> [u16; GROUP_LABEL_COUNT] {
    let mut counts = [0u16; GROUP_LABEL_COUNT];
    for channel in &app.channels {
        for &group in &channel.groups {
            if (1..=GROUP_LABEL_COUNT as u8).contains(&group) {
                counts[(group - 1) as usize] += 1;
            }
        }
    }
    counts
}

fn channels_for_group(app: &App, group: u8, limit: usize) -> Vec<String> {
    app.channels
        .iter()
        .filter(|channel| channel.groups.contains(&group))
        .take(limit)
        .map(|channel| {
            if channel.name.trim().is_empty() {
                channel.channel_num.to_string()
            } else {
                format!("{} {}", channel.channel_num, channel.name)
            }
        })
        .collect()
}
