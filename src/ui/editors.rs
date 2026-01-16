use super::theme::*;
use crate::app::{App, AppMode};
use crate::protocol::{SETTINGS_METADATA, SettingType};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Table},
};

const EDITOR_MIN_WIDTH: u16 = 55;
const EDITOR_MIN_HEIGHT: u16 = 18;
const FOOTER_HEIGHT: u16 = 2;

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let terminal_width = r.width;
    let terminal_height = r.height;

    let width = if percent_x < 100 {
        ((terminal_width as u32 * percent_x as u32) / 100) as u16
    } else {
        terminal_width
    };

    let height = if percent_y < 100 {
        ((terminal_height as u32 * percent_y as u32) / 100) as u16
    } else {
        terminal_height
    };

    let width = width
        .max(EDITOR_MIN_WIDTH)
        .min(terminal_width.saturating_sub(2));
    let height = height
        .max(EDITOR_MIN_HEIGHT)
        .min(terminal_height.saturating_sub(2));

    let x = (terminal_width.saturating_sub(width) / 2).max(1);
    let y = (terminal_height.saturating_sub(height) / 2).max(1);

    Rect::new(x, y, width, height)
}

pub fn get_minimal_popup_size(num_fields: usize, has_footer: bool) -> (u16, u16) {
    let footer_height = if has_footer { FOOTER_HEIGHT } else { 0 };
    let min_height = (num_fields as u16).max(10) + footer_height + 2;
    (EDITOR_MIN_WIDTH, min_height)
}

pub fn responsive_popup_area(content_width: u16, content_height: u16, terminal: Rect) -> Rect {
    let popup_width = content_width
        .max(EDITOR_MIN_WIDTH)
        .min(terminal.width.saturating_sub(2));
    let popup_height = content_height
        .max(EDITOR_MIN_HEIGHT)
        .min(terminal.height.saturating_sub(2));

    let x = (terminal.width.saturating_sub(popup_width) / 2).max(1);
    let y = (terminal.height.saturating_sub(popup_height) / 2).max(1);

    Rect::new(x, y, popup_width, popup_height)
}

fn render_editor_scroll(
    f: &mut Frame,
    area: Rect,
    title: &str,
    fields: Vec<(&str, String, bool)>,
    current_field_idx: usize,
    edit_buffer: &str,
    selection_index: usize,
    help_text: &str,
) {
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(FOOTER_HEIGHT)])
        .split(inner_area);

    let available_height = chunks[0].height as usize;
    let num_fields = fields.len();
    let row_height = 1;
    let vertical_padding = 1;

    let visible_rows = if num_fields * row_height > (available_height - vertical_padding * 2) {
        (available_height - vertical_padding * 2).max(1)
    } else {
        num_fields
    };

    let start_row = if current_field_idx >= visible_rows {
        current_field_idx.saturating_sub(visible_rows - 1)
    } else {
        0
    };

    let visible_fields: Vec<(usize, &str, String, bool)> = fields[start_row..]
        .iter()
        .take(visible_rows)
        .enumerate()
        .map(|(i, (label, value, is_enum))| {
            let actual_idx = start_row + i;
            (actual_idx, *label, value.clone(), *is_enum)
        })
        .collect();

    let table_rows: Vec<Row> = visible_fields
        .iter()
        .map(|(idx, label, value, _)| {
            let style = if *idx == current_field_idx {
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let display_value = if *idx == current_field_idx {
                format!("> {} <", edit_buffer)
            } else {
                value.clone()
            };
            Row::new(vec![
                Cell::from(*label).style(style),
                Cell::from(display_value).style(style),
            ])
        })
        .collect();

    let table = Table::new(
        table_rows,
        [Constraint::Percentage(35), Constraint::Percentage(65)],
    )
    .block(Block::default().borders(Borders::NONE));
    f.render_widget(table, chunks[0]);

    let (_, _, is_enum) = fields[current_field_idx];
    if is_enum {
        let options = match current_field_idx {
            6 => vec!["Off", "On"],
            7 => vec!["Wide", "Narrow"],
            8 => vec!["FM", "AM", "USB", "LSB", "CW"],
            _ => vec![],
        };

        let popup_width = options.iter().map(|s| s.len()).max().unwrap_or(4) as u16 + 4;
        let popup_height = options.len() as u16 + 2;
        let popup_x = if area.x + area.width + popup_width + 2 <= f.area().width {
            area.x + area.width + 2
        } else if area.y + area.height + popup_height + 2 <= f.area().height {
            area.x + 2
        } else {
            f.area().width.saturating_sub(popup_width + 2) / 2
        };
        let popup_y = if area.y + area.height + popup_height + 2 <= f.area().height {
            area.y + area.height + 2
        } else {
            f.area().height.saturating_sub(popup_height + 2) / 2
        };

        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);
        f.render_widget(Clear, popup_area);

        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                let style = if i == selection_index {
                    Style::default()
                        .fg(Color::Black)
                        .bg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(format!(" {} ", opt)).style(style)
            })
            .collect();
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Select "))
            .style(Style::default().bg(Color::Rgb(30, 30, 35)));
        f.render_widget(list, popup_area);
    }

    if chunks[1].height > 0 {
        f.render_widget(
            Paragraph::new(help_text)
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center),
            chunks[1],
        );
    }
}

