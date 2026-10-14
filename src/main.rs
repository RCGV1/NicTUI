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
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use nictui::app::{App, AppMode, MainTab};
use nictui::cli::{Cli, DemoView, Dispatch, dispatch};
use nictui::skill::print_post_exit_skill_hint;
use nictui::ui;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::Duration;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match dispatch(cli)? {
        Dispatch::LaunchTui {
            port,
            demo,
            demo_view,
        } => launch_tui(port, demo, demo_view),
        Dispatch::Exit => Ok(()),
    }
}

struct TerminalSession;

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show,
            crossterm::style::ResetColor
        );
    }
}

fn launch_tui(port: Option<String>, demo: bool, demo_view: DemoView) -> Result<()> {
    let session = TerminalSession;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, DisableMouseCapture, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = if demo { App::demo() } else { App::new() };
    if demo {
        // Demo mode is intentionally hardware-free so it is safe for screenshots and UI review.
        app.mode = AppMode::Main(match demo_view {
            DemoView::Channels => MainTab::Channels,
            DemoView::Settings => MainTab::Settings,
            DemoView::Remote => {
                app.remote_active = true;
                MainTab::Remote
            }
        });
        app.last_main_tab = match app.mode {
            AppMode::Main(tab) => tab,
            _ => MainTab::Channels,
        };
    } else if let Some(port_name) = port {
        app.connect_to_port_by_name(&port_name);
    } else if should_auto_start_ble_scan() {
        app.start_ble_scan(false);
    }
    let res = run_app(&mut terminal, &mut app);

    drop(terminal);
    drop(session);
    res?;
    print_post_exit_skill_hint();

    Ok(())
}

fn should_auto_start_ble_scan() -> bool {
    !cfg!(target_os = "macos")
}

fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    <B as ratatui::backend::Backend>::Error: std::error::Error + Send + Sync + 'static,
{
    let mut was_dialog_open = false;
    let mut needs_redraw = true;

    loop {
        needs_redraw |= app.update();

        let is_dialog_open = app.dialog_open;

        if (was_dialog_open || needs_redraw) && !is_dialog_open {
            terminal.draw(|f| ui::ui(f, app))?;
            needs_redraw = false;
        }

        was_dialog_open = is_dialog_open;

        let timeout = if app.remote_active {
            Duration::from_millis(20)
        } else if app.dialog_open {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(250)
        };

        if event::poll(timeout)? {
            let key = match event::read()? {
                Event::Key(key) => key,
                Event::Resize(_, _) => {
                    needs_redraw = true;
                    continue;
                }
                _ => continue,
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }

            needs_redraw = true;
            if handle_write_in_progress_key(app, key.code) {
                continue;
            }

            match &mut app.mode {
                AppMode::PortSelection => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Up => app.previous_port(),
                    KeyCode::Down => app.next_port(),
                    KeyCode::Char('r') => app.refresh_radio_targets_from_tui(),
                    KeyCode::Char('b') => app.start_ble_scan_from_tui(),
                    KeyCode::Enter => app.select_port(),
                    _ => {}
                },
                AppMode::Main(tab) => {
                    let current_tab = *tab;
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc if current_tab != MainTab::Remote => return Ok(()),
                        KeyCode::Tab => app.next_tab(),
                        KeyCode::BackTab => app.prev_tab(),

                        // Tab specific actions
                        _ => {
                            if let Some(target) =
                                tab_shortcut_target(current_tab, app.remote_active, key.code)
                            {
                                if target == MainTab::Remote {
                                    if current_tab != MainTab::Remote {
                                        app.last_non_remote_tab = current_tab;
                                    }
                                } else {
                                    app.last_non_remote_tab = target;
                                }
                                app.mode = AppMode::Main(target);
                                app.last_main_tab = target;
                                continue;
                            }

                            match current_tab {
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
                                    KeyCode::Char('r') => app.start_read_presets(),
                                    KeyCode::Enter => app.start_edit_scan_preset(),
                                    _ => {}
                                },
                                MainTab::MemoryGroups => match key.code {
                                    KeyCode::Up => app.prev_group_item(),
                                    KeyCode::Down => app.next_group_item(),
                                    KeyCode::Char('r') => app.start_read_channels(),
                                    KeyCode::Char('w') => app.start_write_dirty_group_labels(),
                                    KeyCode::Enter => app.start_edit_group_label(),
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
                                    KeyCode::Esc if !app.remote_active => app.leave_remote_tab(),
                                    _ if maybe_send_remote_key(app, key.code) => {}
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
                            }
                        }
                    }
                }
                AppMode::EditChannel(_) => app.handle_channel_editor_key(key.code),
                AppMode::EditSetting(_) => app.handle_settings_editor_key(key.code),
                AppMode::EditDTMF(_) => app.handle_dtmf_editor_key(key.code),
                AppMode::EditScanPreset(_) => app.handle_scan_preset_editor_key(key.code),
                AppMode::EditGroupLabel(_) => app.handle_group_label_editor_key(key.code),
                AppMode::EditBandPlan(_) => app.handle_bandplan_editor_key(key.code),
                AppMode::DeleteChannelConfirm(idx) => match key.code {
                    KeyCode::Esc => app.mode = AppMode::Main(MainTab::Channels),
                    KeyCode::Enter => {
                        let channel_idx = *idx;
                        app.mode = AppMode::Main(MainTab::Channels);
                        app.confirm_delete_channel(channel_idx);
                    }
                    _ => {}
                },
                AppMode::Reading => {
                    if key.code == KeyCode::Esc {
                        return Ok(());
                    }
                }
                AppMode::Writing | AppMode::BinFlashing => {}

                AppMode::Error(_) => match key.code {
                    KeyCode::Esc | KeyCode::Enter => app.mode = AppMode::PortSelection,
                    _ => {}
                },
            }
        }
    }
}

