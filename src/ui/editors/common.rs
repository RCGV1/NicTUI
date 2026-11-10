use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

pub(super) const CHANNEL_EDITOR_WIDTH: u16 = 84;
pub(super) const CHANNEL_EDITOR_HEIGHT: u16 = 20;
pub(super) const SETTINGS_EDITOR_WIDTH: u16 = 56;
pub(super) const SETTINGS_EDITOR_HEIGHT: u16 = 17;
pub(super) const DTMF_EDITOR_WIDTH: u16 = 66;
pub(super) const DTMF_EDITOR_HEIGHT: u16 = 16;
pub(super) const BANDPLAN_EDITOR_WIDTH: u16 = 68;
pub(super) const BANDPLAN_EDITOR_HEIGHT: u16 = 19;
pub(super) const SCAN_PRESET_EDITOR_WIDTH: u16 = 68;
pub(super) const SCAN_PRESET_EDITOR_HEIGHT: u16 = 18;
pub(super) const PROGRESS_OVERLAY_WIDTH: u16 = 80;
pub(super) const PROGRESS_OVERLAY_HEIGHT: u16 = 8;
pub(super) const ERROR_DIALOG_WIDTH: u16 = 50;
pub(super) const ERROR_DIALOG_HEIGHT: u16 = 9;
pub(super) const DELETE_CONFIRM_WIDTH: u16 = 38;
pub(super) const DELETE_CONFIRM_HEIGHT: u16 = 10;

pub(super) fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let max_width = area.width.saturating_sub(2).max(1);
    let max_height = area.height.saturating_sub(2).max(1);
    let width = width.min(max_width).max(1);
    let height = height.min(max_height).max(1);
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    Rect::new(area.x + x, area.y + y, width, height)
}

pub(super) fn anchor_right_of(editor_area: Rect, popup_width: u16, popup_height: u16) -> Rect {
    let width = popup_width.min(editor_area.width.saturating_sub(2).max(1));
    // Leave the editor's save/cancel rows unobscured by long option lists.
    let height = popup_height.min(editor_area.height.saturating_sub(4).max(1));
    Rect::new(
        editor_area.x + editor_area.width.saturating_sub(width + 1),
        editor_area.y + 1,
        width,
        height,
    )
}

pub(super) fn field_body(area: Rect, details_width: u16, fields_width: u16) -> [Rect; 2] {
    let chunks = if area.width < details_width + fields_width {
        Layout::vertical([Constraint::Length(4), Constraint::Min(4)]).split(area)
    } else {
        Layout::horizontal([
            Constraint::Length(details_width),
            Constraint::Min(fields_width),
        ])
        .split(area)
    };
    [chunks[0], chunks[1]]
}

pub(super) fn editor_footer<'a>(status: &'a str, error_prefix: &str) -> Paragraph<'a> {
    let error = status.starts_with(error_prefix);
    Paragraph::new(if error {
        format!("{status}\nEnter: Save | Esc: Cancel")
    } else {
        match error_prefix {
            "Invalid setting:" => "↑/↓: Option | Enter: Save | Esc: Cancel",
            "Invalid DTMF:" => "↑/↓/Tab: Field | Enter: Save | Esc: Cancel",
            _ => "↑/↓/Tab: Field | ←/→: Option | Enter: Save | Esc: Cancel",
        }
        .to_string()
    })
    .style(Style::default().fg(if error { COLOR_WARNING } else { COLOR_DIM }))
    .alignment(ratatui::layout::Alignment::Center)
    .wrap(Wrap { trim: true })
}

pub(super) fn begin_editor(f: &mut Frame, title: String, width: u16, height: u16) -> (Rect, Rect) {
    let area = centered_fixed(width, height, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner_area = block.inner(area);
    f.render_widget(block, area);
    (area, inner_area)
}

pub(super) fn render_option_popup(
    f: &mut Frame,
    editor_area: Rect,
    options: &[impl AsRef<str>],
    selection_index: usize,
) {
    let popup_width = options.iter().map(|s| s.as_ref().len()).max().unwrap_or(4) as u16 + 6;
    let popup_height = options.len() as u16 + 2;
    let popup_area = anchor_right_of(editor_area, popup_width, popup_height);
    f.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let style = if i == selection_index {
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_TEXT)
            };
            let marker = if i == selection_index { ">" } else { " " };
            ListItem::new(format!(" {} {} ", marker, opt.as_ref())).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Options "))
        .style(Style::default().bg(COLOR_SURFACE_1));
    let mut state = ListState::default();
    if selection_index < options.len() {
        state.select(Some(selection_index));
    }
    f.render_stateful_widget(list, popup_area, &mut state);
}
