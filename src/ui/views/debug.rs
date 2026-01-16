use crate::app::App;
use crate::ui::theme::{COLOR_PRIMARY, COLOR_SECONDARY};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, List, ListItem},
};

pub fn render_debug_logs(f: &mut Frame, app: &App, area: Rect) {
    let logs: Vec<ListItem> = app
        .logs
        .iter()
        .rev()
        .map(|l| {
            let color = if l.contains("TX:") {
                COLOR_SECONDARY
            } else if l.contains("RX:") {
                COLOR_PRIMARY
            } else {
                Color::Gray
            };
            ListItem::new(l.as_str()).style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(logs).block(Block::default().borders(ratatui::widgets::Borders::NONE));
    f.render_widget(list, area);
}
