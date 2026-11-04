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
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use nictui::app::{App, AppMode, MainTab};
use nictui::cancellation;
use nictui::cli::{Cli, DemoView, Dispatch, dispatch};
use nictui::skill::print_post_exit_skill_hint;
use nictui::ui;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    cancellation::install_handler()?;
    let cli = Cli::parse();
    if cancellation::is_requested() {
        anyhow::bail!("Interrupted: signal requested before command started");
    }

    match dispatch(cli)? {
        Dispatch::LaunchTui {
            port,
            demo,
            demo_view,
        } => launch_tui(port, demo, demo_view),
        Dispatch::Exit => Ok(()),
    }?;
    if cancellation::is_requested() {
        anyhow::bail!("Interrupted: signal requested; active operation cleanup finished");
    }
    Ok(())
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
    if !demo {
        app.refresh_ports();
    }
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
    run_app_with_events_and_mouse(
        terminal,
        app,
        |timeout| {
            if event::poll(timeout)? {
                event::read().map(Some)
            } else {
                Ok(None)
            }
        },
        |enabled| {
            if enabled {
                execute!(io::stdout(), EnableMouseCapture)
            } else {
                execute!(io::stdout(), DisableMouseCapture)
            }
        },
    )
}

#[cfg(test)]
fn run_app_with_events<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    next_event: impl FnMut(Duration) -> io::Result<Option<Event>>,
) -> Result<()>
where
    <B as ratatui::backend::Backend>::Error: std::error::Error + Send + Sync + 'static,
{
    run_app_with_events_and_mouse(terminal, app, next_event, |_| Ok(()))
}

