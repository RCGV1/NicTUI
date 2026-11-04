use crate::app::{App, AppMode, MainTab};
use crate::ui::render_shortcut;
use crate::ui::theme::VERSION;
use crate::ui::theme::{
    COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SURFACE_0, COLOR_SURFACE_2, COLOR_TEXT,
    COLOR_WARNING,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let target_left_width = if area.width < 56 {
        14
    } else if area.width < 90 {
        18
    } else {
        24
    };
    let left_width = target_left_width.min(area.width);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left_width), Constraint::Min(0)])
        .split(area);

    let compact = chunks[0].width < 18;
    let nav = if matches!(app.mode, AppMode::Main(MainTab::Remote)) && app.remote_active {
        Line::from(vec![render_shortcut("Tab"), Span::raw(" leave")])
    } else if compact {
        Line::from(Span::styled(" Views ", Style::default().fg(COLOR_PRIMARY)))
    } else {
        let mut spans = vec![
            render_shortcut("Tab"),
            Span::styled(
                if area.width < 90 {
                    " views"
                } else {
                    " / 0-9 views"
                },
                Style::default().fg(COLOR_DIM),
            ),
        ];
        if area.width >= 90 {
            spans.push(Span::styled(
                format!(" {VERSION}"),
                Style::default().fg(COLOR_DIM),
            ));
        }
        Line::from(spans)
    };

    let status = Paragraph::new(nav)
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_0))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(status, chunks[0]);

    let hints = footer_hints(app, chunks[1].width);
    let hints_p = Paragraph::new(hints)
        .alignment(ratatui::layout::Alignment::Left)
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_2))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(hints_p, chunks[1]);
}

