use crate::app::App;
use crate::app::MainTab;
use crate::ui::theme::{COLOR_BORDER, COLOR_PRIMARY, COLOR_SIDEBAR};
use ratatui::{
    Frame,
    layout::Rect,
    prelude::Stylize,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem},
};

pub fn render_sidebar(f: &mut Frame, _app: &App, area: Rect, active_tab: MainTab) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(COLOR_BORDER))
        .bg(COLOR_SIDEBAR);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let tabs = vec![
        (MainTab::Channels, "1 Channels"),
        (MainTab::Settings, "2 Settings"),
        (MainTab::Scanning, "3 Scanning"),
        (MainTab::BandPlan, "4 BandPlan"),
        (MainTab::DTMF, "5 DTMF"),
        (MainTab::Remote, "6 Remote"),
        (MainTab::Codeplug, "7 Codeplug"),
        (MainTab::BinFlash, "8 BIN Flash"),
        (MainTab::Debug, "9 Debug"),
    ];

    let items: Vec<ListItem> = tabs
        .into_iter()
        .map(|(tab, label)| {
            let style = if tab == active_tab {
                Style::default()
                    .fg(Color::Black)
                    .bg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(format!(" {} ", label)).style(style)
        })
        .collect();

    let list =
        List::new(items).block(Block::default().padding(ratatui::widgets::Padding::vertical(1)));
    f.render_widget(list, inner);
}