fn run_app_with_events_and_mouse<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut next_event: impl FnMut(Duration) -> io::Result<Option<Event>>,
    mut set_mouse_capture: impl FnMut(bool) -> io::Result<()>,
) -> Result<()>
where
    <B as ratatui::backend::Backend>::Error: std::error::Error + Send + Sync + 'static,
{
    let mut was_dialog_open = false;
    let mut needs_redraw = true;
    let mut shutdown = TuiShutdown::default();
    let mut last_redraw = Instant::now();
    let mut mouse_captured = false;

    loop {
        needs_redraw |= app.update();
        if cancellation::is_requested() {
            shutdown.request(app);
        }
        needs_redraw |= remote_redraw_due(&app.mode, last_redraw, Instant::now());
        if shutdown.ready(app) {
            if let Some(error) = shutdown.terminal_error.take() {
                return Err(error);
            }
            return match &app.mode {
                AppMode::Error(message) => Err(anyhow::anyhow!(message.clone())),
                _ => Ok(()),
            };
        }
        needs_redraw |= shutdown.refresh_status(app);
        if shutdown.terminal_error.is_some() {
            // A closed terminal must not terminate a worker that still owns the radio.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }

        let wants_mouse = app.mode == AppMode::Main(MainTab::Remote)
            && !app.dialog_open
            && !app.remote_screen.show_diagnostics;
        if wants_mouse != mouse_captured {
            if let Err(error) = set_mouse_capture(wants_mouse) {
                shutdown.terminal_failed(app, error.into())?;
                continue;
            }
            mouse_captured = wants_mouse;
        }

        let is_dialog_open = app.dialog_open;

        if (was_dialog_open || needs_redraw) && !is_dialog_open {
            if let Err(error) = terminal.draw(|f| ui::ui(f, app)) {
                shutdown.terminal_failed(app, error.into())?;
                continue;
            }
            last_redraw = Instant::now();
            needs_redraw = false;
        }

        was_dialog_open = is_dialog_open;

        let mut timeout = if app.remote_active {
            Duration::from_millis(20)
        } else if app.dialog_open {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(250)
        };
        if app.mode == AppMode::Main(MainTab::Remote) && !app.dialog_open {
            timeout = timeout.min(Duration::from_secs(1).saturating_sub(last_redraw.elapsed()));
        }

        let input = match next_event(timeout) {
            Ok(input) => input,
            Err(error) => {
                shutdown.terminal_failed(app, error.into())?;
                continue;
            }
        };
        if let Some(input) = input {
            if matches!(input, Event::Resize(..)) {
                needs_redraw = true;
                continue;
            }
            if let Event::Mouse(mouse) = input {
                if shutdown.requested {
                    continue;
                }
                if app.mode == AppMode::Main(MainTab::Remote)
                    && !app.remote_screen.show_diagnostics
                    && mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && remote_modifiers_allowed(mouse.modifiers)
                {
                    let size = match terminal.size() {
                        Ok(size) => size,
                        Err(error) => {
                            shutdown.terminal_failed(app, error.into())?;
                            continue;
                        }
                    };
                    if let Some(key) = ui::views::main_layout::remote_key_at_terminal(
                        ratatui::layout::Rect::new(0, 0, size.width, size.height),
                        mouse.column,
                        mouse.row,
                    ) {
                        app.send_key(key);
                        needs_redraw = true;
                    }
                }
                continue;
            }
            let Event::Key(key) = input else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }

            needs_redraw = true;
            if shutdown.handle_key(app, key) {
                continue;
            }

            if app.mode == AppMode::Main(MainTab::Remote) && !remote_key_event_allowed(key) {
                continue;
            }

            match &mut app.mode {
                AppMode::PortSelection => match key.code {
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
                                    KeyCode::Char('w') => app.start_write_scan_presets(),
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
                                    KeyCode::Char('w') => app.start_write_band_plans(),
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
                                    KeyCode::Char('D') => {
                                        app.remote_screen.show_diagnostics =
                                            !app.remote_screen.show_diagnostics;
                                    }
                                    KeyCode::Char('o') => app.remote_on(),
                                    KeyCode::Char('p') => app.remote_off(),
                                    KeyCode::Esc if !app.remote_active => app.leave_remote_tab(),
                                    _ if maybe_send_remote_key(app, key) => {}
                                    _ => {}
                                },
                                MainTab::Codeplug => match key.code {
                                    KeyCode::Char('r') => app.start_read_codeplug(),
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
                AppMode::Reading | AppMode::Writing | AppMode::BinFlashing => {}

                AppMode::Error(_) => match key.code {
                    KeyCode::Esc | KeyCode::Enter => app.mode = AppMode::PortSelection,
                    _ => {}
                },
            }
        }
    }
}

fn remote_redraw_due(mode: &AppMode, last_redraw: Instant, now: Instant) -> bool {
    *mode == AppMode::Main(MainTab::Remote)
        && now.saturating_duration_since(last_redraw) >= Duration::from_secs(1)
}

fn operation_in_progress(app: &App) -> bool {
    matches!(
        app.mode,
        AppMode::Reading | AppMode::Writing | AppMode::BinFlashing
    )
}

#[derive(Default)]
struct TuiShutdown {
    requested: bool,
    quit_after_remote_stop: bool,
    terminal_error: Option<anyhow::Error>,
}

impl TuiShutdown {
    fn terminal_failed(&mut self, app: &mut App, error: anyhow::Error) -> Result<()> {
        if !operation_in_progress(app) && !app.remote_active {
            return Err(error);
        }
        self.terminal_error = Some(error);
        self.request(app);
        Ok(())
    }

    fn handle_key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        if self.requested {
            return true;
        }

        let ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        let quit = match key.code {
            KeyCode::Char('q') => {
                operation_in_progress(app)
                    || matches!(app.mode, AppMode::PortSelection | AppMode::Main(_))
            }
            KeyCode::Esc => {
                operation_in_progress(app)
                    || matches!(app.mode, AppMode::PortSelection)
                    || matches!(app.mode, AppMode::Main(tab) if tab != MainTab::Remote)
            }
            _ => false,
        };
        if !ctrl_c && !quit {
            return false;
        }

        self.request(app);
        true
    }

    fn request(&mut self, app: &mut App) {
        if self.requested {
            return;
        }
        self.requested = true;
        if app.remote_active {
            app.remote_off();
            self.quit_after_remote_stop = true;
        }
        self.refresh_status(app);
    }

    fn ready(&self, app: &App) -> bool {
        // Busy modes are released by completion/error events, after device cleanup.
        self.requested
            && !operation_in_progress(app)
            && (!self.quit_after_remote_stop || !app.remote_active)
    }

    fn refresh_status(&self, app: &mut App) -> bool {
        if !self.requested || !operation_in_progress(app) {
            return false;
        }
        let status = match app.mode {
            AppMode::BinFlashing => {
                "Quit queued; flashing cannot be safely aborted. Keep the radio connected until it finishes."
            }
            AppMode::Writing => {
                "Quit queued; writing cannot be safely cancelled. Keep the radio connected until it finishes."
            }
            _ => {
                "Quit queued; waiting for read cleanup. Keep the radio connected until it finishes."
            }
        };
        if app.status_message == status {
            return false;
        }
        app.status_message = status.to_string();
        true
    }
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

fn remote_modifiers_allowed(modifiers: KeyModifiers) -> bool {
    (modifiers & !KeyModifiers::SHIFT).is_empty()
}

fn remote_key_event_allowed(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && remote_modifiers_allowed(key.modifiers)
}

fn remote_key_code(key: KeyEvent) -> Option<u8> {
    if !remote_key_event_allowed(key) {
        return None;
    }

    match key.code {
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
        KeyCode::Char('a') | KeyCode::Char(' ') => Some(0x13),
        KeyCode::Char('f') => Some(0x12),
        KeyCode::Char('v') | KeyCode::Char('b') => Some(0x11),
        _ => None,
    }
}

fn maybe_send_remote_key(app: &mut App, key: KeyEvent) -> bool {
    if !app.remote_active {
        return false;
    }

    if let Some(key) = remote_key_code(key) {
        app.send_key(key);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nictui::app::{AppEvent, WriteScope};
    use ratatui::backend::TestBackend;

    fn quit_keys() -> [KeyEvent; 3] {
        [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ]
    }

    #[test]
    fn remote_freshness_redraws_once_per_second_without_input() {
        let last = Instant::now();
        let remote = AppMode::Main(MainTab::Remote);
        assert!(!remote_redraw_due(
            &remote,
            last,
            last + Duration::from_millis(999)
        ));
        assert!(remote_redraw_due(
            &remote,
            last,
            last + Duration::from_secs(1)
        ));
        assert!(remote_redraw_due(
            &remote,
            last,
            last + Duration::from_secs(7)
        ));
        assert!(!remote_redraw_due(
            &AppMode::Main(MainTab::Channels),
            last,
            last + Duration::from_secs(7)
        ));
    }

    #[test]
    fn quit_keys_queue_without_releasing_in_flight_operations() {
        for mode in [AppMode::Reading, AppMode::Writing, AppMode::BinFlashing] {
            for key in quit_keys() {
                let mut app = App::new();
                app.mode = mode.clone();
                app.progress = 0.4;
                let mut shutdown = TuiShutdown::default();
                assert!(shutdown.handle_key(&mut app, key));
                assert_eq!(app.mode, mode);
                assert_eq!(app.progress, 0.4);
                assert!(!shutdown.ready(&app));
                assert!(app.status_message.starts_with("Quit queued;"));

                app.event_tx.send(AppEvent::Progress(1.0)).unwrap();
                app.event_tx
                    .send(AppEvent::Status("Device progress".to_string()))
                    .unwrap();
                app.update();
                assert!(
                    !shutdown.ready(&app),
                    "100% is not a cleanup acknowledgement"
                );
                assert!(shutdown.refresh_status(&mut app));
                assert!(app.status_message.starts_with("Quit queued;"));

                // Repeated quit and action keys must not cancel or start more work.
                for code in [
                    key.code,
                    KeyCode::Char('w'),
                    KeyCode::Char('f'),
                    KeyCode::Tab,
                ] {
                    assert!(shutdown.handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE)));
                    assert_eq!(app.mode, mode);
                    assert!(!shutdown.ready(&app));
                }
            }
        }
    }

    #[test]
    fn queued_quit_exits_event_loop_on_completion_or_error() {
        for key in quit_keys() {
            for (mode, completion) in [
                (AppMode::Reading, AppEvent::ReadPresetsComplete(Vec::new())),
                (
                    AppMode::Writing,
                    AppEvent::WriteComplete(WriteScope::Channels),
                ),
                (AppMode::BinFlashing, AppEvent::BinFlashComplete),
                (AppMode::Reading, AppEvent::Error("read failed".to_string())),
                (
                    AppMode::Writing,
                    AppEvent::Error("write failed".to_string()),
                ),
                (
                    AppMode::BinFlashing,
                    AppEvent::BinFlashFailed("flash failed".to_string()),
                ),
            ] {
                let failed = matches!(completion, AppEvent::Error(_) | AppEvent::BinFlashFailed(_));
                let mut app = App::new();
                app.mode = mode;
                app.channels_dirty = true;
                let tx = app.event_tx.clone();
                let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
                let mut completion = Some(completion);
                let mut polls = 0;
                let result = run_app_with_events(&mut terminal, &mut app, |_| {
                    polls += 1;
                    match polls {
                        1 => Ok(Some(Event::Key(key))),
                        2 => {
                            tx.send(AppEvent::Status("Still working".to_string()))
                                .unwrap();
                            Ok(Some(Event::Key(key)))
                        }
                        3 => {
                            tx.send(completion.take().unwrap()).unwrap();
                            Ok(None)
                        }
                        _ => panic!("queued quit did not exit after completion"),
                    }
                });
                assert_eq!(polls, 3);
                assert_eq!(result.is_err(), failed);
                assert!(!operation_in_progress(&app));
                if failed {
                    assert!(result.unwrap_err().to_string().contains("failed"));
                    assert!(app.channels_dirty, "failed writes must remain unsaved");
                }
                let buffer = terminal.backend().buffer();
                let rendered: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                assert!(rendered.contains("Quit queued;"));
            }
        }
    }

    #[test]
    fn quit_preserves_remote_stop_acknowledgement() {
        for key in quit_keys() {
            let mut app = App::new();
            app.mode = AppMode::Main(MainTab::Channels);
            app.remote_active = true;
            let mut shutdown = TuiShutdown::default();
            assert!(shutdown.handle_key(&mut app, key));
            assert!(shutdown.quit_after_remote_stop);
            assert!(
                app.remote_stop_signal
                    .load(std::sync::atomic::Ordering::SeqCst)
            );
            assert!(app.remote_active);
            assert!(!shutdown.ready(&app));
            app.event_tx
                .send(AppEvent::RemoteStopped {
                    message: "Remote stopped".to_string(),
                    failure: None,
                })
                .unwrap();
            app.update();
            assert!(shutdown.ready(&app));
        }
    }

    #[test]
    fn idle_quit_is_immediate_but_editor_and_remote_escape_keep_their_meanings() {
        for mode in [AppMode::PortSelection, AppMode::Main(MainTab::Channels)] {
            for key in quit_keys() {
                let mut app = App::new();
                app.mode = mode.clone();
                let mut shutdown = TuiShutdown::default();
                assert!(shutdown.handle_key(&mut app, key));
                assert!(shutdown.ready(&app));
            }
        }
        for mode in [
            AppMode::EditChannel(0),
            AppMode::Error("test".to_string()),
            AppMode::Main(MainTab::Remote),
        ] {
            let mut app = App::new();
            app.mode = mode;
            let mut shutdown = TuiShutdown::default();
            assert!(!shutdown.handle_key(&mut app, quit_keys()[1]));
            assert!(!shutdown.ready(&app));
        }
        let mut app = App::new();
        app.mode = AppMode::EditChannel(0);
        assert!(!TuiShutdown::default().handle_key(&mut app, quit_keys()[0]));
        assert!(TuiShutdown::default().handle_key(&mut app, quit_keys()[2]));
    }

    #[test]
    fn released_quit_key_does_not_exit_event_loop() {
        let mut app = App::new();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut polls = 0;
        run_app_with_events(&mut terminal, &mut app, |_| {
            polls += 1;
            assert!(polls <= 2);
            let mut key = quit_keys()[0];
            if polls == 1 {
                key.kind = KeyEventKind::Release;
            }
            Ok(Some(Event::Key(key)))
        })
        .unwrap();
        assert_eq!(polls, 2);
    }

    #[test]
    fn terminal_error_drains_worker_without_more_terminal_events() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };

        for (mode, completion) in [
            (AppMode::Reading, AppEvent::ReadPresetsComplete(Vec::new())),
            (
                AppMode::Writing,
                AppEvent::WriteComplete(WriteScope::Channels),
            ),
            (AppMode::BinFlashing, AppEvent::BinFlashComplete),
        ] {
            let mut app = App::new();
            app.mode = mode;
            let tx = app.event_tx.clone();
            let (failed, failure_rx) = mpsc::channel();
            let resumed = Arc::new(AtomicBool::new(false));
            let worker_resumed = Arc::clone(&resumed);
            let worker = std::thread::spawn(move || {
                let triggered = failure_rx.recv_timeout(Duration::from_secs(2)).is_ok();
                std::thread::sleep(Duration::from_millis(20));
                worker_resumed.store(triggered, Ordering::SeqCst);
                tx.send(completion).unwrap();
            });
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            let mut polls = 0;
            let result = run_app_with_events(&mut terminal, &mut app, |_| {
                polls += 1;
                assert_eq!(polls, 1, "failed terminal must never be polled again");
                failed.send(()).unwrap();
                Err(io::Error::other("terminal closed while busy"))
            });
            worker.join().unwrap();
            assert!(
                resumed.load(Ordering::SeqCst),
                "exit preceded worker cleanup"
            );
            assert!(!operation_in_progress(&app));
            assert_eq!(
                result.unwrap_err().to_string(),
                "terminal closed while busy"
            );
        }
    }

    #[test]
    fn idle_terminal_errors_are_not_hidden() {
        let mut app = App::new();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let result = run_app_with_events(&mut terminal, &mut app, |_| {
            Err(io::Error::other("idle terminal error"))
        });
        assert_eq!(result.unwrap_err().to_string(), "idle terminal error");
    }

    #[cfg(unix)]
    mod pty_shutdown {
        use super::*;
        use nictui::protocol::{BLOCK_SIZE, EEPROM_SIZE};
        use serialport::{SerialPort, TTYPort};
        use std::io::{Read, Write};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        use std::thread::{self, JoinHandle};

        struct Radio {
            slave: TTYPort,
            paused: mpsc::Receiver<()>,
            release: mpsc::Sender<()>,
            stop: Arc<AtomicBool>,
            worker: Option<JoinHandle<io::Result<Vec<u8>>>>,
        }

        impl Radio {
            fn new(fail_read: bool, fail_resume: bool) -> Self {
                let (mut master, slave) = TTYPort::pair().unwrap();
                master.set_timeout(Duration::from_millis(50)).unwrap();
                let (paused_tx, paused) = mpsc::channel();
                let (release, release_rx) = mpsc::channel();
                let stop = Arc::new(AtomicBool::new(false));
                let stopped = Arc::clone(&stop);
                let worker = thread::spawn(move || {
                    let mut memory = vec![0; EEPROM_SIZE];
                    memory[0x1900..0x1902].copy_from_slice(&0xD82Fu16.to_be_bytes());
                    memory[0x1A00..0x1A02].copy_from_slice(&0xA46Du16.to_be_bytes());
                    let mut trace = Vec::new();
                    let mut is_paused = false;
                    let mut resumes = 0;
                    while !stopped.load(Ordering::SeqCst) {
                        let mut command = [0];
                        match master.read_exact(&mut command) {
                            Ok(()) => {}
                            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                            Err(error) => return Err(error),
                        }
                        trace.push(command[0]);
                        let mut response = command.to_vec();
                        match command[0] {
                            0x01 => {}
                            0x45 => {
                                is_paused = true;
                                paused_tx.send(()).unwrap();
                                release_rx
                                    .recv_timeout(Duration::from_secs(5))
                                    .map_err(io::Error::other)?;
                            }
                            0x46 => {
                                resumes += 1;
                                if fail_resume && resumes == 1 {
                                    response[0] = 0;
                                } else {
                                    is_paused = false;
                                }
                            }
                            0x30 => {
                                let mut block = [0];
                                master.read_exact(&mut block)?;
                                let start = usize::from(block[0]) * BLOCK_SIZE;
                                let data = &memory[start..start + BLOCK_SIZE];
                                response.extend_from_slice(data);
                                let checksum = data.iter().fold(0u8, |sum, b| sum.wrapping_add(*b));
                                response.push(checksum ^ u8::from(fail_read && is_paused));
                            }
                            other => {
                                return Err(io::Error::other(format!(
                                    "unexpected radio command {other:02X}"
                                )));
                            }
                        }
                        master.write_all(&response)?;
                    }
                    Ok(trace)
                });
                Self {
                    slave,
                    paused,
                    release,
                    stop,
                    worker: Some(worker),
                }
            }

            fn finish(&mut self) -> Vec<u8> {
                self.stop.store(true, Ordering::SeqCst);
                self.worker.take().unwrap().join().unwrap().unwrap()
            }
        }

        impl Drop for Radio {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::SeqCst);
                let _ = self.release.send(());
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
            }
        }

        #[test]
        fn quit_during_paused_read_waits_for_resume_and_drop_retry() {
            for key in quit_keys() {
                for (fail_read, fail_resume) in [(false, false), (true, false), (false, true)] {
                    let mut radio = Radio::new(fail_read, fail_resume);
                    let mut app = App::new();
                    app.protocol_port_name = Some(radio.slave.name().unwrap());
                    app.mode = AppMode::Main(MainTab::Settings);
                    app.start_read_settings();
                    assert!(operation_in_progress(&app));
                    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
                    let started = Instant::now();
                    let mut polls = 0;
                    let result = run_app_with_events(&mut terminal, &mut app, |_| {
                        polls += 1;
                        assert!(
                            started.elapsed() < Duration::from_secs(10),
                            "shutdown stalled"
                        );
                        match polls {
                            1 => {
                                radio.paused.recv_timeout(Duration::from_secs(5)).unwrap();
                                Ok(Some(Event::Key(key)))
                            }
                            2 => {
                                // The queued-quit frame is drawn while the read is still paused.
                                radio.release.send(()).unwrap();
                                Ok(Some(Event::Key(key)))
                            }
                            _ => {
                                thread::sleep(Duration::from_millis(5));
                                Ok(None)
                            }
                        }
                    });
                    // Stop only the emulator after the event loop returns; never kill a process.
                    let trace = radio.finish();
                    assert!(polls >= 2);
                    assert_eq!(result.is_err(), fail_read || fail_resume, "{trace:?}");
                    assert!(!operation_in_progress(&app));
                    assert_eq!(trace.iter().filter(|&&c| c == 0x45).count(), 1);
                    assert_eq!(
                        trace.iter().filter(|&&c| c == 0x46).count(),
                        if fail_resume { 2 } else { 1 }
                    );
                    assert_eq!(trace.last(), Some(&0x46), "exit preceded RX resume");
                    let buffer = terminal.backend().buffer();
                    let rendered: String =
                        buffer.content.iter().map(|cell| cell.symbol()).collect();
                    assert!(rendered.contains("Quit queued;"));
                }
            }
        }
    }

    #[test]
    fn startup_ble_auto_scan_matches_platform_policy() {
        assert_eq!(should_auto_start_ble_scan(), !cfg!(target_os = "macos"));
    }

    #[test]
    fn writes_and_flashes_keep_the_ui_locked_until_completion() {
        for mode in [AppMode::Writing, AppMode::BinFlashing] {
            let mut app = App::demo();
            app.mode = mode.clone();
            let mut shutdown = TuiShutdown::default();
            for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('f')] {
                assert!(shutdown.handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE)));
                assert_eq!(app.mode, mode);
                assert!(!shutdown.ready(&app));
            }
            assert!(app.status_message.starts_with("Quit queued;"));
        }
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::BinFlash);
        assert!(!TuiShutdown::default().handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)
        ));
    }

    #[test]
    fn mouse_capture_is_limited_to_the_visible_remote_keypad() {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Channels);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut events = [
            KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        ]
        .into_iter()
        .map(Event::Key);
        let mut capture_changes = Vec::new();
        run_app_with_events_and_mouse(
            &mut terminal,
            &mut app,
            |_| Ok(Some(events.next().expect("UI must quit on q"))),
            |enabled| {
                capture_changes.push(enabled);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(capture_changes, [true, false, true, false]);
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
        assert!(maybe_send_remote_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)
        ));
    }

    #[test]
    fn remote_b_shortcuts_match_visible_keypad_label() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.demo_mode = true;
        app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Live;

        for shortcut in ['b', 'v'] {
            let key = KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::NONE);
            assert_eq!(remote_key_code(key), Some(0x11));
            assert!(maybe_send_remote_key(&mut app, key));
            assert_eq!(app.status_message, "Demo key: B");
        }
    }

    #[test]
    fn remote_modified_ptt_keys_do_not_activate() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.demo_mode = true;
        app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Live;
        app.status_message = "No key activated".to_string();

        for modifier in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
        ] {
            for modifiers in [modifier, modifier | KeyModifiers::SHIFT] {
                for code in [KeyCode::Char('a'), KeyCode::Char(' ')] {
                    let key = KeyEvent::new(code, modifiers);
                    assert!(!remote_key_event_allowed(key));
                    assert_eq!(remote_key_code(key), None);
                    assert!(!maybe_send_remote_key(&mut app, key));
                    assert_eq!(app.status_message, "No key activated");
                }
                for code in [KeyCode::Char('o'), KeyCode::Char('p')] {
                    assert!(!remote_key_event_allowed(KeyEvent::new(code, modifiers)));
                }
            }
        }
    }

    #[test]
    fn remote_repeat_keys_do_not_retrigger() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.demo_mode = true;
        app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Live;

        let press = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(maybe_send_remote_key(&mut app, press));
        assert_eq!(app.status_message, "Demo key: A/PTT");
        app.status_message = "No new activation".to_string();

        for code in [
            KeyCode::Char('a'),
            KeyCode::Char(' '),
            KeyCode::Char('1'),
            KeyCode::Up,
            KeyCode::Char('o'),
            KeyCode::Char('p'),
        ] {
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                let mut key = KeyEvent::new(code, KeyModifiers::NONE);
                key.kind = kind;
                assert!(!remote_key_event_allowed(key));
                assert_eq!(remote_key_code(key), None);
                assert!(!maybe_send_remote_key(&mut app, key));
                assert_eq!(app.status_message, "No new activation");
            }
        }
    }

    #[test]
    fn remote_plain_and_shifted_key_mappings_are_preserved() {
        for (code, expected) in [
            (KeyCode::Char('0'), 0x01),
            (KeyCode::Char('1'), 0x02),
            (KeyCode::Char('2'), 0x03),
            (KeyCode::Char('3'), 0x04),
            (KeyCode::Char('4'), 0x05),
            (KeyCode::Char('5'), 0x06),
            (KeyCode::Char('6'), 0x07),
            (KeyCode::Char('7'), 0x08),
            (KeyCode::Char('8'), 0x09),
            (KeyCode::Char('9'), 0x0A),
            (KeyCode::Enter, 0x0B),
            (KeyCode::Char('m'), 0x0B),
            (KeyCode::Up, 0x0C),
            (KeyCode::Char('u'), 0x0C),
            (KeyCode::Down, 0x0D),
            (KeyCode::Char('d'), 0x0D),
            (KeyCode::Esc, 0x0E),
            (KeyCode::Char('e'), 0x0E),
            (KeyCode::Char('*'), 0x0F),
            (KeyCode::Char('#'), 0x10),
            (KeyCode::Char('b'), 0x11),
            (KeyCode::Char('v'), 0x11),
            (KeyCode::Char('f'), 0x12),
            (KeyCode::Char('a'), 0x13),
            (KeyCode::Char(' '), 0x13),
        ] {
            for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
                let key = KeyEvent::new(code, modifiers);
                assert!(remote_key_event_allowed(key));
                assert_eq!(remote_key_code(key), Some(expected), "{key:?}");
            }
        }
    }

    #[test]
    fn remote_ctrl_c_still_quits_before_activation_guard() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.demo_mode = true;
        app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Live;
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut polls = 0;

        run_app_with_events(&mut terminal, &mut app, |_| {
            polls += 1;
            assert_eq!(polls, 1, "Ctrl+C must initiate shutdown immediately");
            Ok(Some(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            ))))
        })
        .unwrap();

        assert!(!app.remote_active);
        assert_eq!(polls, 1);
    }

    fn remote_route_frame(events: Vec<Event>) -> (App, String) {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.demo_mode = true;
        app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Live;
        app.status_message = "Remote route untouched".to_string();
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
        let mut events = events
            .into_iter()
            .chain(std::iter::once(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            ))));

        run_app_with_events(&mut terminal, &mut app, |_| {
            Ok(Some(events.next().expect("route must exit on Ctrl+C")))
        })
        .unwrap();

        // Shutdown exits before redrawing, leaving the last input's status visible.
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        (app, rendered)
    }

    #[test]
    fn remote_route_rejects_modified_and_repeated_actions() {
        for code in [KeyCode::Char('a'), KeyCode::Char('o'), KeyCode::Char('p')] {
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
            ] {
                let event = Event::Key(KeyEvent::new(code, modifiers));
                let (_, rendered) = remote_route_frame(vec![event]);
                assert!(
                    rendered.contains("Remote route untouched"),
                    "{code:?} {modifiers:?}"
                );
            }
            let mut repeat = KeyEvent::new(code, KeyModifiers::NONE);
            repeat.kind = KeyEventKind::Repeat;
            let (_, rendered) = remote_route_frame(vec![Event::Key(repeat)]);
            assert!(rendered.contains("Remote route untouched"), "{code:?}");
        }
    }

    #[test]
    fn remote_mouse_route_rejects_modified_clicks() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 50);
        let (column, row) = (0..area.height)
            .flat_map(|row| (0..area.width).map(move |column| (column, row)))
            .find(|&(column, row)| {
                ui::views::main_layout::remote_key_at_terminal(area, column, row) == Some(0x11)
            })
            .expect("B key must be visible");

        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyModifiers::NONE,
            KeyModifiers::SHIFT,
        ] {
            let (_, rendered) = remote_route_frame(vec![Event::Mouse(event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers,
            })]);
            let expected = if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT {
                "Demo key: B"
            } else {
                "Remote route untouched"
            };
            assert!(rendered.contains(expected), "{modifiers:?}");
        }
    }

    #[test]
    fn remote_shift_d_toggles_details_while_plain_d_sends_down() {
        let (app, rendered) = remote_route_frame(vec![Event::Key(KeyEvent::new(
            KeyCode::Char('D'),
            KeyModifiers::SHIFT,
        ))]);
        assert!(app.remote_screen.show_diagnostics);
        assert!(rendered.contains("Remote route untouched"));

        let (app, rendered) = remote_route_frame(vec![Event::Key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::NONE,
        ))]);
        assert!(!app.remote_screen.show_diagnostics);
        assert!(rendered.contains("Demo key: down"));
    }

    #[test]
    fn channel_navigation_repeats_are_preserved() {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Channels);
        app.demo_mode = true;
        app.channels = vec![nictui::protocol::Channel::default(); 3];
        app.channel_state.select(Some(0));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut events = [KeyCode::Down, KeyCode::Down, KeyCode::Up]
            .into_iter()
            .map(|code| {
                let mut key = KeyEvent::new(code, KeyModifiers::NONE);
                key.kind = KeyEventKind::Repeat;
                Event::Key(key)
            })
            .chain(std::iter::once(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            ))));

        run_app_with_events(&mut terminal, &mut app, |_| {
            Ok(Some(events.next().expect("route must exit on Ctrl+C")))
        })
        .unwrap();

        assert_eq!(app.channel_state.selected(), Some(1));
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
        assert!(!maybe_send_remote_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)
        ));
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
