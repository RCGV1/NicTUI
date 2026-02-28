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

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nictui::app::{App, AppMode, MainTab};
use nictui::ui;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Print version information
    Version,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Commands::Version) = cli.command {
        println!("NicTUI {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    <B as ratatui::backend::Backend>::Error: std::error::Error + Send + Sync + 'static,
{
    let mut was_dialog_open = false;

    loop {
        app.update();

        let is_dialog_open = app.dialog_open;

        if !is_dialog_open || (was_dialog_open && !is_dialog_open) {
            terminal.draw(|f| ui::ui(f, app))?;
        }

        was_dialog_open = is_dialog_open;

        let timeout = if app.remote_active {
            Duration::from_millis(10)
        } else if app.dialog_open {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(100)
        };

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }

                match &mut app.mode {
                    AppMode::PortSelection => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Up => app.previous_port(),
                        KeyCode::Down => app.next_port(),
                        KeyCode::Char('r') => app.refresh_ports(),
                        KeyCode::Enter => app.select_port(),
                        _ => {}
                    },
                    AppMode::Main(tab) => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Tab => {
                            if *tab != MainTab::Scanning {
                                app.next_tab();
                            }
                        }
                        KeyCode::BackTab => {
                            if *tab != MainTab::Scanning {
                                app.prev_tab();
                            }
                        }
                        KeyCode::Char('1') => {
                            app.mode = AppMode::Main(MainTab::Channels);
                            app.last_main_tab = MainTab::Channels;
                        }
                        KeyCode::Char('2') => {
                            app.mode = AppMode::Main(MainTab::Settings);
                            app.last_main_tab = MainTab::Settings;
                        }
                        KeyCode::Char('3') => {
                            app.mode = AppMode::Main(MainTab::Scanning);
                            app.last_main_tab = MainTab::Scanning;
                        }
                        KeyCode::Char('4') => {
                            app.mode = AppMode::Main(MainTab::BandPlan);
                            app.last_main_tab = MainTab::BandPlan;
                        }
                        KeyCode::Char('5') => {
                            app.mode = AppMode::Main(MainTab::DTMF);
                            app.last_main_tab = MainTab::DTMF;
                        }
                        KeyCode::Char('6') => {
                            app.mode = AppMode::Main(MainTab::Remote);
                            app.last_main_tab = MainTab::Remote;
                        }
                        KeyCode::Char('7') => {
                            app.mode = AppMode::Main(MainTab::Codeplug);
                            app.last_main_tab = MainTab::Codeplug;
                        }
                        KeyCode::Char('8') => {
                            app.mode = AppMode::Main(MainTab::BinFlash);
                            app.last_main_tab = MainTab::BinFlash;
                        }
                        KeyCode::Char('9') => {
                            app.mode = AppMode::Main(MainTab::Debug);
                            app.last_main_tab = MainTab::Debug;
                        }

                        // Tab specific actions
                        _ => match tab {
                            MainTab::Channels => match key.code {
                                KeyCode::Up => app.prev_channel(),
                                KeyCode::Down => app.next_channel(),
                                KeyCode::Char('r') => app.start_read_channels(),
                                KeyCode::Char('i') => app.pick_import_file(),
                                KeyCode::Char('I') => app.pick_write_file(),
                                KeyCode::Char('e') => app.pick_export_file(),
                                KeyCode::Char('d') => {
                                    if let Some(i) = app.channel_state.selected() {
                                        app.mode = AppMode::DeleteChannelConfirm(i);
                                    }
                                }
                                KeyCode::Char('u') => app.undelete_channel(),
                                KeyCode::Char('n') => app.add_new_channel(None),
                                KeyCode::Char('w') => app.start_write_dirty_channels(),
                                KeyCode::Enter => app.start_edit_channel(),
                                _ => {}
                            },
                            MainTab::Settings => match key.code {
                                KeyCode::Up => app.prev_setting(),
                                KeyCode::Down => app.next_setting(),
                                KeyCode::Char('r') => app.start_read_settings(),
                                KeyCode::Char('w') => app.start_write_settings_and_reboot(),
                                KeyCode::Enter => app.start_edit_setting(),
                                _ => {}
                            },
                            MainTab::Scanning => match key.code {
                                KeyCode::Up => app.prev_scanning_item(),
                                KeyCode::Down => app.next_scanning_item(),
                                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                                    app.toggle_scanning_focus()
                                }
                                KeyCode::Char('r') => app.start_read_presets(),
                                KeyCode::Enter => app.start_edit_scan_preset(),
                                _ => {}
                            },
                            MainTab::BandPlan => match key.code {
                                KeyCode::Up => app.prev_bandplan(),
                                KeyCode::Down => app.next_bandplan(),
                                KeyCode::Char('r') => app.start_read_bandplan(),
                                KeyCode::Enter => app.start_edit_bandplan(),
                                _ => {}
                            },
                            MainTab::DTMF => match key.code {
                                KeyCode::Up => app.prev_dtmf(),
                                KeyCode::Down => app.next_dtmf(),
                                KeyCode::Char('r') => app.start_read_dtmf(),
                                KeyCode::Char('w') => app.start_write_dirty_dtmf(),
                                KeyCode::Enter => {
                                    if let Some(i) = app.dtmf_state.selected() {
                                        app.dtmf_edit_preset_idx = Some(i);
                                        app.mode = AppMode::EditDTMF(0);
                                        app.update_dtmf_edit_buffer();
                                    }
                                }
                                _ => {}
                            },
                            MainTab::Remote => match key.code {
                                KeyCode::Char('o') => app.remote_on(),
                                KeyCode::Char('p') => app.remote_off(),
                                KeyCode::Char('0') => app.send_key(0x80),
                                KeyCode::Char('1') => app.send_key(0x81),
                                KeyCode::Char('2') => app.send_key(0x82),
                                KeyCode::Char('3') => app.send_key(0x83),
                                KeyCode::Char('4') => app.send_key(0x84),
                                KeyCode::Char('5') => app.send_key(0x85),
                                KeyCode::Char('6') => app.send_key(0x86),
                                KeyCode::Char('7') => app.send_key(0x87),
                                KeyCode::Char('8') => app.send_key(0x88),
                                KeyCode::Char('9') => app.send_key(0x89),
                                KeyCode::Char('m') => app.send_key(0x8A),
                                KeyCode::Char('u') => app.send_key(0x8B),
                                KeyCode::Char('d') => app.send_key(0x8C),
                                KeyCode::Char('e') => app.send_key(0x8D),
                                KeyCode::Char('*') => app.send_key(0x8E),
                                KeyCode::Char('#') => app.send_key(0x8F),
                                KeyCode::Char('a') => app.send_key(0x90), // PTT-A
                                KeyCode::Char('b') => app.send_key(0x91), // PTT-B
                                KeyCode::Char('f') => app.send_key(0x92), // Flashlight
                                KeyCode::Char('v') => app.send_key(0x94), // V/M
                                _ => {}
                            },
                            MainTab::Codeplug => match key.code {
                                KeyCode::Char('i') => app.show_codeplug_import_dialog(),
                                KeyCode::Char('e') => app.show_codeplug_export_dialog(),
                                KeyCode::Char('w') => app.start_write_codeplug(),
                                _ => {}
                            },
                            MainTab::BinFlash => match key.code {
                                KeyCode::Char('i') => app.show_bin_firmware_dialog(),
                                KeyCode::Char('f') => app.start_bin_flash(),
                                _ => {}
                            },
                            _ => {}
                        },
                    },
                    AppMode::EditChannel(field_idx) => match key.code {
                        KeyCode::Esc => {
                            app.pending_channel_edit = None;
                            app.mode = AppMode::Main(MainTab::Channels);
                        }
                        KeyCode::Enter => app.commit_edit(),
                        KeyCode::Up => {
                            let idx = *field_idx;
                            app.save_current_field_to_pending(idx);
                            app.mode = AppMode::EditChannel(if idx == 0 { 10 } else { idx - 1 });
                            app.update_edit_buffer();
                        }
                        KeyCode::Down => {
                            let idx = *field_idx;
                            app.save_current_field_to_pending(idx);
                            app.mode = AppMode::EditChannel((idx + 1) % 11);
                            app.update_edit_buffer();
                        }
                        KeyCode::Left => {
                            if [6, 7, 8].contains(field_idx) {
                                let max = match field_idx {
                                    6 => 2,
                                    7 => 2,
                                    8 => 5,
                                    _ => 0,
                                };
                                if app.selection_index > 0 {
                                    app.selection_index -= 1;
                                } else {
                                    app.selection_index = max - 1;
                                }
                            }
                        }
                        KeyCode::Right => {
                            if [6, 7, 8].contains(field_idx) {
                                let max = match field_idx {
                                    6 => 2,
                                    7 => 2,
                                    8 => 5,
                                    _ => 0,
                                };
                                app.selection_index = (app.selection_index + 1) % max;
                            }
                        }
                        KeyCode::Tab => {
                            let idx = *field_idx;
                            app.save_current_field_to_pending(idx);
                            app.mode = AppMode::EditChannel((idx + 1) % 11);
                            app.update_edit_buffer();
                        }
                        KeyCode::BackTab => {
                            let idx = *field_idx;
                            app.save_current_field_to_pending(idx);
                            app.mode = AppMode::EditChannel(if idx == 0 { 10 } else { idx - 1 });
                            app.update_edit_buffer();
                        }
                        KeyCode::Backspace => {
                            app.edit_buffer.pop();
                        }
                        KeyCode::Char(c) => {
                            app.edit_buffer.push(c);
                        }
                        _ => {}
                    },
                    AppMode::EditSetting(idx) => match key.code {
                        KeyCode::Esc => app.mode = AppMode::Main(MainTab::Settings),
                        KeyCode::Enter => app.commit_setting_edit(),
                        KeyCode::Down => {
                            let meta = &nictui::protocol::SETTINGS_METADATA[*idx];
                            if let nictui::protocol::SettingType::Enum(opts) = meta.setting_type {
                                app.selection_index =
                                    (app.selection_index + opts.len() - 1) % opts.len();
                            } else if let nictui::protocol::SettingType::Boolean = meta.setting_type
                            {
                                app.selection_index = (app.selection_index + 1) % 2;
                            }
                        }
                        KeyCode::Up => {
                            let meta = &nictui::protocol::SETTINGS_METADATA[*idx];
                            if let nictui::protocol::SettingType::Enum(opts) = meta.setting_type {
                                app.selection_index = (app.selection_index + 1) % opts.len();
                            } else if let nictui::protocol::SettingType::Boolean = meta.setting_type
                            {
                                app.selection_index = (app.selection_index + 1) % 2;
                            }
                        }
                        KeyCode::Char(c) => {
                            app.edit_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.edit_buffer.pop();
                        }
                        _ => {}
                    },
                    AppMode::EditDTMF(field_idx) => match key.code {
                        KeyCode::Esc => {
                            app.dtmf_edit_preset_idx = None;
                            app.mode = AppMode::Main(MainTab::DTMF);
                        }
                        KeyCode::Enter => app.commit_dtmf_edit(),
                        KeyCode::Up => {
                            let current_idx = *field_idx;
                            let new_idx = if current_idx == 0 { 1 } else { current_idx - 1 };
                            app.save_current_dtmf_field_to_pending(current_idx);
                            app.mode = AppMode::EditDTMF(new_idx);
                            app.update_dtmf_edit_buffer();
                        }
                        KeyCode::Down => {
                            let current_idx = *field_idx;
                            app.save_current_dtmf_field_to_pending(current_idx);
                            app.mode = AppMode::EditDTMF((current_idx + 1) % 2);
                            app.update_dtmf_edit_buffer();
                        }
                        KeyCode::Tab => {
                            let current_idx = *field_idx;
                            app.save_current_dtmf_field_to_pending(current_idx);
                            app.mode = AppMode::EditDTMF((current_idx + 1) % 2);
                            app.update_dtmf_edit_buffer();
                        }
                        KeyCode::BackTab => {
                            let current_idx = *field_idx;
                            let new_idx = if current_idx == 0 { 1 } else { current_idx - 1 };
                            app.save_current_dtmf_field_to_pending(current_idx);
                            app.mode = AppMode::EditDTMF(new_idx);
                            app.update_dtmf_edit_buffer();
                        }
                        KeyCode::Char(c) => {
                            app.edit_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.edit_buffer.pop();
                        }
                        _ => {}
                    },
                    AppMode::EditScanPreset(field_idx) => match key.code {
                        KeyCode::Esc => {
                            app.editing_scan_preset = None;
                            app.mode = AppMode::Main(MainTab::Scanning);
                        }
                        KeyCode::Enter => app.commit_scan_preset_edit(),
                        KeyCode::Up => {
                            let current_idx = *field_idx;
                            let new_idx = if current_idx == 0 { 7 } else { current_idx - 1 };
                            app.save_current_scan_preset_field(current_idx);
                            app.mode = AppMode::EditScanPreset(new_idx);
                            app.update_scan_preset_edit_buffer();
                        }
                        KeyCode::Down => {
                            let current_idx = *field_idx;
                            app.save_current_scan_preset_field(current_idx);
                            app.mode = AppMode::EditScanPreset((current_idx + 1) % 8);
                            app.update_scan_preset_edit_buffer();
                        }
                        KeyCode::Tab => {
                            let current_idx = *field_idx;
                            app.save_current_scan_preset_field(current_idx);
                            app.mode = AppMode::EditScanPreset((current_idx + 1) % 8);
                            app.update_scan_preset_edit_buffer();
                        }
                        KeyCode::BackTab => {
                            let current_idx = *field_idx;
                            let new_idx = if current_idx == 0 { 7 } else { current_idx - 1 };
                            app.save_current_scan_preset_field(current_idx);
                            app.mode = AppMode::EditScanPreset(new_idx);
                            app.update_scan_preset_edit_buffer();
                        }
                        KeyCode::Left => {
                            if *field_idx == 6 {
                                app.selection_index = app.selection_index.saturating_sub(1);
                                app.update_scan_preset_edit_buffer();
                            }
                        }
                        KeyCode::Right => {
                            if *field_idx == 6 {
                                app.selection_index = (app.selection_index + 1).min(2);
                                app.update_scan_preset_edit_buffer();
                            }
                        }
                        KeyCode::Char(c) => {
                            app.edit_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.edit_buffer.pop();
                        }
                        _ => {}
                    },
                    AppMode::EditBandPlan(field_idx) => match key.code {
                        KeyCode::Esc => {
                            app.editing_band_plan = None;
                            app.mode = AppMode::Main(MainTab::BandPlan);
                        }
                        KeyCode::Enter => app.commit_bandplan_edit(),
                        KeyCode::Up => {
                            let current_idx = *field_idx;
                            let new_idx = if current_idx == 0 { 7 } else { current_idx - 1 };
                            app.save_current_bandplan_field(current_idx);
                            app.mode = AppMode::EditBandPlan(new_idx);
                            app.update_bandplan_edit_buffer();
                        }
                        KeyCode::Down => {
                            let current_idx = *field_idx;
                            app.save_current_bandplan_field(current_idx);
                            app.mode = AppMode::EditBandPlan((current_idx + 1) % 8);
                            app.update_bandplan_edit_buffer();
                        }
                        KeyCode::Tab => {
                            let current_idx = *field_idx;
                            app.save_current_bandplan_field(current_idx);
                            app.mode = AppMode::EditBandPlan((current_idx + 1) % 8);
                            app.update_bandplan_edit_buffer();
                        }
                        KeyCode::BackTab => {
                            let current_idx = *field_idx;
                            let new_idx = if current_idx == 0 { 7 } else { current_idx - 1 };
                            app.save_current_bandplan_field(current_idx);
                            app.mode = AppMode::EditBandPlan(new_idx);
                            app.update_bandplan_edit_buffer();
                        }
                        KeyCode::Left => {
                            if *field_idx >= 4 && *field_idx <= 7 {
                                app.selection_index = app.selection_index.saturating_sub(1);
                                app.update_bandplan_edit_buffer();
                            }
                        }
                        KeyCode::Right => {
                            if *field_idx >= 4 && *field_idx <= 7 {
                                app.selection_index = (app.selection_index + 1).min(2);
                                app.update_bandplan_edit_buffer();
                            }
                        }
                        KeyCode::Char(c) => {
                            app.edit_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.edit_buffer.pop();
                        }
                        _ => {}
                    },
                    AppMode::DeleteChannelConfirm(idx) => match key.code {
                        KeyCode::Esc => app.mode = AppMode::Main(MainTab::Channels),
                        KeyCode::Enter => {
                            let channel_idx = *idx;
                            app.mode = AppMode::Main(MainTab::Channels);
                            app.confirm_delete_channel(channel_idx);
                        }
                        _ => {}
                    },
                    AppMode::Reading | AppMode::Writing => match key.code {
                        KeyCode::Esc => return Ok(()),
                        _ => {}
                    },
                    AppMode::BinFlashing => match key.code {
                        KeyCode::Esc => app.mode = AppMode::Main(MainTab::BinFlash),
                        _ => {}
                    },

                    AppMode::Error(_) => match key.code {
                        KeyCode::Esc | KeyCode::Enter => app.mode = AppMode::PortSelection,
                        _ => {}
                    },
                }
            }
        }
    }
}
