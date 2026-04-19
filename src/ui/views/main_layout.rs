use crate::app::{App, MainTab};
use crate::ui::views::bandplan::render_bandplan;
use crate::ui::views::bin_flash::render_bin_flash_view;
use crate::ui::views::channels::render_channels_table;
use crate::ui::views::codeplug::render_codeplug_view;
use crate::ui::views::debug::render_debug_logs;
use crate::ui::views::dtmf::render_dtmf;
use crate::ui::views::footer::render_footer;
use crate::ui::views::groups::render_memory_groups_page;
use crate::ui::views::header::render_header;
use crate::ui::views::remote::render_remote_screen;
use crate::ui::views::scanning::render_scanning_page;
use crate::ui::views::settings::render_settings_table;
use crate::ui::views::sidebar::render_sidebar;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

pub fn render_main_layout(f: &mut Frame, app: &mut App, area: Rect, tab: MainTab) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(f, app, chunks[0]);

    let sidebar_width = responsive_sidebar_width(chunks[1].width);
    let content_area = if sidebar_width == 0 {
        chunks[1]
    } else {
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(sidebar_width), Constraint::Min(0)])
            .split(chunks[1]);
        render_sidebar(f, app, main_chunks[0], tab);
        main_chunks[1]
    };
    match tab {
        MainTab::Channels => render_channels_table(f, app, content_area),
        MainTab::Settings => render_settings_table(f, app, content_area),
        MainTab::Scanning => render_scanning_page(f, app, content_area),
        MainTab::MemoryGroups => render_memory_groups_page(f, app, content_area),
        MainTab::BandPlan => render_bandplan(f, app, content_area),
        MainTab::DTMF => render_dtmf(f, app, content_area),
        MainTab::Remote => render_remote_screen(f, app, content_area),
        MainTab::Codeplug => render_codeplug_view(f, app, content_area),
        MainTab::BinFlash => render_bin_flash_view(f, app, content_area),
        MainTab::Debug => render_debug_logs(f, app, content_area),
    }

    render_footer(f, app, chunks[2]);
}

fn responsive_sidebar_width(total_width: u16) -> u16 {
    match total_width {
        0..=79 => 0,
        80..=99 => 20,
        100..=119 => 22,
        _ => 28,
    }
}
