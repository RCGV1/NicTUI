use crate::app::App;
use crate::protocol::editor_value::{SCAN_MODULATIONS, decimal_value, option_label};
use crate::ui::render_shortcut;
use crate::ui::theme::{
    COLOR_ACCENT, COLOR_BORDER, COLOR_DIM, COLOR_PRIMARY, COLOR_SELECTION_BG, COLOR_SELECTION_FG,
    COLOR_SURFACE_1, COLOR_SURFACE_2, COLOR_TEXT, COLOR_WARNING,
};
use crate::ui::views::ready_state::{ReadyStateContent, render_ready_state};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub fn render_scanning_page(f: &mut Frame, app: &mut App, area: Rect) {
    if app.scan_presets.is_empty() {
        render_ready_state(
            f,
            area,
            ReadyStateContent {
                outer_title: "Scan Presets",
                card_title: "Ready To Load",
                heading: "No Scan Presets Loaded",
                description: "r: read scan presets | Shift+R: read all sections | 8: import .nfw"
                    .into(),
                note: None,
            },
            &[],
        );
        return;
    }
    let summary_height = if area.width < 90 { 3 } else { 2 };
    let detail_height = if area.width < 90 { 7 } else { 6 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Min(1),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let summary_text = if app.scan_presets_dirty {
        format!(
            "Unsaved local | w write radio | {} presets",
            app.scan_presets.len()
        )
    } else if area.width < 90 {
        format!("{} presets | Enter edit | 4 groups", app.scan_presets.len())
    } else {
        format!(
            "{} presets | Enter edits selected preset | 4 opens memory groups",
            app.scan_presets.len()
        )
    };
    let summary = Paragraph::new(summary_text)
        .style(Style::default().fg(if app.scan_presets_dirty {
            COLOR_WARNING
        } else {
            COLOR_DIM
        }))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
    f.render_widget(summary, chunks[0]);

    render_preset_table(f, app, chunks[1], area.width < 90);

    let detail = Paragraph::new(selected_scan_preset_lines(app, area.width < 90))
        .style(Style::default().fg(COLOR_TEXT).bg(COLOR_SURFACE_1))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Focus ")
                .border_style(Style::default().fg(COLOR_BORDER))
                .bg(COLOR_SURFACE_1),
        );
    f.render_widget(detail, chunks[2]);
}

fn render_preset_table(f: &mut Frame, app: &mut App, area: Rect, compact: bool) {
    let preset_border_style = Style::default().fg(if app.scan_presets_dirty {
        COLOR_WARNING
    } else {
        COLOR_PRIMARY
    });

    let header = if compact {
        Row::new(vec![
            Cell::from("#").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Label").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Start MHz").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("MHz").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("kHz").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Mod").style(Style::default().fg(COLOR_ACCENT)),
        ])
    } else {
        Row::new(vec![
            Cell::from("#").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Label").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Start MHz").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Range").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Step").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Resume").style(Style::default().fg(COLOR_ACCENT)),
            Cell::from("Mod").style(Style::default().fg(COLOR_ACCENT)),
        ])
    }
    .style(Style::default().bg(COLOR_SURFACE_2))
    .height(1);

    let rows = app.scan_presets.iter().enumerate().map(|(i, preset)| {
        let is_selected = Some(i) == app.preset_state.selected();
        let style = if is_selected {
            Style::default()
                .bg(COLOR_SELECTION_BG)
                .fg(COLOR_SELECTION_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_TEXT)
        };

        if compact {
            Row::new(vec![
                preset.index.to_string(),
                preset.label.clone(),
                decimal_value(preset.start_freq, 5),
                decimal_value(preset.range as u32, 2),
                decimal_value(preset.step as u32, 2),
                option_label(SCAN_MODULATIONS, preset.modulation),
            ])
        } else {
            Row::new(vec![
                preset.index.to_string(),
                preset.label.clone(),
                decimal_value(preset.start_freq, 5),
                format!("{} MHz", decimal_value(preset.range as u32, 2)),
                format!("{} kHz", decimal_value(preset.step as u32, 2)),
                format!("{}s", preset.resume),
                option_label(SCAN_MODULATIONS, preset.modulation),
            ])
        }
        .style(style)
    });

    let table = Table::new(
        rows,
        if compact {
            vec![
                Constraint::Length(3),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Min(9),
            ]
        } else {
            vec![
                Constraint::Length(4),
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Min(9),
            ]
        },
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(if app.scan_presets_dirty {
                " SCAN PRESETS (Unsaved) "
            } else {
                " SCAN PRESETS "
            })
            .border_style(preset_border_style)
            .bg(COLOR_SURFACE_1),
    )
    .row_highlight_style(
        Style::default()
            .fg(COLOR_SELECTION_FG)
            .bg(COLOR_SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, area, &mut app.preset_state);
}