pub fn render_channel_editor(f: &mut Frame, app: &App) {
    let (min_width, min_height) = get_minimal_popup_size(12, true);
    let area = responsive_popup_area(min_width, min_height, f.area());

    if let Some(ch) = app.pending_channel_edit.as_ref() {
        let mut groups_str = String::new();
        for &g in ch.groups.iter() {
            if g != 0 && g != 0xFF {
                groups_str.push((b'A' + g - 1) as char);
            }
        }

        let current_field_idx = if let AppMode::EditChannel(idx) = app.mode {
            idx
        } else {
            0
        };

        let power_str = if ch.power == 0 {
            "Off".to_string()
        } else {
            ch.power.to_string()
        };

        let fields = vec![
            ("Name", ch.name.clone(), false),
            ("RX Frequency", ch.rx_freq.clone(), false),
            ("TX Frequency", ch.tx_freq.clone(), false),
            ("RX Tone", ch.rx_tone.clone(), false),
            ("TX Tone", ch.tx_tone.clone(), false),
            ("Power", power_str, false),
            (
                "Active",
                if ch.position == 1 {
                    "On".to_string()
                } else {
                    "Off".to_string()
                },
                true,
            ),
            ("Bandwidth", ch.bandwidth.clone(), true),
            ("Modulation", ch.modulation.clone(), true),
            ("Groups", groups_str, false),
            ("Channel Index", ch.channel_num.to_string(), false),
        ];

        render_editor_scroll(
            f,
            area,
            "CHANNEL EDITOR",
            fields,
            current_field_idx,
            &app.edit_buffer,
            app.selection_index,
            "Up/Down/Tab: Navigate | Left/Right: Select | Enter: Save | Esc: Cancel",
        );
    }
}

pub fn render_settings_editor(f: &mut Frame, app: &App) {
    if let AppMode::EditSetting(idx) = app.mode {
        let (min_width, min_height) = get_minimal_popup_size(4, true);
        let area = responsive_popup_area(min_width, min_height, f.area());
        f.render_widget(Clear, area);

        let meta = &SETTINGS_METADATA[idx];

        let block = Block::default()
            .title(format!(" {} ", meta.name))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_ACCENT));

        let inner_area = block.inner(area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(inner_area);

        match meta.setting_type {
            SettingType::Numeric { min, max, unit } => {
                let help_text = format!("Enter {}-{} {}", min, max, unit);
                if chunks[0].height > 0 {
                    f.render_widget(
                        Paragraph::new(help_text)
                            .style(Style::default().fg(Color::Gray))
                            .alignment(ratatui::layout::Alignment::Center),
                        chunks[0],
                    );
                }

                if chunks[1].height > 0 {
                    f.render_widget(
                        Paragraph::new(app.edit_buffer.as_str())
                            .block(Block::default().borders(Borders::ALL))
                            .style(Style::default().fg(Color::White).bg(Color::Rgb(20, 20, 25))),
                        chunks[1],
                    );
                }
            }
            SettingType::Boolean | SettingType::Enum(_) => {
                let options = match meta.setting_type {
                    SettingType::Boolean => vec!["Off", "On"],
                    SettingType::Enum(opts) => opts.to_vec(),
                    _ => unreachable!(),
                };

                let help_text = "Use Up/Down to select, Enter to confirm";
                if chunks[0].height > 0 {
                    f.render_widget(
                        Paragraph::new(help_text)
                            .style(Style::default().fg(Color::Gray))
                            .alignment(ratatui::layout::Alignment::Center),
                        chunks[0],
                    );
                }

                if chunks[1].height > 0 {
                    let items: Vec<ListItem> = options
                        .iter()
                        .enumerate()
                        .map(|(i, opt)| {
                            let style = if i == app.selection_index {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(COLOR_ACCENT)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::White)
                            };
                            ListItem::new(format!(" {} ", opt)).style(style)
                        })
                        .collect();

                    let list = List::new(items)
                        .block(Block::default().borders(Borders::ALL))
                        .style(Style::default().bg(Color::Rgb(30, 30, 35)));
                    f.render_widget(list, chunks[1]);
                }
            }
        }

        if chunks[2].height > 0 {
            if let Some(s) = &app.settings {
                let current_val = s.get_display_value(idx);
                f.render_widget(
                    Paragraph::new(format!("Current: {}", current_val))
                        .style(Style::default().fg(Color::DarkGray))
                        .alignment(ratatui::layout::Alignment::Center),
                    chunks[2],
                );
            }
        }
    }
}

