// NicTUI - Professional TDH3 Radio Programmer
// Copyright (C) 2025 Benjamin Faershtein
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use super::theme::*;
use crate::app::{App, AppMode};
use crate::protocol::{SETTINGS_METADATA, SettingType};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Table},
};

const CHANNEL_EDITOR_WIDTH: u16 = 42;
const CHANNEL_EDITOR_HEIGHT: u16 = 15;

const SETTINGS_EDITOR_WIDTH: u16 = 42;
const SETTINGS_EDITOR_HEIGHT: u16 = 15;

const DTMF_EDITOR_WIDTH: u16 = 50;
const DTMF_EDITOR_HEIGHT: u16 = 11;

const BANDPLAN_EDITOR_WIDTH: u16 = 46;
const BANDPLAN_EDITOR_HEIGHT: u16 = 17;

const SCAN_PRESET_EDITOR_WIDTH: u16 = 50;
const SCAN_PRESET_EDITOR_HEIGHT: u16 = 15;

const PROGRESS_OVERLAY_WIDTH: u16 = 80;
const PROGRESS_OVERLAY_HEIGHT: u16 = 8;

const ERROR_DIALOG_WIDTH: u16 = 50;
const ERROR_DIALOG_HEIGHT: u16 = 9;

const DELETE_CONFIRM_WIDTH: u16 = 38;
const DELETE_CONFIRM_HEIGHT: u16 = 10;

fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    Rect::new(area.x + x, area.y + y, width, height)
}

fn anchor_right_of(editor_area: Rect, popup_width: u16, popup_height: u16) -> Rect {
    let x = editor_area.x + editor_area.width + 1;
    let y = editor_area.y + 1;
    Rect::new(x, y, popup_width, popup_height)
}

pub fn render_channel_editor(f: &mut Frame, app: &App) {
    let area = centered_fixed(CHANNEL_EDITOR_WIDTH, CHANNEL_EDITOR_HEIGHT, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" CHANNEL EDITOR ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner_area = block.inner(area);
    f.render_widget(block, area);

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
            ("Groups (A-G)", groups_str, false),
            ("Channel Index", ch.channel_num.to_string(), false),
        ];

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(12), Constraint::Length(1)])
            .split(inner_area);

        let rows = fields.iter().enumerate().map(|(i, (label, value, _))| {
            let style = if i == current_field_idx {
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let display_value = if i == current_field_idx {
                format!("> {} <", app.edit_buffer)
            } else {
                value.clone()
            };
            Row::new(vec![
                Cell::from(*label).style(style),
                Cell::from(display_value).style(style),
            ])
        });

        let table = Table::new(rows, [Constraint::Length(17), Constraint::Length(24)])
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

            let popup_area = anchor_right_of(area, popup_width, popup_height);
            f.render_widget(Clear, popup_area);

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
                        Style::default().fg(Color::Gray)
                    };
                    ListItem::new(format!(" {} ", opt)).style(style)
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(""))
                .style(Style::default().bg(Color::Rgb(30, 30, 35)));
            f.render_widget(list, popup_area);
        }

        f.render_widget(
            Paragraph::new("↑/↓/Tab: Navigate | ←/→: Select | Enter: Save | Esc: Cancel")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center),
            chunks[1],
        );
    }
}