fn selected_scan_preset_lines(app: &App, compact: bool) -> Vec<Line<'static>> {
    let Some(index) = app.preset_state.selected() else {
        return vec![Line::from("No scan preset selected")];
    };
    let Some(preset) = app.scan_presets.get(index) else {
        return vec![Line::from("No scan preset selected")];
    };

    let mut controls = vec![
        render_shortcut("Enter"),
        Span::raw(": Edit | "),
        render_shortcut("r"),
        Span::raw(": Read"),
    ];
    if app.scan_presets_dirty {
        controls.extend([
            Span::raw(" | "),
            render_shortcut("w"),
            Span::styled(": Write", Style::default().fg(COLOR_WARNING)),
        ]);
    }
    if !compact {
        controls.extend([
            Span::raw(" | "),
            render_shortcut("4"),
            Span::raw(": Memory groups"),
        ]);
    }

    vec![
        Line::from(vec![
            Span::styled(
                format!(" P{:02} ", preset.index),
                Style::default()
                    .fg(COLOR_SELECTION_FG)
                    .bg(COLOR_SELECTION_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {}",
                if preset.label.is_empty() {
                    "<unnamed>"
                } else {
                    preset.label.as_str()
                }
            )),
        ]),
        Line::from(if compact {
            format!(
                "{} | {} MHz | {} kHz",
                decimal_value(preset.start_freq, 5),
                decimal_value(preset.range as u32, 2),
                decimal_value(preset.step as u32, 2)
            )
        } else {
            format!(
                "Start {} MHz | Range {} MHz | Step {} kHz",
                decimal_value(preset.start_freq, 5),
                decimal_value(preset.range as u32, 2),
                decimal_value(preset.step as u32, 2)
            )
        }),
        Line::from(if compact {
            format!(
                "Res {}s | Persist {}s | {} | Ultra raw {}",
                preset.resume,
                decimal_value(preset.persist as u32, 1),
                option_label(SCAN_MODULATIONS, preset.modulation),
                preset.ultrascan
            )
        } else {
            format!(
                "Resume {}s | Persist {}s | {} | Ultrascan raw {}",
                preset.resume,
                decimal_value(preset.persist as u32, 1),
                option_label(SCAN_MODULATIONS, preset.modulation),
                preset.ultrascan
            )
        }),
        Line::from(controls),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppEvent, AppMode, MainTab, WriteScope};
    use crate::protocol::{BandPlan, DTMFPreset, ScanPreset};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn domain_tables_render_protocol_values_without_hex_or_scaling_errors() {
        let mut app = App::new();
        app.scan_presets.push(ScanPreset {
            start_freq: 11_800_000,
            range: 1900,
            step: 1250,
            persist: 200,
            ultrascan: 20,
            ..ScanPreset::default()
        });
        app.preset_state.select(Some(0));
        app.band_plans.push(BandPlan {
            modulation: 4,
            bandwidth: 5,
            max_power: 255,
            ..BandPlan::default()
        });
        app.bandplan_state.select(Some(0));
        app.dtmf_presets.push(DTMFPreset {
            digits: vec![12, 14, 15],
            ..DTMFPreset::default()
        });
        app.dtmf_state.select(Some(0));
        for (tab, expected) in [
            (MainTab::Scanning, "19.00 MHz"),
            (MainTab::BandPlan, "Enforce FM"),
            (MainTab::DTMF, "C*#"),
        ] {
            app.mode = AppMode::Main(tab);
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal
                .draw(|frame| match tab {
                    MainTab::Scanning => render_scanning_page(frame, &mut app, frame.area()),
                    MainTab::BandPlan => {
                        crate::ui::views::bandplan::render_bandplan(frame, &mut app, frame.area())
                    }
                    MainTab::DTMF => {
                        crate::ui::views::dtmf::render_dtmf(frame, &mut app, frame.area())
                    }
                    _ => unreachable!(),
                })
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains(expected), "{expected}");
            if matches!(tab, MainTab::Scanning) {
                assert!(text.contains("12.50 kHz"));
                assert!(text.contains("20.0s"));
            }
            if matches!(tab, MainTab::BandPlan) {
                assert!(text.contains("FM Tuner"));
                assert!(text.contains("Pwr raw"));
            }
        }
    }

    #[test]
    fn scan_and_band_unsaved_state_and_write_actions_survive_full_layout() {
        for tab in [MainTab::Scanning, MainTab::BandPlan] {
            let mut app = App::new();
            app.mode = AppMode::Main(tab);
            app.last_main_tab = tab;
            app.scan_presets.push(ScanPreset::default());
            app.preset_state.select(Some(0));
            app.band_plans.push(BandPlan::default());
            app.bandplan_state.select(Some(0));
            for dirty in [true, false] {
                if dirty {
                    app.scan_presets_dirty = tab == MainTab::Scanning;
                    app.band_plans_dirty = tab == MainTab::BandPlan;
                } else {
                    let scope = if tab == MainTab::Scanning {
                        WriteScope::ScanPresets
                    } else {
                        WriteScope::BandPlans
                    };
                    app.event_tx.send(AppEvent::WriteComplete(scope)).unwrap();
                    app.update();
                }
                assert_eq!(app.scan_presets_dirty, dirty && tab == MainTab::Scanning);
                assert_eq!(app.band_plans_dirty, dirty && tab == MainTab::BandPlan);
                for (width, height) in [(80, 24), (100, 30), (160, 45), (180, 50)] {
                    let content = crate::ui::views::main_layout::main_content_area(Rect::new(
                        0, 0, width, height,
                    ));
                    let sidebar = content.x;
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal
                        .draw(|frame| {
                            crate::ui::views::main_layout::render_main_layout(
                                frame,
                                &mut app,
                                frame.area(),
                                tab,
                            )
                        })
                        .unwrap();
                    let buffer = terminal.backend().buffer();
                    let text = (0..height)
                        .map(|y| {
                            (0..width)
                                .map(|x| buffer[(x, y)].symbol())
                                .collect::<String>()
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let nav = (3..height - 2)
                        .map(|y| {
                            (0..sidebar)
                                .map(|x| buffer[(x, y)].symbol())
                                .collect::<String>()
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if dirty {
                        assert!(text.contains("DIRTY"), "{tab:?} {width}x{height}: {text}");
                        assert!(text.contains("Unsaved"), "{text}");
                        assert!(
                            text.contains(if tab == MainTab::Scanning {
                                "pending scan*"
                            } else {
                                "pending band*"
                            }),
                            "{text}"
                        );
                        if sidebar > 0 {
                            assert!(nav.contains("1*"), "{nav}");
                            assert!(nav.contains("1 dirty"), "{nav}");
                        }
                        let footer = (0..width)
                            .map(|x| buffer[(x, height - 1)].symbol())
                            .collect::<String>()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        assert!(footer.contains("w write"), "{footer}");
                    } else {
                        assert!(!text.contains("DIRTY"), "{text}");
                        assert!(!text.contains("pending scan*"));
                        assert!(!text.contains("pending band*"));
                        assert!(!text.contains("Unsaved"));
                        assert!(!nav.contains("1*"));
                    }
                }
            }
        }
    }
}
