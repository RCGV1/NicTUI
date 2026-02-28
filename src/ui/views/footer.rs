use crate::app::{App, AppMode, MainTab};
use crate::ui::render_shortcut;
use crate::ui::theme::VERSION;
use crate::ui::theme::{COLOR_BORDER, COLOR_PRIMARY, COLOR_WARNING};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(10), Constraint::Fill(1)])
        .split(area);

    let status = Paragraph::new(format!(" {}  |  {} ", app.status_message, VERSION))
        .style(Style::default().bg(Color::Rgb(20, 20, 25)))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(status, chunks[0]);

    let hints = match app.mode {
        AppMode::Main(MainTab::Channels) => {
            let has_changes = app.channels_dirty || !app.deleted_channels.is_empty();
            let mut hints = vec![
                render_shortcut("i"),
                Span::raw(" import | "),
                render_shortcut("r"),
                Span::raw(" read | "),
            ];
            if has_changes {
                hints.push(render_shortcut("w"));
                hints.push(Span::styled(
                    " SAVE ",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                hints.push(render_shortcut("w"));
                hints.push(Span::raw(" write "));
            }
            hints.push(Span::raw(" | "));
            hints.push(render_shortcut("n"));
            hints.push(Span::raw(" new | "));
            hints.push(render_shortcut("d"));
            hints.push(Span::raw(" del "));
            if !app.deleted_channels.is_empty() {
                hints.push(Span::raw(" | "));
                hints.push(render_shortcut("u"));
                hints.push(Span::raw(" undel "));
            }
            Line::from(hints)
        }
        AppMode::Main(MainTab::Settings) => {
            let mut hints = vec![
                render_shortcut("r"),
                Span::raw(" read | "),
                render_shortcut("Enter"),
                Span::raw(" edit "),
            ];
            if app.settings_dirty {
                hints.push(Span::raw("| "));
                hints.push(render_shortcut("w"));
                hints.push(Span::styled(
                    " Save & Reboot ",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                hints.push(Span::raw("| "));
                hints.push(render_shortcut("w"));
                hints.push(Span::raw(" write "));
            }
            Line::from(hints)
        }
        AppMode::Main(MainTab::Scanning) => Line::from(vec![
            render_shortcut("r"),
            Span::raw(" read | "),
            render_shortcut("Enter"),
            Span::raw(" edit | "),
        ]),
        AppMode::Main(MainTab::BandPlan) => {
            Line::from(vec![render_shortcut("r"), Span::raw(" read ")])
        }
        AppMode::Main(MainTab::DTMF) => {
            let mut hints = vec![
                render_shortcut("r"),
                Span::raw(" read | "),
                render_shortcut("Enter"),
                Span::raw(" edit | "),
            ];
            if app.dtmf_dirty {
                hints.push(render_shortcut("w"));
                hints.push(Span::styled(
                    " SAVE ",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                hints.push(render_shortcut("w"));
                hints.push(Span::raw(" write "));
            }
            Line::from(hints)
        }
        AppMode::Main(MainTab::Remote) => Line::from(vec![
            render_shortcut("a"),
            Span::raw(" ptt-a | "),
            render_shortcut("b"),
            Span::raw(" ptt-b | "),
            render_shortcut("f"),
            Span::raw(" light"),
        ]),
        AppMode::Main(MainTab::Codeplug) => Line::from(vec![
            render_shortcut("i"),
            Span::raw(" import | "),
            render_shortcut("e"),
            Span::raw(" export | "),
            render_shortcut("w"),
            Span::raw(" write "),
        ]),
        AppMode::Main(MainTab::BinFlash) => Line::from(vec![
            render_shortcut("i"),
            Span::raw(" import | "),
            render_shortcut("f"),
            Span::raw(" flash "),
        ]),
        AppMode::Main(MainTab::Debug) => Line::from(vec![]),
        _ => Line::from(vec![]),
    };

    let hints_p = Paragraph::new(hints)
        .alignment(ratatui::layout::Alignment::Center)
        .style(
            Style::default()
                .fg(COLOR_PRIMARY)
                .bg(Color::Rgb(20, 20, 25)),
        )
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(hints_p, chunks[1]);
}