pub fn render_settings_editor(f: &mut Frame, app: &App) {
    if let AppMode::EditSetting(idx) = app.mode {
        let area = centered_fixed(SETTINGS_EDITOR_WIDTH, SETTINGS_EDITOR_HEIGHT, f.area());
        f.render_widget(Clear, area);

        let meta = &SETTINGS_METADATA[idx];

        let block = Block::default()
            .title(format!(" Edit Setting: {} ", meta.name))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_ACCENT));

        let inner_area = block.inner(area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(11), Constraint::Length(1)])
            .split(inner_area);

        match meta.setting_type {
            SettingType::Numeric { min, max, unit } => {
                let help_text = format!("Enter {}-{} {}", min, max, unit);
                f.render_widget(
                    Paragraph::new(help_text).style(Style::default().fg(Color::Gray)),
                    chunks[0],
                );

                f.render_widget(
                    Paragraph::new(app.edit_buffer.as_str())
                        .block(Block::default().borders(Borders::ALL).title(" Value "))
                        .style(Style::default().fg(Color::White)),
                    chunks[0],
                );
            }
            SettingType::Boolean | SettingType::Enum(_) => {
                let options = match meta.setting_type {
                    SettingType::Boolean => vec!["Off", "On"],
                    SettingType::Enum(opts) => opts.to_vec(),
                    _ => unreachable!(),
                };

                let rows = options.iter().enumerate().map(|(i, opt)| {
                    let style = if i == app.selection_index {
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::Rgb(40, 40, 80))
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    let display_value = if i == app.selection_index {
                        format!("> {} <", opt)
                    } else {
                        opt.to_string()
                    };
                    Row::new(vec![Cell::from(display_value).style(style)])
                });

                let table = Table::new(rows, [Constraint::Length(39)])
                    .block(Block::default().borders(Borders::NONE))
                    .row_highlight_style(
                        Style::default()
                            .bg(Color::Rgb(40, 40, 80))
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol(">> ");
                f.render_widget(table, chunks[0]);
            }
        }

        f.render_widget(
            Paragraph::new("↑/↓: Select | Enter: Save | Esc: Cancel")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center),
            chunks[1],
        );
    }
}

pub fn render_dtmf_editor(f: &mut Frame, app: &App) {
    let area = centered_fixed(DTMF_EDITOR_WIDTH, DTMF_EDITOR_HEIGHT, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Edit DTMF Preset ")
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner_area = block.inner(area);
    f.render_widget(block, area);

    if let AppMode::EditDTMF(field_idx) = app.mode {
        if let Some(idx) = app.dtmf_state.selected() {
            if let Some(dtmf) = app.dtmf_presets.get(idx) {
                let digits_str: String = dtmf.digits.iter().map(|d| format!("{:X}", d)).collect();

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ])
                    .split(inner_area);

                let label_style = if field_idx == 0 {
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let label_display = if field_idx == 0 {
                    format!("> {} <", app.edit_buffer)
                } else {
                    dtmf.label.clone()
                };
                f.render_widget(
                    Paragraph::new("Label:").style(Style::default().fg(Color::Gray)),
                    chunks[0],
                );
                f.render_widget(
                    Paragraph::new(label_display)
                        .block(Block::default().borders(Borders::ALL))
                        .style(label_style),
                    chunks[1],
                );

                let digits_style = if field_idx == 1 {
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let digits_display = if field_idx == 1 {
                    format!("> {} <", app.edit_buffer)
                } else {
                    digits_str
                };
                f.render_widget(
                    Paragraph::new("Digits:").style(Style::default().fg(Color::Gray)),
                    chunks[2],
                );
                f.render_widget(
                    Paragraph::new(digits_display)
                        .block(Block::default().borders(Borders::ALL))
                        .style(digits_style),
                    chunks[3],
                );

                f.render_widget(
                    Paragraph::new("Keys: 0-9, A-F, *, #")
                        .style(Style::default().fg(Color::DarkGray)),
                    chunks[4],
                );

                f.render_widget(
                    Paragraph::new("↑/↓/Tab: Navigate | Enter: Save | Esc: Cancel")
                        .style(Style::default().fg(Color::DarkGray))
                        .alignment(ratatui::layout::Alignment::Center),
                    chunks[5],
                );
            }
        }
    }
}

pub fn render_progress_overlay(
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
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(instruction_text, chunks[0]);

        let status = Paragraph::new(app.status_message.as_str())
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(status, chunks[1]);

        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .percent((app.progress * 100.0) as u16)
            .label(format!("{:.1}%", app.progress * 100.0));
        f.render_widget(gauge, chunks[2]);

        let help = Paragraph::new("Press Esc to abort")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
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
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .percent((app.progress * 100.0) as u16)
            .label(format!("{:.1}%", app.progress * 100.0));
        f.render_widget(gauge, chunks[1]);

        let help = Paragraph::new("Press Esc to cancel")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(help, chunks[2]);
    }
}

