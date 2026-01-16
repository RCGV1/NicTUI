use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::Span,
};

mod editors;
mod theme;
pub mod views;
mod widgets;

use self::editors::*;
use self::theme::*;
use crate::app::{App, AppMode, MainTab};
use views::bin_flash::*;
pub use views::*;

pub fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();
    match &app.mode {
        AppMode::PortSelection => render_port_selection(f, app, area),
        AppMode::Main(tab) => render_main_layout(f, app, area, *tab),
        AppMode::Reading | AppMode::Writing => render_progress_overlay(f, app, area),
        AppMode::BinFlashing => render_bin_flash_overlay(f, app, area),
        AppMode::EditChannel(_) => {
            render_main_layout(f, app, area, MainTab::Channels);
            render_channel_editor(f, app);
        }
        AppMode::EditSetting(_) => {
            render_main_layout(f, app, area, MainTab::Settings);
            render_settings_editor(f, app);
        }
        AppMode::EditDTMF(_) => {
            render_main_layout(f, app, area, MainTab::DTMF);
            render_dtmf_editor(f, app);
        }
        AppMode::EditScanPreset(_) => {
            render_main_layout(f, app, area, MainTab::Scanning);
            render_scan_preset_editor(f, app);
        }
        AppMode::EditBandPlan(_) => {
            render_main_layout(f, app, area, MainTab::BandPlan);
            render_bandplan_editor(f, app);
        }
        AppMode::DeleteChannelConfirm(_) => {
            render_main_layout(f, app, area, MainTab::Channels);
            render_delete_confirm(f, app, area);
        }
        AppMode::Error(msg) => render_error(f, msg, area),
    }
}

pub fn render_shortcut(key: &str) -> Span {
    Span::styled(
        format!(" {} ", key),
        Style::default()
            .fg(COLOR_HEADER)
            .bg(COLOR_PRIMARY)
            .add_modifier(Modifier::BOLD),
    )
}
