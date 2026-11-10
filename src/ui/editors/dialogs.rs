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

const PROGRESS_QUIT_HINT: &str = "Esc: finish safely, then quit";

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

        let status = Paragraph::new(sanitized_progress_status(&app.status_message))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(status, chunks[1]);

        let progress_percent = progress_percent(app.progress);
        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .bg(COLOR_SURFACE_1)
                    .add_modifier(Modifier::BOLD),
            )
            .percent(progress_percent)
            .label(format!("{}%", progress_percent));
        f.render_widget(gauge, chunks[2]);

        let help = Paragraph::new(PROGRESS_QUIT_HINT)
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

        let status = Paragraph::new(sanitized_progress_status(&app.status_message))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(status, chunks[0]);

        let progress_percent = progress_percent(app.progress);
        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .bg(COLOR_SURFACE_1)
                    .add_modifier(Modifier::BOLD),
            )
            .percent(progress_percent)
            .label(format!("{}%", progress_percent));
        f.render_widget(gauge, chunks[1]);

        let help = Paragraph::new(PROGRESS_QUIT_HINT)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(COLOR_DIM));
        f.render_widget(help, chunks[2]);
    }
}

fn progress_percent(progress: f64) -> u16 {
    if !progress.is_finite() {
        return 0;
    }

    progress.clamp(0.0, 1.0).mul_add(100.0, 0.0).round() as u16
}

fn sanitized_progress_status(status: &str) -> String {
    if let Some(rest) = status.strip_prefix("Opening ")
        && let Some((_, action)) = rest.split_once(" and ")
    {
        let mut chars = action.chars();
        if let Some(first) = chars.next() {
            return first.to_uppercase().collect::<String>() + chars.as_str();
        }
    }

    status.to_string()
}

pub(crate) fn render_error(f: &mut Frame, msg: &str, area: Rect) {
    let dialog_area = centered_fixed(ERROR_DIALOG_WIDTH, ERROR_DIALOG_HEIGHT, area);
    f.render_widget(Clear, dialog_area);
    let block = Block::default()
        .title(" ERROR ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ERROR));
    let inner = block.inner(dialog_area);
    f.render_widget(block, dialog_area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let p = Paragraph::new(msg)
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(COLOR_ERROR))
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(p, chunks[0]);
    f.render_widget(
        Paragraph::new("Press Esc to return").alignment(ratatui::layout::Alignment::Center),
        chunks[1],
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn progress_quit_hint_matches_deferred_exit_and_fits_narrow_overlays() {
        for mode in [AppMode::Reading, AppMode::Writing, AppMode::BinFlashing] {
            let mut app = App::new();
            app.mode = mode;
            app.status_message = "Working".to_string();
            for width in [33, 40, 60, 80, 120] {
                let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
                terminal
                    .draw(|frame| crate::ui::ui(frame, &mut app))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let rendered: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                assert!(
                    rendered.contains("Esc: finish safely, then quit"),
                    "missing or clipped quit hint for {:?} at width {width}",
                    app.mode
                );
                assert!(!rendered.contains("Esc to cancel"));
                assert!(!rendered.contains("Esc to abort"));
            }
        }
    }
}