fn footer_hints(app: &App, width: u16) -> Line<'static> {
    let compact = width < 72;
    let tight = width < 44;
    let mut hints = Vec::new();

    match app.mode {
        AppMode::Main(MainTab::Channels) => {
            let has_changes = app.channels_dirty || !app.deleted_channels.is_empty();
            let channels_empty = app.channels.is_empty() && app.deleted_channels.is_empty();
            if has_changes {
                push_warning_hint(&mut hints, "w", if tight { "save" } else { "save changes" });
                push_hint(&mut hints, "Enter", "edit");
                push_hint(&mut hints, "n", "new");
                if !tight {
                    push_hint(&mut hints, "d", "del");
                }
                if !app.deleted_channels.is_empty() {
                    push_hint(&mut hints, "u", "undo del");
                }
            } else if channels_empty {
                push_hint(&mut hints, "r", "read");
                push_hint(
                    &mut hints,
                    "i",
                    if tight { "import" } else { "import file" },
                );
                push_hint(&mut hints, "n", "new");
            } else {
                push_hint(&mut hints, "Enter", "edit");
                push_hint(&mut hints, "n", "new");
                push_hint(&mut hints, "d", "del");
                if !tight {
                    push_hint(&mut hints, "w", "write");
                }
                if !compact {
                    push_hint(&mut hints, "i", "import");
                    push_hint(&mut hints, "r", "read");
                }
            }
        }
        AppMode::Main(MainTab::Settings) => {
            if app.settings_dirty {
                push_warning_hint(
                    &mut hints,
                    "w",
                    if tight { "save" } else { "save + reboot" },
                );
                push_hint(&mut hints, "Enter", "edit");
                if !tight {
                    push_hint(&mut hints, "r", "read");
                }
            } else {
                push_hint(&mut hints, "Enter", "edit");
                push_hint(&mut hints, "r", "read");
                if !tight {
                    push_hint(&mut hints, "w", "write");
                }
            }
        }
        AppMode::Main(MainTab::Scanning) => {
            if app.scan_presets_dirty {
                push_warning_hint(&mut hints, "w", "write");
            }
            push_hint(&mut hints, "Enter", "edit");
            push_hint(&mut hints, "r", "read");
        }
        AppMode::Main(MainTab::MemoryGroups) => {
            if app.group_labels_dirty {
                push_warning_hint(&mut hints, "w", if tight { "save" } else { "save names" });
            }
            push_hint(&mut hints, "Enter", "rename");
            push_hint(&mut hints, "r", "read");
        }
        AppMode::Main(MainTab::BandPlan) => {
            if app.band_plans_dirty {
                push_warning_hint(&mut hints, "w", "write");
            }
            push_hint(&mut hints, "Enter", "edit");
            push_hint(&mut hints, "r", "read");
        }
        AppMode::Main(MainTab::DTMF) => {
            if app.dtmf_dirty {
                push_warning_hint(&mut hints, "w", "save");
            }
            push_hint(&mut hints, "Enter", "edit");
            push_hint(&mut hints, "r", "read");
            if !app.dtmf_dirty && !tight {
                push_hint(&mut hints, "w", "write");
            }
        }
        AppMode::Main(MainTab::Remote) if app.remote_active => {
            push_warning_hint(&mut hints, "p", "stop");
            push_hint(&mut hints, "D", "details");
            push_hint(&mut hints, "Tab", "leave");
            push_hint(&mut hints, "Up/Down", "move");
            push_hint(&mut hints, "Enter", "menu");
            if !compact {
                push_hint(&mut hints, "Esc", "exit menu");
                push_hint(&mut hints, "a", "PTT");
                push_hint(&mut hints, "f/v", "F/B");
            }
        }
        AppMode::Main(MainTab::Remote) => {
            push_hint(&mut hints, "o", "start");
            push_hint(&mut hints, "D", "details");
            push_hint(&mut hints, "Esc", "back");
            if !tight {
                push_hint(&mut hints, "Tab", "next");
            }
        }
        AppMode::Main(MainTab::Codeplug) => {
            push_hint(&mut hints, "r", "read full");
            push_hint(&mut hints, "i", "import");
            if app.codeplug_snapshot_is_current() {
                push_hint(&mut hints, "e", "export");
                if crate::ui::views::codeplug::write_block_reason(app, None).is_none() {
                    push_warning_hint(&mut hints, "w", "write");
                }
            }
        }
        AppMode::Main(MainTab::BinFlash) => {
            push_hint(&mut hints, "i", "import");
            push_warning_hint(&mut hints, "f", "flash");
        }
        AppMode::Main(MainTab::Debug) => {
            push_hint(&mut hints, "q", "quit");
        }
        AppMode::EditChannel(_)
        | AppMode::EditSetting(_)
        | AppMode::EditDTMF(_)
        | AppMode::EditScanPreset(_)
        | AppMode::EditGroupLabel(_)
        | AppMode::EditBandPlan(_) => {
            push_warning_hint(&mut hints, "Enter", "save");
            push_hint(&mut hints, "Esc", "cancel");
        }
        AppMode::DeleteChannelConfirm(_) => {
            push_warning_hint(&mut hints, "Enter", "delete");
            push_hint(&mut hints, "Esc", "cancel");
        }
        AppMode::Error(_) => {
            push_hint(&mut hints, "Esc", "back");
        }
        _ => {
            push_hint(&mut hints, "q", "quit");
        }
    }

    // Drop optional hints as whole groups, never a half key/label at the edge.
    let mut fitted = Vec::new();
    let mut used = 0;
    for group in hints.split(|span| span.content == "  | ") {
        let gap = if fitted.is_empty() { 0 } else { 4 };
        let needed = group.iter().map(Span::width).sum::<usize>() + gap;
        if used + needed > width as usize {
            break;
        }
        push_separator(&mut fitted);
        fitted.extend_from_slice(group);
        used += needed;
    }
    Line::from(fitted)
}

fn push_hint(spans: &mut Vec<Span<'static>>, key: &'static str, label: &'static str) {
    push_separator(spans);
    spans.push(render_shortcut(key));
    spans.push(Span::raw(format!(" {label}")));
}

fn push_warning_hint(spans: &mut Vec<Span<'static>>, key: &'static str, label: &'static str) {
    push_separator(spans);
    spans.push(render_shortcut(key));
    spans.push(Span::styled(
        format!(" {label}"),
        Style::default()
            .fg(COLOR_WARNING)
            .add_modifier(Modifier::BOLD),
    ));
}

