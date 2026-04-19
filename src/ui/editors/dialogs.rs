use super::common::{
    DELETE_CONFIRM_HEIGHT, DELETE_CONFIRM_WIDTH, ERROR_DIALOG_HEIGHT, ERROR_DIALOG_WIDTH,
    PROGRESS_OVERLAY_HEIGHT, PROGRESS_OVERLAY_WIDTH, centered_fixed,
};
use crate::app::{App, AppMode};
use crate::ui::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, Gauge, Paragraph},
};

pub(crate) fn render_progress_overlay(
    f: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    instruction: Option<&str>,
) {
    let popup_area = centered_fixed(PROGRESS_OVERLAY_WIDTH, PROGRESS_OVERLAY_HEIGHT, area);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY));

    let inner_area = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if let Some(instr) = instruction {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(inner_area);

        let instruction_text = Paragraph::new(instr)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(COLOR_ACCENT));
        f.render_widget(instruction_text, chunks[0]);

        let status = Paragraph::new(app.status_message.as_str())
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(status, chunks[1]);

        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .bg(COLOR_SURFACE_1)
                    .add_modifier(Modifier::BOLD),
            )
            .percent((app.progress * 100.0) as u16)
            .label(format!("{:.1}%", app.progress * 100.0));
        f.render_widget(gauge, chunks[2]);

        let help = Paragraph::new("Press Esc to abort")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(COLOR_DIM));
        f.render_widget(help, chunks[3]);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(inner_area);

        let status = Paragraph::new(app.status_message.as_str())
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(status, chunks[0]);

        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .bg(COLOR_SURFACE_1)
                    .add_modifier(Modifier::BOLD),
            )
            .percent((app.progress * 100.0) as u16)
            .label(format!("{:.1}%", app.progress * 100.0));
        f.render_widget(gauge, chunks[1]);

        let help = Paragraph::new("Press Esc to cancel")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(COLOR_DIM));
        f.render_widget(help, chunks[2]);
    }
}

pub(crate) fn render_error(f: &mut Frame, msg: &str, area: Rect) {
    let dialog_area = centered_fixed(ERROR_DIALOG_WIDTH, ERROR_DIALOG_HEIGHT, area);
    f.render_widget(Clear, dialog_area);
    let p = Paragraph::new(format!("\n ERROR\n\n{}\n\nPress Esc to return", msg))
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(COLOR_ERROR))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_ERROR)),
        );
    f.render_widget(p, dialog_area);
}

pub(crate) fn render_delete_confirm(f: &mut Frame, app: &App, area: Rect) {
    if let AppMode::DeleteChannelConfirm(channel_idx) = app.mode {
        let popup_area = centered_fixed(DELETE_CONFIRM_WIDTH, DELETE_CONFIRM_HEIGHT, area);
        f.render_widget(Clear, popup_area);

        let channel_name = app
            .channels
            .get(channel_idx)
            .map(|c| format!("Channel {} ({})", c.channel_num, c.name))
            .unwrap_or_else(|| format!("Channel {}", channel_idx + 1));

        let p = Paragraph::new(format!(
            "\n DELETE CHANNEL?\n\n{}\n\nEnter to confirm, Esc to cancel",
            channel_name,
        ))
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(COLOR_TEXT))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_WARNING)),
        );
        f.render_widget(p, popup_area);
    }
}