pub fn render_error(f: &mut Frame, msg: &str, area: Rect) {
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

pub fn render_delete_confirm(f: &mut Frame, app: &App, area: Rect) {
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
    let area = centered_fixed(
        SCAN_PRESET_EDITOR_WIDTH,
        SCAN_PRESET_EDITOR_HEIGHT,
        f.area(),
    );
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" SCAN PRESET EDITOR ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner_area = block.inner(area);
    f.render_widget(block, area);

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
            ("Ultrascan", sp.ultrascan.to_string(), false),
        ];

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(13), Constraint::Length(1)])
            .split(inner_area);

        let rows = fields.iter().enumerate().map(|(i, (label, value, _))| {
            let style = if i == current_field_idx {
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let display_value = if i == current_field_idx {
                format!("> {} <", app.edit_buffer)
            } else {
                value.clone()
            };
            Row::new(vec![
                Cell::from(*label).style(style),
                Cell::from(display_value).style(style),
            ])
        });

        let table = Table::new(rows, [Constraint::Length(16), Constraint::Length(33)])
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(table, chunks[0]);

        let (_, _, is_enum) = fields[current_field_idx];
        if is_enum {
            let options = match current_field_idx {
                6 => vec!["FM", "AM", "USB"],
                _ => vec![],
            };

            let popup_width = options.iter().map(|s| s.len()).max().unwrap_or(4) as u16 + 4;
            let popup_height = options.len() as u16 + 2;

            let popup_area = anchor_right_of(area, popup_width, popup_height);
            f.render_widget(Clear, popup_area);

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
                        Style::default().fg(Color::Gray)
                    };
                    ListItem::new(format!(" {} ", opt)).style(style)
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(""))
                .style(Style::default().bg(Color::Rgb(30, 30, 35)));
            f.render_widget(list, popup_area);
        }

        f.render_widget(
            Paragraph::new("↑/↓/Tab: Navigate | ←/→: Select | Enter: Save | Esc: Cancel")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center),
            chunks[1],
        );
    }
}

pub fn render_bandplan_editor(f: &mut Frame, app: &App) {
    let area = centered_fixed(BANDPLAN_EDITOR_WIDTH, BANDPLAN_EDITOR_HEIGHT, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" BAND PLAN EDITOR ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner_area = block.inner(area);
    f.render_widget(block, area);

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

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(15), Constraint::Length(1)])
            .split(inner_area);

        let rows = fields.iter().enumerate().map(|(i, (label, value, _))| {
            let style = if i == current_field_idx {
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let display_value = if i == current_field_idx {
                format!("> {} <", app.edit_buffer)
            } else {
                value.clone()
            };
            Row::new(vec![
                Cell::from(*label).style(style),
                Cell::from(display_value).style(style),
            ])
        });

        let table = Table::new(rows, [Constraint::Length(14), Constraint::Length(31)])
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(table, chunks[0]);

        let (_, _, is_enum) = fields[current_field_idx];
        if is_enum {
            let options = match current_field_idx {
                4 => vec!["No", "Yes"],
                5 => vec!["No", "Yes"],
                6 => vec!["FM", "AM", "USB"],
                7 => vec!["Wide", "Narrow"],
                _ => vec![],
            };

            let popup_width = options.iter().map(|s| s.len()).max().unwrap_or(4) as u16 + 4;
            let popup_height = options.len() as u16 + 2;

            let popup_area = anchor_right_of(area, popup_width, popup_height);
            f.render_widget(Clear, popup_area);

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
                        Style::default().fg(Color::Gray)
                    };
                    ListItem::new(format!(" {} ", opt)).style(style)
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(""))
                .style(Style::default().bg(Color::Rgb(30, 30, 35)));
            f.render_widget(list, popup_area);
        }

        f.render_widget(
            Paragraph::new("↑/↓/Tab: Navigate | ←/→: Select | Enter: Save | Esc: Cancel")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center),
            chunks[1],
        );
    }
}