fn push_separator(spans: &mut Vec<Span<'static>>) {
    if !spans.is_empty() {
        spans.push(Span::styled("  | ", Style::default().fg(COLOR_DIM)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint_text(app: &App, width: u16) -> String {
        footer_hints(app, width)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn remote_active_footer_omits_start_hint() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;

        let text = hint_text(&app, 120);

        assert!(text.contains(" stop"));
        assert!(!text.contains(" start"));
        assert!(!text.contains("switch tabs"));
    }

    #[test]
    fn remote_active_footer_fits_common_terminal_width() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;

        let text = hint_text(&app, 62);

        assert!(text.chars().count() <= 62);
        assert!(text.contains(" leave"));
        assert!(!text.contains("exit menu"));
    }

    #[test]
    fn debug_footer_still_has_a_real_hint() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Debug);

        assert!(hint_text(&app, 60).contains(" quit"));
    }

    #[test]
    fn dirty_channel_footer_prioritizes_save_at_narrow_width() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Channels);
        app.channels_dirty = true;

        let text = hint_text(&app, 36);

        assert!(text.contains(" save"));
        assert!(text.contains(" edit"));
        assert!(!text.contains(" import"));
    }

    #[test]
    fn hints_fit_without_partial_key_labels_at_every_width() {
        let mut app = App::new();
        app.channels_dirty = true;
        app.settings_dirty = true;
        app.remote_active = true;
        for tab in [
            MainTab::Channels,
            MainTab::Settings,
            MainTab::Remote,
            MainTab::Codeplug,
            MainTab::BinFlash,
        ] {
            app.mode = AppMode::Main(tab);
            for width in 0..160 {
                let line = footer_hints(&app, width);
                assert!(line.width() <= width as usize, "{tab:?} at {width}");
                let text = hint_text(&app, width);
                assert!(!text.ends_with(" | "));
            }
        }
    }

    #[test]
    fn active_remote_navigation_does_not_advertise_digit_tab_shortcuts() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        let mut terminal = Terminal::new(TestBackend::new(160, 2)).unwrap();
        terminal
            .draw(|frame| render_footer(frame, &app, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..160)
            .map(|x| buffer[(x, 1)].symbol())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Tab leave"));
        assert!(!text.contains("0-9 views"));
        assert!(text.contains("f/v F/B"));
    }

    #[test]
    fn scan_and_band_write_hints_take_priority_until_saved() {
        let mut app = App::new();
        for tab in [MainTab::Scanning, MainTab::BandPlan] {
            app.mode = AppMode::Main(tab);
            app.scan_presets_dirty = tab == MainTab::Scanning;
            app.band_plans_dirty = tab == MainTab::BandPlan;
            let text = hint_text(&app, 36);
            assert!(text.contains(" write"), "{text}");
            assert!(text.contains(" edit"), "{text}");
            assert!(footer_hints(&app, 36).width() <= 36);
            app.scan_presets_dirty = false;
            app.band_plans_dirty = false;
            assert!(!hint_text(&app, 36).contains(" write"));
        }
    }

    #[test]
    fn codeplug_footer_prioritizes_full_read_and_hides_blocked_actions() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Codeplug);
        for width in [62, 76, 136, 156] {
            let text = hint_text(&app, width)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            assert!(text.starts_with("r read full"), "{text}");
            assert!(text.contains("i import"));
            assert!(!text.contains("e export"));
            assert!(!text.contains("w write"));
        }
        app.codeplug_data = Some(vec![0; crate::protocol::EEPROM_SIZE]);
        app.codeplug_workspace_baseline = app.workspace_snapshot();
        app.protocol_port_name = Some("test://no-hardware".to_string());
        let text = hint_text(&app, 62);
        assert!(text.contains(" export"));
        assert!(text.contains(" write"));
        app.scan_presets_dirty = true;
        let text = hint_text(&app, 62);
        assert!(!text.contains(" export"));
        assert!(!text.contains(" write"));
    }
}