pub fn render_dtmf_editor(f: &mut Frame, app: &App) {
    let (min_width, min_height) = get_minimal_popup_size(5, true);
    let area = responsive_popup_area(min_width, min_height, f.area());

    if let AppMode::EditDTMF(field_idx) = app.mode {
        if let Some(idx) = app.dtmf_state.selected() {
            if let Some(dtmf) = app.dtmf_presets.get(idx) {
                f.render_widget(Clear, area);

                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(" DTMF PRESET ")
                    .border_style(Style::default().fg(COLOR_ACCENT));

                let inner_area = block.inner(area);
                f.render_widget(block, area);

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Length(3),
                        Constraint::Length(2),
                        Constraint::Length(3),
                        Constraint::Length(2),
                        Constraint::Length(2),
                    ])
                    .split(inner_area);

                let digits_str: String = dtmf.digits.iter().map(|d| format!("{:X}", d)).collect();

                if chunks[0].height > 0 {
                    f.render_widget(
                        Paragraph::new("Label").style(Style::default().fg(Color::Gray)),
                        chunks[0],
                    );
                }

                if chunks[1].height > 0 {
                    let label_style = if field_idx == 0 {
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let label_display = if field_idx == 0 {
                        format!("> {} <", app.edit_buffer)
                    } else {
                        dtmf.label.clone()
                    };
                    f.render_widget(
                        Paragraph::new(label_display)
                            .block(Block::default().borders(Borders::ALL))
                            .style(label_style),
                        chunks[1],
                    );
                }

                if chunks[2].height > 0 {
                    f.render_widget(
                        Paragraph::new("Digits (0-9, A-F, *, #)")
                            .style(Style::default().fg(Color::Gray)),
                        chunks[2],
                    );
                }

                if chunks[3].height > 0 {
                    let digits_style = if field_idx == 1 {
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let digits_display = if field_idx == 1 {
                        format!("> {} <", app.edit_buffer)
                    } else {
                        digits_str.clone()
                    };
                    f.render_widget(
                        Paragraph::new(digits_display)
                            .block(Block::default().borders(Borders::ALL))
                            .style(digits_style),
                        chunks[3],
                    );
                }

                if chunks[4].height > 0 {
                    f.render_widget(
                        Paragraph::new("Keys: 0-9, A-F, *, #")
                            .style(Style::default().fg(Color::DarkGray)),
                        chunks[4],
                    );
                }

                if chunks[5].height > 0 {
                    f.render_widget(
                        Paragraph::new("Up/Down/Tab: Navigate | Enter: Save | Esc: Cancel")
                            .style(Style::default().fg(Color::DarkGray))
                            .alignment(ratatui::layout::Alignment::Center),
                        chunks[5],
                    );
                }
            }
        }
    }
}

pub fn render_progress_overlay(f: &mut Frame, app: &App, area: Rect) {
    let (min_width, min_height) = get_minimal_popup_size(3, false);
    let popup_area = responsive_popup_area(min_width, min_height, area);

    let popup_width = popup_area.width.max(50).min(area.width.saturating_sub(4));
    let popup_height = popup_area.height.max(6).min(area.height.saturating_sub(4));

    let x = (area.width.saturating_sub(popup_width) / 2).max(2);
    let y = (area.height.saturating_sub(popup_height) / 2).max(2);

    let centered_area = Rect::new(x, y, popup_width, popup_height);
    f.render_widget(Clear, centered_area);

    let block = Block::default()
        .title(" OPERATION IN PROGRESS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY));

    let inner_area = block.inner(centered_area);
    f.render_widget(block, centered_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Length(2),
        ])
        .split(inner_area);

    let status = Paragraph::new(app.status_message.as_str())
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(Color::White));
    f.render_widget(status, chunks[0]);

    if chunks[1].height > 0 {
        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .percent((app.progress * 100.0) as u16)
            .label(format!("{:.1}%", app.progress * 100.0));
        f.render_widget(gauge, chunks[1]);
    }

    if chunks[2].height > 0 {
        f.render_widget(
            Paragraph::new("Please wait...")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center),
            chunks[2],
        );
    }
}

