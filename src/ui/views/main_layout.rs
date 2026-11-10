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
    let chunks = main_regions(area);

    render_header(f, app, chunks[0]);

    let (sidebar, content_area) = body_regions(chunks[1]);
    if let Some(sidebar) = sidebar {
        render_sidebar(f, app, sidebar, tab);
    }
    match tab {
        MainTab::Activity => crate::ui::views::activity::render_activity(f, app, content_area),
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

fn responsive_sidebar_width(body: Rect) -> u16 {
    if body.height < 20 {
        return 0;
    }
    match body.width {
        0..=99 => 0,
        100..=119 => 22,
        120..=199 => 23,
        _ => 28,
    }
}

pub fn remote_key_at_terminal(area: Rect, column: u16, row: u16) -> Option<u8> {
    crate::ui::views::remote::remote_key_at(main_content_area(area), column, row)
}

pub fn main_content_area(area: Rect) -> Rect {
    body_regions(main_regions(area)[1]).1
}

fn main_regions(area: Rect) -> [Rect; 3] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);
    [chunks[0], chunks[1], chunks[2]]
}

fn body_regions(body: Rect) -> (Option<Rect>, Rect) {
    let sidebar = responsive_sidebar_width(body).min(body.width);
    (
        (sidebar != 0).then(|| Rect::new(body.x, body.y, sidebar, body.height)),
        Rect::new(body.x + sidebar, body.y, body.width - sidebar, body.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_windows_reserve_the_whole_body_for_the_active_view() {
        for (width, height) in [(60, 20), (80, 24), (99, 40), (160, 24)] {
            let body = main_regions(Rect::new(0, 0, width, height))[1];
            assert_eq!(body_regions(body), (None, body));
        }
        for (width, height) in [(100, 30), (120, 36), (190, 80)] {
            let body = main_regions(Rect::new(0, 0, width, height))[1];
            let (sidebar, content) = body_regions(body);
            assert!(sidebar.is_some());
            assert_eq!(sidebar.unwrap().right(), content.x);
            assert_eq!(content.right(), body.right());
        }
    }

    #[test]
    fn remote_hit_targets_use_the_same_responsive_body_as_rendering() {
        for (width, height) in [(60, 20), (80, 24), (100, 30), (160, 24), (190, 80)] {
            let area = Rect::new(0, 0, width, height);
            let chunks = main_regions(area);
            let (sidebar, content) = body_regions(chunks[1]);
            for y in 0..height {
                for x in 0..width {
                    let key = remote_key_at_terminal(area, x, y);
                    assert_eq!(key, crate::ui::views::remote::remote_key_at(content, x, y));
                    if !chunks[1].contains((x, y).into())
                        || sidebar.is_some_and(|rect| rect.contains((x, y).into()))
                    {
                        assert!(key.is_none());
                    }
                }
            }
        }
    }
}