fn handle_write_in_progress_key(app: &mut App, code: KeyCode) -> bool {
    if !matches!(app.mode, AppMode::Writing | AppMode::BinFlashing) {
        return false;
    }
    if code == KeyCode::Esc {
        app.status_message = "Write in progress. Wait for completion before leaving.".to_string();
    }
    true
}

fn tab_shortcut_target(
    current_tab: MainTab,
    remote_active: bool,
    code: KeyCode,
) -> Option<MainTab> {
    if current_tab == MainTab::Remote && remote_active {
        return None;
    }

    match code {
        KeyCode::Char('1') => Some(MainTab::Channels),
        KeyCode::Char('2') => Some(MainTab::Settings),
        KeyCode::Char('3') => Some(MainTab::Scanning),
        KeyCode::Char('4') => Some(MainTab::MemoryGroups),
        KeyCode::Char('5') => Some(MainTab::BandPlan),
        KeyCode::Char('6') => Some(MainTab::DTMF),
        KeyCode::Char('7') => Some(MainTab::Remote),
        KeyCode::Char('8') => Some(MainTab::Codeplug),
        KeyCode::Char('9') => Some(MainTab::BinFlash),
        KeyCode::Char('0') => Some(MainTab::Debug),
        _ => None,
    }
}

fn maybe_send_remote_key(app: &mut App, code: KeyCode) -> bool {
    if !app.remote_active {
        return false;
    }

    let key = match code {
        KeyCode::Char('0') => Some(0x01),
        KeyCode::Char('1') => Some(0x02),
        KeyCode::Char('2') => Some(0x03),
        KeyCode::Char('3') => Some(0x04),
        KeyCode::Char('4') => Some(0x05),
        KeyCode::Char('5') => Some(0x06),
        KeyCode::Char('6') => Some(0x07),
        KeyCode::Char('7') => Some(0x08),
        KeyCode::Char('8') => Some(0x09),
        KeyCode::Char('9') => Some(0x0A),
        KeyCode::Enter | KeyCode::Char('m') => Some(0x0B),
        KeyCode::Up | KeyCode::Char('u') => Some(0x0C),
        KeyCode::Down | KeyCode::Char('d') => Some(0x0D),
        KeyCode::Esc | KeyCode::Char('e') => Some(0x0E),
        KeyCode::Char('*') => Some(0x0F),
        KeyCode::Char('#') => Some(0x10),
        KeyCode::Char('a') => Some(0x13),
        KeyCode::Char('b') => Some(0x1A),
        KeyCode::Char('f') => Some(0x12),
        KeyCode::Char('v') => Some(0x11),
        _ => None,
    };

    if let Some(key) = key {
        app.send_key(key);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_ble_auto_scan_matches_platform_policy() {
        assert_eq!(should_auto_start_ble_scan(), !cfg!(target_os = "macos"));
    }

    #[test]
    fn writes_and_flashes_keep_the_ui_locked_until_completion() {
        let mut app = App::demo();
        for mode in [AppMode::Writing, AppMode::BinFlashing] {
            app.mode = mode;
            for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('f')] {
                assert!(handle_write_in_progress_key(&mut app, code));
                assert!(matches!(app.mode, AppMode::Writing | AppMode::BinFlashing));
            }
            assert!(app.status_message.contains("Wait for completion"));
        }
        app.mode = AppMode::Main(MainTab::BinFlash);
        assert!(!handle_write_in_progress_key(&mut app, KeyCode::Char('f')));
    }

    #[test]
    fn live_remote_tab_keeps_digit_shortcuts_for_keypad() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.last_main_tab = MainTab::Remote;
        app.remote_active = true;

        assert_eq!(
            tab_shortcut_target(MainTab::Remote, true, KeyCode::Char('1')),
            None
        );
        assert!(matches!(app.mode, AppMode::Main(MainTab::Remote)));
        assert!(maybe_send_remote_key(&mut app, KeyCode::Char('1')));
    }

    #[test]
    fn non_remote_tabs_still_use_numeric_tab_shortcuts() {
        assert_eq!(
            tab_shortcut_target(MainTab::Channels, false, KeyCode::Char('7')),
            Some(MainTab::Remote)
        );
    }

    #[test]
    fn idle_remote_tab_keeps_navigation_shortcuts() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);

        assert_eq!(
            tab_shortcut_target(MainTab::Remote, false, KeyCode::Char('8')),
            Some(MainTab::Codeplug)
        );
        assert!(!maybe_send_remote_key(&mut app, KeyCode::Char('1')));
    }

    #[test]
    fn leaving_idle_remote_returns_to_previous_non_remote_tab() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.last_non_remote_tab = MainTab::Channels;

        app.leave_remote_tab();

        assert!(matches!(app.mode, AppMode::Main(MainTab::Channels)));
    }
}