pub fn render_error(f: &mut Frame, msg: &str, area: Rect) {
    let (min_width, min_height) = get_minimal_popup_size(8, false);
    let area = responsive_popup_area(min_width, min_height, area);
    f.render_widget(Clear, area);

    let p = Paragraph::new(format!("\n ERROR\n\n{}\n\nPress Esc to return", msg))
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(COLOR_ERROR))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_ERROR)),
        );
    f.render_widget(p, area);
}

pub fn render_delete_confirm(f: &mut Frame, app: &App, area: Rect) {
    if let AppMode::DeleteChannelConfirm(channel_idx) = app.mode {
        let (min_width, min_height) = get_minimal_popup_size(6, false);
        let popup_area = responsive_popup_area(min_width, min_height, area);
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
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_WARNING)),
        );
        f.render_widget(p, popup_area);
    }
}

pub fn render_scan_preset_editor(f: &mut Frame, app: &App) {
    let (min_width, min_height) = get_minimal_popup_size(10, true);
    let area = responsive_popup_area(min_width, min_height, f.area());

    if let Some(sp) = app.editing_scan_preset.as_ref() {
        let current_field_idx = if let AppMode::EditScanPreset(idx) = app.mode {
            idx
        } else {
            0
        };

        let mod_str = match sp.modulation {
            1 => "AM".to_string(),
            2 => "USB".to_string(),
            _ => "FM".to_string(),
        };

        let fields = vec![
            ("Label", sp.label.clone(), false),
            (
                "Start Freq",
                format!("{:.5}", sp.start_freq as f64 / 100000.0),
                false,
            ),
            ("Range (MHz)", sp.range.to_string(), false),
            ("Step (Hz)", sp.step.to_string(), false),
            ("Persist (s)", sp.persist.to_string(), false),
            ("Resume (s)", sp.resume.to_string(), false),
            ("Modulation", mod_str, true),
            ("Ultrascan", sp.ultrascan.to_string(), true),
        ];

        render_editor_scroll(
            f,
            area,
            "SCAN PRESET EDITOR",
            fields,
            current_field_idx,
            &app.edit_buffer,
            app.selection_index,
            "Up/Down/Tab: Navigate | Left/Right: Select | Enter: Save | Esc: Cancel",
        );
    }
}

pub fn render_bandplan_editor(f: &mut Frame, app: &App) {
    let (min_width, min_height) = get_minimal_popup_size(10, true);
    let area = responsive_popup_area(min_width, min_height, f.area());

    if let Some(bp) = app.editing_band_plan.as_ref() {
        let current_field_idx = if let AppMode::EditBandPlan(idx) = app.mode {
            idx
        } else {
            0
        };

        let mod_str = match bp.modulation {
            1 => "AM".to_string(),
            2 => "USB".to_string(),
            _ => "FM".to_string(),
        };

        let bw_str = match bp.bandwidth {
            1 => "Narrow".to_string(),
            _ => "Wide".to_string(),
        };

        let fields = vec![
            ("Index", bp.index.to_string(), false),
            (
                "Start Freq",
                format!("{:.5}", bp.start_freq as f64 / 100000.0),
                false,
            ),
            (
                "End Freq",
                format!("{:.5}", bp.end_freq as f64 / 100000.0),
                false,
            ),
            ("Max Power", bp.max_power.to_string(), false),
            (
                "TX Allowed",
                if bp.tx_allowed {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                },
                true,
            ),
            (
                "Wrap",
                if bp.wrap {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                },
                true,
            ),
            ("Modulation", mod_str, true),
            ("Bandwidth", bw_str, true),
        ];

        render_editor_scroll(
            f,
            area,
            "BAND PLAN EDITOR",
            fields,
            current_field_idx,
            &app.edit_buffer,
            app.selection_index,
            "Up/Down/Tab: Navigate | Left/Right: Select | Enter: Save | Esc: Cancel",
        );
    }
}
