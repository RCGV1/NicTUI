use super::common::{CHANNEL_EDITOR_WIDTH, begin_editor};
use crate::app::App;
use crate::ui::theme::{COLOR_ACCENT, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_TEXT};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub(crate) fn render_group_label_editor(f: &mut Frame, app: &App) {
    let Some(index) = app.editing_group_label_idx else {
        return;
    };

    let title = format!(" Group {} Name ", (b'A' + index as u8) as char);
    let (_area, inner_area) = begin_editor(f, title, CHANNEL_EDITOR_WIDTH.saturating_sub(6), 11);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Length(3)])
        .split(inner_area);

    let details = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Current ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(
                app.group_labels
                    .get(index)
                    .map(String::as_str)
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or("<unnamed>"),
            ),
        ]),
        Line::default(),
        Line::from(Span::styled(
            "This name appears anywhere the app shows this memory group.",
            Style::default().fg(COLOR_ACCENT),
        )),
        Line::from("Use up to 6 characters. Enter saves, Esc cancels."),
    ])
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(Block::default().borders(Borders::ALL).title(" Details "));

    let input = Paragraph::new(app.edit_buffer.as_str())
        .style(
            Style::default()
                .fg(COLOR_TEXT)
                .bg(COLOR_SELECTION_BG)
                .add_modifier(Modifier::BOLD),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Name ")
                .border_style(Style::default().fg(COLOR_PRIMARY)),
        );

    f.render_widget(details, chunks[0]);
    f.render_widget(input, chunks[1]);
}
