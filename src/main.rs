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
        self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
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
            simulate,
            demo_view,
            simulator_http_endpoint,
        } => launch_tui(port, demo, simulate, demo_view, simulator_http_endpoint),
        Dispatch::Exit => Ok(()),
    }?;
    if cancellation::is_requested() {
        anyhow::bail!("Interrupted: signal requested; active operation cleanup finished");
    }
    Ok(())
}

#[derive(Default)]
struct TerminalSession {
    keyboard_enhanced: bool,
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.keyboard_enhanced {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableFocusChange,
            LeaveAlternateScreen,
            crossterm::cursor::Show,
            crossterm::style::ResetColor
        );
    }
}

fn launch_tui(
    port: Option<String>,
    demo: bool,
    simulate: bool,
    demo_view: DemoView,
    simulator_http_endpoint: Option<String>,
) -> Result<()> {
    let mut session = TerminalSession::default();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        DisableMouseCapture,
        EnterAlternateScreen,
        EnableFocusChange
    )?;
    let keyboard_release_supported = cfg!(windows)
        || query_keyboard_releases(
            std::env::var("TERM_PROGRAM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
            crossterm::terminal::supports_keyboard_enhancement,
        );
    if keyboard_release_supported && !cfg!(windows) {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        )?;
        session.keyboard_enhanced = true;
    }
    let backend = CrosstermBackend::new(ui::TerminalWriter::new(
        stdout,
        ui::TerminalPalette::detect(),
    ));
    let mut terminal = Terminal::new(backend)?;

    let mut app = if simulate {
        initialize_simulated_tui_app(
            port.as_deref().expect("simulator endpoint required"),
            demo_view,
            simulator_http_endpoint.as_deref(),
        )
    } else {
        initialize_tui_app(
            port.as_deref(),
            demo,
            demo_view,
            App::refresh_ports,
            |app| app.start_ble_scan(false),
        )
    };
    app.remote_keyboard_release_supported = keyboard_release_supported;
    if simulate && demo_view == DemoView::Remote {
        app.remote_on();
    }
    let res = run_app(&mut terminal, &mut app);

    drop(terminal);
    drop(session);
    res?;
    print_post_exit_skill_hint();

    Ok(())
}

fn query_keyboard_releases(
    term_program: Option<&str>,
    term: Option<&str>,
    query: impl FnOnce() -> io::Result<bool>,
) -> bool {
    // Apple Terminal has no key-up protocol; never infer a held PTT from repeats.
    if term_program == Some("Apple_Terminal") || term == Some("dumb") {
        return false;
    }
    query().unwrap_or(false)
}

fn initialize_simulated_tui_app(
    endpoint: &str,
    view: DemoView,
    simulator_http_endpoint: Option<&str>,
) -> App {
    let mut app = App::new();
    app.connect_to_port_by_name(endpoint);
    app.simulator_mode = true;
    app.simulator_http_endpoint = simulator_http_endpoint.map(str::to_owned);
    let tab = match view {
        DemoView::Channels => MainTab::Channels,
        DemoView::Settings => MainTab::Settings,
        DemoView::Remote => MainTab::Remote,
        DemoView::Activity => MainTab::Activity,
    };
    app.mode = AppMode::Main(tab);
    app.last_main_tab = tab;
    if tab != MainTab::Remote {
        app.last_non_remote_tab = tab;
    }
    app.status_message = "SIMULATOR | external local model, no RF hardware".into();
    app
}

fn initialize_tui_app(
    port: Option<&str>,
    demo: bool,
    demo_view: DemoView,
    discover_serial: impl FnOnce(&mut App),
    discover_ble: impl FnOnce(&mut App),
) -> App {
    let mut app = if demo { App::demo() } else { App::new() };
    if demo {
        // Demo mode is intentionally hardware-free so it is safe for screenshots and UI review.
        app.mode = AppMode::Main(match demo_view {
            DemoView::Channels => MainTab::Channels,
            DemoView::Settings => MainTab::Settings,
            DemoView::Activity => MainTab::Activity,
            DemoView::Remote => {
                app.remote_active = true;
                MainTab::Remote
            }
        });
        app.last_main_tab = match app.mode {
            AppMode::Main(tab) => tab,
            _ => MainTab::Channels,
        };
        if app.last_main_tab != MainTab::Remote {
            app.last_non_remote_tab = app.last_main_tab;
        }
    } else if let Some(port_name) = port {
        // An explicit target may already be scanning; discovery would interrupt it.
        app.connect_to_port_by_name(port_name);
    } else {
        discover_serial(&mut app);
        if should_auto_start_ble_scan() {
            discover_ble(&mut app);
        }
    }
    app
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
    let palette = ui::TerminalPalette::detect();
    let mut was_dialog_open = false;
    let mut needs_redraw = true;
    let mut shutdown = TuiShutdown::default();
    let mut last_redraw = Instant::now();
    let mut mouse_captured = false;
    let mut held_input = None;
    let mut keyboard_space_down = false;

    loop {
        needs_redraw |= app.update();
        app.release_remote_key_if_inactive();
        if app.held_remote_key().is_none() {
            held_input = None;
        }
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
            && !app.help.open
            && !app.remote_screen.show_diagnostics;
        if !wants_mouse && held_input == Some(RemoteHoldSource::Mouse) {
            release_remote_hold(app, &mut held_input);
            needs_redraw = true;
        }
        if wants_mouse != mouse_captured {
            if let Err(error) = set_mouse_capture(wants_mouse) {
                shutdown.terminal_failed(app, error.into())?;
                continue;
            }
            mouse_captured = wants_mouse;
        }

        let is_dialog_open = app.dialog_open;

        if (was_dialog_open || needs_redraw) && !is_dialog_open {
            if let Err(error) = terminal.draw(|f| {
                ui::ui(f, app);
                palette.apply(f.buffer_mut());
            }) {
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
                release_remote_hold(app, &mut held_input);
                needs_redraw = true;
                continue;
            }
            if matches!(input, Event::FocusLost) {
                release_remote_hold(app, &mut held_input);
                needs_redraw = true;
                continue;
            }
            if let Event::Mouse(mouse) = input {
                if shutdown.requested || app.help.open {
                    continue;
                }
                if held_input == Some(RemoteHoldSource::Mouse)
                    || app.mode == AppMode::Main(MainTab::Remote)
                {
                    let size = match terminal.size() {
                        Ok(size) => size,
                        Err(error) => {
                            shutdown.terminal_failed(app, error.into())?;
                            continue;
                        }
                    };
                    needs_redraw |= handle_remote_mouse(
                        app,
                        &mut held_input,
                        mouse,
                        ratatui::layout::Rect::new(0, 0, size.width, size.height),
                    );
                }
                continue;
            }
            let Event::Key(mut key) = input else {
                continue;
            };
            if key.code == KeyCode::Tab && key.modifiers == KeyModifiers::SHIFT {
                key.code = KeyCode::BackTab;
            }
            if key.kind == KeyEventKind::Release {
                if key.code == KeyCode::Char(' ') {
                    keyboard_space_down = false;
                    if held_input == Some(RemoteHoldSource::Keyboard) {
                        release_remote_hold(app, &mut held_input);
                        needs_redraw = true;
                    }
                }
                continue;
            }

            needs_redraw = true;
            let emergency_quit =
                key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL;
            if (shutdown.requested || !app.help.open || emergency_quit)
                && shutdown.handle_key(app, key)
            {
                continue;
            }

            // Track Space before overlays consume input; only key-up clears its latch.
            if handle_remote_keyboard_hold(app, &mut held_input, &mut keyboard_space_down, key) {
                continue;
            }

            if app.dialog_open {
                continue;
            }

            if app.help.open
                || key.code == KeyCode::F(1)
                || (key.code == KeyCode::Char('?')
                    && matches!(app.mode, AppMode::Main(_) | AppMode::PortSelection))
            {
                let size = match terminal.size() {
                    Ok(size) => size,
                    Err(error) => {
                        shutdown.terminal_failed(app, error.into())?;
                        continue;
                    }
                };
                let was_open = app.help.open;
                let rows =
                    ui::help_page_rows(ratatui::layout::Rect::new(0, 0, size.width, size.height));
                if app.handle_help_key(key, rows) {
                    if !was_open && app.help.open {
                        release_remote_hold(app, &mut held_input);
                    }
                    continue;
                }
            }

            if app.handle_read_all_key(key) {
                continue;
            }

            if !ui_key_event_allowed(&app.mode, key) {
                continue;
            }

            if app.mode == AppMode::Main(MainTab::Remote) && !remote_key_event_allowed(key) {
                continue;
            }

            if matches!(app.mode, AppMode::Main(_))
                && matches!(
                    key.code,
                    KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown
                )
            {
                let size = match terminal.size() {
                    Ok(size) => size,
                    Err(error) => {
                        shutdown.terminal_failed(app, error.into())?;
                        continue;
                    }
                };
                let rows = table_page_rows(app, size.width, size.height);
                if app.mode == AppMode::Main(MainTab::Debug) && app.handle_log_key(key.code, rows) {
                    continue;
                }
                if app.handle_table_navigation(key.code, rows) {
                    continue;
                }
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
                                tab_shortcut_for_key(current_tab, app.remote_active, key)
                            {
                                if current_tab == MainTab::Remote && target != MainTab::Remote {
                                    app.remote_off();
                                }
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
                                MainTab::Activity => {
                                    if activity_key_event_allowed(key) {
                                        app.handle_activity_key(key.code);
                                    }
                                }
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
                                    KeyCode::Char('r') => app.start_read_group_labels(),
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
                                MainTab::Debug => {
                                    let size = match terminal.size() {
                                        Ok(size) => size,
                                        Err(error) => {
                                            shutdown.terminal_failed(app, error.into())?;
                                            continue;
                                        }
                                    };
                                    let rows = table_page_rows(app, size.width, size.height);
                                    app.handle_log_key(key.code, rows);
                                }
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
                    KeyCode::Esc | KeyCode::Enter => {
                        app.mode = if app.simulator_mode || app.demo_mode {
                            AppMode::Main(app.last_main_tab)
                        } else {
                            AppMode::PortSelection
                        };
                    }
                    _ => {}
                },
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteHoldSource {
    Mouse,
    Keyboard,
}

fn release_remote_hold(app: &mut App, held: &mut Option<RemoteHoldSource>) {
    app.release_remote_key();
    *held = None;
}

fn handle_remote_keyboard_hold(
    app: &mut App,
    held: &mut Option<RemoteHoldSource>,
    space_down: &mut bool,
    key: KeyEvent,
) -> bool {
    if key.code != KeyCode::Char(' ') || !app.remote_keyboard_release_supported {
        return false;
    }
    // Windows can label auto-repeat as Press; cancellation must retain this latch
    // until key-up, independently of the radio's current hold/session state.
    let fresh_press = key.kind == KeyEventKind::Press && !*space_down;
    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        *space_down = true;
    }
    if app.mode != AppMode::Main(MainTab::Remote)
        || app.help.open
        || !app.remote_active
        || !remote_modifiers_allowed(key.modifiers)
    {
        return false;
    }
    if fresh_press && app.press_remote_key(0x13) {
        *held = Some(RemoteHoldSource::Keyboard);
    }
    true
}

fn handle_remote_mouse(
    app: &mut App,
    held: &mut Option<RemoteHoldSource>,
    mouse: MouseEvent,
    area: ratatui::layout::Rect,
) -> bool {
    if *held == Some(RemoteHoldSource::Mouse) {
        let outside_ptt =
            ui::views::main_layout::remote_key_at_terminal(area, mouse.column, mouse.row)
                != Some(0x13);
        if matches!(
            mouse.kind,
            MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Moved
        ) || (mouse.kind == MouseEventKind::Drag(MouseButton::Left) && outside_ptt)
        {
            release_remote_hold(app, held);
            return true;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            release_remote_hold(app, held);
        }
    }
    if app.mode != AppMode::Main(MainTab::Remote)
        || app.help.open
        || app.remote_screen.show_diagnostics
        || app.dialog_open
        || mouse.kind != MouseEventKind::Down(MouseButton::Left)
        || !remote_modifiers_allowed(mouse.modifiers)
    {
        return false;
    }
    let Some(key) = ui::views::main_layout::remote_key_at_terminal(area, mouse.column, mouse.row)
    else {
        return false;
    };
    if key == 0x13 {
        if app.press_remote_key(key) {
            *held = Some(RemoteHoldSource::Mouse);
        }
    } else {
        app.send_key(key);
    }
    true
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
        if !operation_in_progress(app) && !app.remote_active && !app.activity_active() {
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
        if key.kind != KeyEventKind::Press {
            return false;
        }

        let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL;
        let quit = !app.help.open
            && key.modifiers.is_empty()
            && match key.code {
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
        app.stop_activity();
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
            && !app.activity_active()
            && (!self.quit_after_remote_stop || !app.remote_active)
    }

    fn refresh_status(&self, app: &mut App) -> bool {
        if !self.requested || (!operation_in_progress(app) && !app.activity_active()) {
            return false;
        }
        let status = if app.activity_active() {
            "Quit queued; waiting for Activity capture cleanup."
        } else {
            match app.mode {
                AppMode::BinFlashing => {
                    "Quit queued; flashing cannot be safely aborted. Keep the radio connected until it finishes."
                }
                AppMode::Writing => {
                    "Quit queued; writing cannot be safely cancelled. Keep the radio connected until it finishes."
                }
                _ => {
                    "Quit queued; waiting for read cleanup. Keep the radio connected until it finishes."
                }
            }
        };
        if app.status_message == status {
            return false;
        }
        app.status_message = status.to_string();
        true
    }
}

fn ui_key_event_allowed(mode: &AppMode, key: KeyEvent) -> bool {
    if !(key.modifiers & !KeyModifiers::SHIFT).is_empty() {
        return false;
    }
    key.kind == KeyEventKind::Press
        || (key.kind == KeyEventKind::Repeat
            && (matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::PageUp
                    | KeyCode::PageDown
            ) || (matches!(
                mode,
                AppMode::EditChannel(_)
                    | AppMode::EditSetting(_)
                    | AppMode::EditDTMF(_)
                    | AppMode::EditScanPreset(_)
                    | AppMode::EditGroupLabel(_)
                    | AppMode::EditBandPlan(_)
            ) && matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace))))
}

fn table_page_rows(app: &App, width: u16, height: u16) -> usize {
    let content =
        ui::views::main_layout::main_content_area(ratatui::layout::Rect::new(0, 0, width, height));
    let content_height = content.height;
    let content_width = content.width;
    let narrow = content_width < 90;
    let reserved = match app.mode {
        AppMode::Main(MainTab::Debug) => return ui::log_page_rows(content),
        AppMode::Main(MainTab::Channels) => {
            (if content_width < 100 { 3 } else { 2 }) + if narrow { 5 } else { 6 }
        }
        AppMode::Main(MainTab::Scanning | MainTab::BandPlan) => {
            if narrow {
                9
            } else {
                8
            }
        }
        AppMode::Main(MainTab::Activity) => {
            let inner_width = content_width.saturating_sub(2);
            let inner_height = content_height.saturating_sub(2);
            let state = app.activity.lock().unwrap();
            let heartbeat_rows = (state.heartbeat_status(app.activity_active()).label().len()
                as u16)
                .div_ceil(inner_width.saturating_sub(2).max(1))
                .min(3)
                + 2;
            // Outer border, summary, heartbeat, setup, controls, and table header.
            return usize::from(
                inner_height
                    .saturating_sub(
                        2 * if inner_height >= 12 { 2 } else { 1 }
                            + heartbeat_rows
                            + if inner_width < 76 { 3 } else { 2 }
                            + 1,
                    )
                    .max(1),
            );
        }
        _ => {
            if narrow {
                9
            } else {
                7
            }
        }
    };
    // Main header/footer, section summary/details, table borders and header.
    usize::from(content_height.saturating_sub(reserved + 3).max(1))
}

fn tab_shortcut_target(
    current_tab: MainTab,
    remote_active: bool,
    code: KeyCode,
) -> Option<MainTab> {
    if matches!(code, KeyCode::Char('a' | 'A')) {
        return Some(MainTab::Activity);
    }
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

fn tab_shortcut_for_key(current: MainTab, remote_active: bool, key: KeyEvent) -> Option<MainTab> {
    if key.kind != KeyEventKind::Press || !(key.modifiers & !KeyModifiers::SHIFT).is_empty() {
        return None;
    }
    tab_shortcut_target(current, remote_active, key.code)
}

fn activity_key_event_allowed(key: KeyEvent) -> bool {
    (key.kind == KeyEventKind::Press
        || (key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Up | KeyCode::Down)))
        && (key.modifiers & !KeyModifiers::SHIFT).is_empty()
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
        KeyCode::Char(' ') => Some(0x13),
        KeyCode::Char('f') => Some(0x12),
        KeyCode::Char('v') | KeyCode::Char('b') => Some(0x11),
        _ => None,
    }
}

fn maybe_send_remote_key(app: &mut App, key: KeyEvent) -> bool {
    if !app.remote_active || app.help.open {
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
mod help_input_tests {
    use super::*;
    use nictui::app::logs::LogFilter;
    use nictui::app::{AppEvent, WriteScope};
    use nictui::protocol::RemotePacket;
    use nictui::remote::{RemoteControlCommand, RemoteSessionPhase};
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl_c() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
    }

    fn route(app: &mut App, inputs: Vec<Event>) -> (Vec<bool>, String) {
        route_sized(app, inputs, 120, 50)
    }

    fn route_sized(
        app: &mut App,
        inputs: Vec<Event>,
        width: u16,
        height: u16,
    ) -> (Vec<bool>, String) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut inputs = inputs.into_iter().chain([ctrl_c()]);
        let mut capture = Vec::new();
        run_app_with_events_and_mouse(
            &mut terminal,
            app,
            |_| Ok(Some(inputs.next().expect("help routing must terminate"))),
            |enabled| {
                capture.push(enabled);
                Ok(())
            },
        )
        .unwrap();
        assert!(inputs.next().is_none(), "help close must not quit the app");
        let frame = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        (capture, frame)
    }

    fn remote_app() -> App {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.remote_keyboard_release_supported = true;
        app.remote_screen.phase = RemoteSessionPhase::Live;
        app
    }

    #[test]
    fn help_overlay_intercepts_actions_on_all_eleven_tabs_and_port_selection() {
        let modes = [
            AppMode::Main(MainTab::Channels),
            AppMode::Main(MainTab::Settings),
            AppMode::Main(MainTab::Scanning),
            AppMode::Main(MainTab::MemoryGroups),
            AppMode::Main(MainTab::BandPlan),
            AppMode::Main(MainTab::DTMF),
            AppMode::Main(MainTab::Remote),
            AppMode::Main(MainTab::Codeplug),
            AppMode::Main(MainTab::BinFlash),
            AppMode::Main(MainTab::Debug),
            AppMode::Main(MainTab::Activity),
            AppMode::PortSelection,
        ];
        for mode in modes {
            for opener in [KeyCode::F(1), KeyCode::Char('?')] {
                let mut app = App::demo();
                app.mode = mode.clone();
                app.edit_buffer = "draft".into();
                app.codeplug_data = Some(vec![1, 2, 3]);
                app.settings_dirty = true;
                let channels = format!("{:?}", app.channels);
                let logs = app.logs.clone();
                let channel = app.channel_state.selected();
                let preset = app.preset_state.selected();
                route(
                    &mut app,
                    [
                        opener,
                        KeyCode::Tab,
                        KeyCode::Char('7'),
                        KeyCode::Char('n'),
                        KeyCode::Char('d'),
                        KeyCode::Char('c'),
                        KeyCode::Char('f'),
                        KeyCode::Char('w'),
                        KeyCode::Char('R'),
                        KeyCode::Enter,
                        KeyCode::Down,
                        KeyCode::PageDown,
                        KeyCode::Char('q'),
                        opener,
                        KeyCode::Esc,
                        opener,
                        opener,
                    ]
                    .into_iter()
                    .map(key)
                    .collect(),
                );
                assert_eq!(app.mode, mode, "{mode:?} {opener:?}");
                assert!(!app.help.open);
                assert_eq!(app.edit_buffer, "draft");
                assert_eq!(app.codeplug_data, Some(vec![1, 2, 3]));
                assert!(app.settings_dirty);
                assert_eq!(format!("{:?}", app.channels), channels);
                assert_eq!(app.logs, logs);
                assert_eq!(app.channel_state.selected(), channel);
                assert_eq!(app.preset_state.selected(), preset);
                assert!(!app.ble_scan_in_progress);
            }
        }
    }

    #[test]
    fn help_preserves_every_editor_draft_and_confirmation() {
        for mode in [
            AppMode::EditChannel(0),
            AppMode::EditSetting(0),
            AppMode::EditDTMF(0),
            AppMode::EditScanPreset(0),
            AppMode::EditGroupLabel(0),
            AppMode::EditBandPlan(0),
            AppMode::DeleteChannelConfirm(0),
        ] {
            let mut app = App::demo();
            app.mode = mode.clone();
            app.edit_buffer = "unsaved draft".into();
            app.pending_channel_edit = app.channels.first().cloned();
            app.pending_dtmf_edit = app.dtmf_presets.first().cloned();
            app.editing_scan_preset = app.scan_presets.first().cloned();
            app.editing_band_plan = app.band_plans.first().cloned();
            app.editing_group_label_idx = Some(0);
            let drafts = format!(
                "{:?}{:?}{:?}{:?}",
                app.pending_channel_edit,
                app.pending_dtmf_edit,
                app.editing_scan_preset,
                app.editing_band_plan
            );
            route(
                &mut app,
                [
                    KeyCode::F(1),
                    KeyCode::Char('?'),
                    KeyCode::Char('R'),
                    KeyCode::Char('x'),
                    KeyCode::Backspace,
                    KeyCode::Tab,
                    KeyCode::Enter,
                    KeyCode::Char('q'),
                    KeyCode::F(1),
                    KeyCode::Esc,
                ]
                .into_iter()
                .map(key)
                .collect(),
            );
            assert_eq!(app.mode, mode);
            assert_eq!(app.edit_buffer, "unsaved draft");
            assert_eq!(app.editing_group_label_idx, Some(0));
            assert_eq!(
                format!(
                    "{:?}{:?}{:?}{:?}",
                    app.pending_channel_edit,
                    app.pending_dtmf_edit,
                    app.editing_scan_preset,
                    app.editing_band_plan
                ),
                drafts
            );
            assert!(!app.channels_dirty && !app.dtmf_dirty && !app.settings_dirty);
        }
    }

    #[test]
    fn question_mark_remains_typed_text_outside_help() {
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let mut app = App::demo();
            app.start_edit_channel();
            app.edit_buffer = "draft".into();
            route(
                &mut app,
                vec![Event::Key(KeyEvent::new(KeyCode::Char('?'), modifiers))],
            );
            assert!(!app.help.open);
            assert_eq!(app.mode, AppMode::EditChannel(0));
            assert_eq!(app.edit_buffer, "draft?");

            let mut app = App::demo();
            route(
                &mut app,
                vec![Event::Key(KeyEvent::new(KeyCode::Char('?'), modifiers))],
            );
            assert!(app.help.open);
        }
    }

    #[test]
    fn q_and_escape_close_help_without_quitting_or_cancelling_an_editor() {
        for mode in [AppMode::Main(MainTab::Channels), AppMode::EditChannel(0)] {
            for close in [KeyCode::Char('q'), KeyCode::Esc] {
                let mut app = App::demo();
                app.mode = mode.clone();
                app.edit_buffer = "unsaved draft".into();
                route(
                    &mut app,
                    vec![key(KeyCode::F(1)), key(close), key(KeyCode::F(1))],
                );
                assert_eq!(app.mode, mode);
                assert_eq!(app.edit_buffer, "unsaved draft");
                assert!(
                    app.help.open,
                    "F1 after closing help must still be processed"
                );
            }
        }
    }

    #[test]
    fn ctrl_c_in_help_waits_for_cleanup_and_requested_shutdown_blocks_help_toggles() {
        for initially_open in [false, true] {
            let mut app = App::demo();
            app.mode = AppMode::Reading;
            app.help.open = initially_open;
            let tx = app.event_tx.clone();
            let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
            let mut polls = 0;
            run_app_with_events(&mut terminal, &mut app, |_| {
                polls += 1;
                Ok(Some(match polls {
                    1 => ctrl_c(),
                    2 => key(KeyCode::F(1)),
                    3 => {
                        tx.send(AppEvent::WriteComplete(WriteScope::None)).unwrap();
                        key(KeyCode::F(1))
                    }
                    _ => panic!("shutdown must wait for completion, then exit"),
                }))
            })
            .unwrap();
            assert_eq!(polls, 3);
            assert_eq!(app.help.open, initially_open);
        }
    }

    #[test]
    fn native_dialog_ignores_help_keys_even_when_overlay_was_open() {
        for open in [false, true] {
            let mut app = remote_app();
            app.dialog_open = true;
            app.help.open = open;
            app.help.scroll = 17;
            let (capture, _) = route(
                &mut app,
                vec![
                    key(KeyCode::F(1)),
                    key(KeyCode::Char('?')),
                    key(KeyCode::Down),
                ],
            );
            assert_eq!(app.help.open, open);
            assert_eq!(app.help.scroll, 17);
            assert!(capture.is_empty());
            assert!(app.dialog_open);
        }
    }

    #[test]
    fn busy_help_closes_without_quitting_and_completion_still_updates() {
        for mode in [AppMode::Reading, AppMode::Writing, AppMode::BinFlashing] {
            let mut app = App::demo();
            app.mode = mode;
            app.last_main_tab = MainTab::Settings;
            app.settings_dirty = true;
            let tx = app.event_tx.clone();
            let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
            let mut polls = 0;
            run_app_with_events(&mut terminal, &mut app, |_| {
                polls += 1;
                Ok(Some(match polls {
                    1 => key(KeyCode::F(1)),
                    2 => key(KeyCode::Char('q')),
                    3 => key(KeyCode::F(1)),
                    4 => key(KeyCode::Esc),
                    5 => key(KeyCode::F(1)),
                    6 => {
                        tx.send(AppEvent::Progress(0.75)).unwrap();
                        tx.send(AppEvent::WriteComplete(WriteScope::None)).unwrap();
                        key(KeyCode::Char('R'))
                    }
                    7 => ctrl_c(),
                    _ => panic!("busy help must not request shutdown"),
                }))
            })
            .unwrap();
            assert_eq!(polls, 7);
            assert!(app.help.open);
            assert_eq!(app.mode, AppMode::Main(MainTab::Settings));
            assert!(app.settings_dirty);
            assert_eq!(app.progress, 0.75);
            assert!(!app.status_message.contains("Quit queued"));
        }
    }

    #[test]
    fn f1_opens_over_errors_and_ctrl_c_remains_an_emergency_exit() {
        let mut app = App::demo();
        app.mode = AppMode::Error("fixture".into());
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
        let mut inputs = [key(KeyCode::F(1)), ctrl_c()].into_iter();
        let error = run_app_with_events(&mut terminal, &mut app, |_| {
            Ok(Some(
                inputs.next().expect("Ctrl+C must exit over error help"),
            ))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "fixture");
        assert!(app.help.open);
        assert_eq!(app.mode, AppMode::Error("fixture".into()));
    }

    #[test]
    fn help_toggle_rejects_modifiers_repeat_and_release() {
        let mut inputs = Vec::new();
        for code in [KeyCode::F(1), KeyCode::Char('?')] {
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
            ] {
                inputs.push(Event::Key(KeyEvent::new(code, modifiers)));
            }
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                let mut event = KeyEvent::new(code, KeyModifiers::NONE);
                event.kind = kind;
                inputs.push(Event::Key(event));
            }
        }
        let mut app = App::demo();
        route(&mut app, inputs.clone());
        assert!(!app.help.open);
        inputs.insert(0, key(KeyCode::F(1)));
        route(&mut app, inputs);
        assert!(app.help.open);
    }

    #[test]
    fn help_scrolls_rows_pages_and_endpoints_without_table_navigation() {
        let mut app = App::demo();
        route_sized(
            &mut app,
            vec![key(KeyCode::F(1)), key(KeyCode::End)],
            60,
            12,
        );
        let maximum = app.help.scroll;
        assert!(maximum > 0 && maximum < u16::MAX);
        let page = ui::help_page_rows(ratatui::layout::Rect::new(0, 0, 60, 12)) as u16;
        for (codes, expected) in [
            (vec![KeyCode::Down, KeyCode::Down, KeyCode::Up], 1),
            (
                vec![KeyCode::PageDown, KeyCode::PageDown, KeyCode::PageUp],
                (page * 2).min(maximum).saturating_sub(page),
            ),
            (vec![KeyCode::PageDown, KeyCode::Home, KeyCode::Up], 0),
            (vec![KeyCode::End, KeyCode::Down], maximum),
        ] {
            let mut app = App::demo();
            let selected = app.channel_state.selected();
            let mut inputs = vec![key(KeyCode::F(1))];
            inputs.extend(codes.into_iter().map(key));
            route_sized(&mut app, inputs, 60, 12);
            assert_eq!(app.help.scroll, expected);
            assert_eq!(app.channel_state.selected(), selected);
        }
    }

    #[test]
    fn help_controller_saturates_scroll_and_resets_on_reopen() {
        let mut app = App::demo();
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(app.handle_help_key(press(KeyCode::F(1)), 0));
        assert!(app.handle_help_key(press(KeyCode::PageDown), 0));
        assert_eq!(app.help.scroll, 1);
        app.handle_help_key(press(KeyCode::PageDown), usize::MAX);
        app.handle_help_key(press(KeyCode::Down), usize::MAX);
        assert_eq!(app.help.scroll, u16::MAX);
        app.handle_help_key(press(KeyCode::PageUp), usize::MAX);
        app.handle_help_key(press(KeyCode::Up), 0);
        assert_eq!(app.help.scroll, 0);
        app.handle_help_key(press(KeyCode::End), 0);
        app.handle_help_key(press(KeyCode::F(1)), 0);
        app.handle_help_key(press(KeyCode::F(1)), 0);
        assert!(app.help.open);
        assert_eq!(app.help.scroll, 0);
    }

    #[test]
    fn help_releases_keyboard_ptt_and_blocks_radio_keys() {
        let mut app = remote_app();
        let (capture, frame) = route(
            &mut app,
            vec![
                key(KeyCode::Char(' ')),
                key(KeyCode::F(1)),
                key(KeyCode::Char(' ')),
                key(KeyCode::Char('1')),
                key(KeyCode::Up),
                key(KeyCode::Char('d')),
                key(KeyCode::F(1)),
            ],
        );
        assert_eq!(capture, [true, false, true]);
        assert!(frame.contains("Demo key released"), "{frame}");
        assert_eq!(app.held_remote_key(), None);
    }

    fn space_event(kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
            kind,
        ))
    }

    #[test]
    fn help_cancelled_keyboard_ptt_requires_keyup_before_fresh_press() {
        for repeat in [KeyEventKind::Press, KeyEventKind::Repeat] {
            for rearm in [false, true] {
                let mut app = remote_app();
                let mut inputs = vec![
                    space_event(KeyEventKind::Press),
                    key(KeyCode::F(1)),
                    key(KeyCode::F(1)),
                    space_event(repeat),
                    space_event(repeat),
                ];
                if rearm {
                    inputs.extend([
                        space_event(KeyEventKind::Release),
                        space_event(KeyEventKind::Press),
                    ]);
                }
                let (_, frame) = route(&mut app, inputs);
                assert_eq!(frame.contains("Demo key held: A/PTT"), rearm, "{frame}");
                if !rearm {
                    assert!(frame.contains("Demo key released"), "{frame}");
                }
            }
        }
    }

    #[test]
    fn help_tracks_space_presses_and_releases_while_overlay_is_open() {
        for initially_held in [false, true] {
            for release_in_help in [false, true] {
                let mut app = remote_app();
                let mut inputs = Vec::new();
                if initially_held {
                    inputs.push(space_event(KeyEventKind::Press));
                }
                inputs.extend([key(KeyCode::F(1)), space_event(KeyEventKind::Press)]);
                if release_in_help {
                    inputs.push(space_event(KeyEventKind::Release));
                }
                inputs.extend([key(KeyCode::F(1)), space_event(KeyEventKind::Press)]);
                let (_, frame) = route(&mut app, inputs);
                assert_eq!(
                    frame.contains("Demo key held: A/PTT"),
                    release_in_help,
                    "initially held: {initially_held}; {frame}"
                );
            }
        }
    }

    #[test]
    fn help_does_not_change_legacy_keyboard_ptt_taps() {
        let mut app = remote_app();
        app.remote_keyboard_release_supported = false;
        let (_, frame) = route(
            &mut app,
            vec![
                space_event(KeyEventKind::Press),
                key(KeyCode::F(1)),
                space_event(KeyEventKind::Press),
                key(KeyCode::F(1)),
                space_event(KeyEventKind::Press),
            ],
        );
        assert!(frame.contains("Demo key: A/PTT"), "{frame}");
        assert!(!frame.contains("Demo key held:"), "{frame}");
        assert_eq!(app.held_remote_key(), None);
    }

    #[test]
    fn help_cancelled_keyboard_ptt_stays_disarmed_across_session_restart() {
        for release_in_help in [false, true] {
            let mut app = remote_app();
            let mut inputs = vec![space_event(KeyEventKind::Press), key(KeyCode::F(1))];
            if release_in_help {
                inputs.push(space_event(KeyEventKind::Release));
            }
            inputs.extend([
                key(KeyCode::F(1)),
                key(KeyCode::Char('p')),
                key(KeyCode::Char('o')),
                space_event(KeyEventKind::Press),
            ]);
            let (_, frame) = route(&mut app, inputs);
            assert_eq!(
                frame.contains("Demo key held: A/PTT"),
                release_in_help,
                "{frame}"
            );
            assert!(!app.remote_active);
            assert_eq!(app.held_remote_key(), None);
        }
    }

    #[test]
    fn help_releases_mouse_ptt_and_ignores_mouse_focus_resize_and_key_up() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 50);
        let (column, row) = (0..area.height)
            .flat_map(|row| (0..area.width).map(move |column| (column, row)))
            .find(|&(column, row)| {
                ui::views::main_layout::remote_key_at_terminal(area, column, row) == Some(0x13)
            })
            .unwrap();
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        let mut up = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        up.kind = KeyEventKind::Release;
        let mut app = remote_app();
        let (capture, frame) = route(
            &mut app,
            vec![
                down.clone(),
                key(KeyCode::Char('?')),
                down,
                Event::FocusLost,
                Event::FocusGained,
                Event::Resize(120, 50),
                Event::Key(up),
                key(KeyCode::Char('q')),
            ],
        );
        assert_eq!(capture, [true, false, true]);
        assert!(frame.contains("Demo key released"), "{frame}");
        assert_eq!(app.held_remote_key(), None);
    }

    #[test]
    fn opening_help_sends_only_release_to_a_mock_remote_command_channel() {
        let mut app = remote_app();
        assert!(app.press_remote_key(0x13));
        app.demo_mode = false;
        let (tx, rx) = std::sync::mpsc::channel();
        app.remote_tx = Some(tx);
        let events = app.event_tx.clone();
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
        let mut polls = 0;
        let released = std::cell::Cell::new(false);
        run_app_with_events_and_mouse(
            &mut terminal,
            &mut app,
            |_| {
                polls += 1;
                Ok(Some(match polls {
                    1 => key(KeyCode::F(1)),
                    2 => {
                        assert!(released.get(), "release must precede the help redraw");
                        key(KeyCode::Down)
                    }
                    3 => key(KeyCode::Char(' ')),
                    4 => {
                        assert!(rx.try_recv().is_err(), "help must not inject radio input");
                        events
                            .send(AppEvent::RemoteStopped {
                                message: "mock stopped".into(),
                                failure: None,
                            })
                            .unwrap();
                        ctrl_c()
                    }
                    _ => panic!("mock session must stop"),
                }))
            },
            |enabled| {
                if !enabled {
                    // Capture changes precede terminal.draw in the actual loop.
                    let release = rx
                        .try_recv()
                        .expect("release before disabling capture/draw");
                    let expected = RemoteControlCommand::release_key();
                    assert_eq!(release.label, expected.label);
                    assert_eq!(release.steps.len(), 1);
                    assert_eq!(release.steps[0].bytes, expected.steps[0].bytes);
                    assert_eq!(release.steps[0].pause_after, Duration::ZERO);
                    released.set(true);
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(polls, 4);
        assert_eq!(app.held_remote_key(), None);
        assert!(app.help.open);
    }

    #[test]
    fn remote_telemetry_keeps_updating_behind_help() {
        let mut app = remote_app();
        let tx = app.event_tx.clone();
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
        let received = Instant::now();
        let mut polls = 0;
        run_app_with_events(&mut terminal, &mut app, |_| {
            polls += 1;
            Ok(Some(match polls {
                1 => key(KeyCode::F(1)),
                2 => {
                    tx.send(AppEvent::RemoteReceived(received)).unwrap();
                    tx.send(AppEvent::RemotePacket(RemotePacket::SignalStrength {
                        strength: 73,
                        mode: 0,
                        battery: 0,
                    }))
                    .unwrap();
                    key(KeyCode::Char('d'))
                }
                3 => ctrl_c(),
                _ => panic!("telemetry test must exit"),
            }))
        })
        .unwrap();
        assert!(app.help.open);
        assert_eq!(app.remote_screen.signal_strength, 73);
        assert!(app.remote_screen.last_received.unwrap() >= received);
    }

    fn log_app() -> App {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Debug);
        app.logs = (0..200).map(|i| format!("RX: fixture {i}")).collect();
        app
    }

    #[test]
    fn logs_receive_row_page_and_endpoint_keys_with_the_rendered_page_size() {
        for (width, height) in [(80, 24), (120, 36), (220, 80)] {
            let area = ui::main_content_area(ratatui::layout::Rect::new(0, 0, width, height));
            let page = ui::log_page_rows(area);
            for (codes, scroll, follow) in [
                (vec![KeyCode::Down, KeyCode::Down, KeyCode::Up], 1, false),
                (vec![KeyCode::PageDown], page, false),
                (
                    vec![KeyCode::PageDown, KeyCode::PageDown, KeyCode::PageUp],
                    page,
                    false,
                ),
                (vec![KeyCode::Down, KeyCode::Home], 0, false),
                (vec![KeyCode::PageDown, KeyCode::End], 0, true),
            ] {
                let mut app = log_app();
                route_sized(
                    &mut app,
                    codes.into_iter().map(key).collect(),
                    width,
                    height,
                );
                assert_eq!(app.log_view.scroll, scroll, "{width}x{height}");
                assert_eq!(app.log_view.follow, follow);
            }
        }
    }

    #[test]
    fn logs_receive_clear_filter_and_repeated_rows_but_not_modified_actions() {
        let mut app = log_app();
        let mut repeated = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        repeated.kind = KeyEventKind::Repeat;
        route(&mut app, vec![Event::Key(repeated), Event::Key(repeated)]);
        assert_eq!(app.log_view.scroll, 2);
        assert!(!app.log_view.follow);

        let mut app = log_app();
        route(&mut app, vec![key(KeyCode::Char('f'))]);
        assert_eq!(app.log_view.filter, LogFilter::Rx);
        assert_eq!(app.logs.len(), 200);
        let mut app = log_app();
        route(&mut app, vec![key(KeyCode::Char('c'))]);
        assert!(app.logs.is_empty());
        assert!(app.log_view.follow);
        assert_eq!(app.log_view.scroll, 0);

        let mut app = log_app();
        route(
            &mut app,
            vec![
                Event::Key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT)),
                Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER)),
            ],
        );
        assert_eq!(app.log_view.filter, LogFilter::All);
        assert_eq!(app.logs.len(), 200);
    }

    #[test]
    fn help_blocks_all_log_actions_without_changing_the_log_anchor() {
        let mut app = log_app();
        route(
            &mut app,
            vec![
                key(KeyCode::Down),
                key(KeyCode::F(1)),
                key(KeyCode::Down),
                key(KeyCode::PageDown),
                key(KeyCode::Home),
                key(KeyCode::End),
                key(KeyCode::Char('c')),
                key(KeyCode::Char('f')),
                key(KeyCode::Char('q')),
            ],
        );
        assert_eq!(app.log_view.scroll, 1);
        assert!(!app.log_view.follow);
        assert_eq!(app.log_view.filter, LogFilter::All);
        assert_eq!(app.logs.len(), 200);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nictui::app::{AppEvent, WriteScope};
    use ratatui::backend::TestBackend;

    fn route_keys(app: &mut App, keys: impl IntoIterator<Item = KeyEvent>) {
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        let mut events = keys
            .into_iter()
            .chain([KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)]);
        run_app_with_events(&mut terminal, app, |_| {
            Ok(Some(Event::Key(
                events.next().expect("event loop must quit"),
            )))
        })
        .unwrap();
    }

    #[test]
    fn modified_and_repeated_actions_do_not_edit_navigate_or_quit() {
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('d'),
            KeyCode::Char('q'),
            KeyCode::Char('a'),
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Enter,
            KeyCode::Esc,
        ] {
            let mut keys = Vec::new();
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ] {
                keys.push(KeyEvent::new(code, modifiers));
            }
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                let mut key = KeyEvent::new(code, KeyModifiers::NONE);
                key.kind = kind;
                keys.push(key);
            }
            for key in keys {
                let mut app = App::demo();
                app.mode = AppMode::Main(MainTab::Channels);
                let count = app.channels.len();
                route_keys(&mut app, [key]);
                assert_eq!(app.mode, AppMode::Main(MainTab::Channels), "{key:?}");
                assert_eq!(app.channels.len(), count, "{key:?}");
                assert!(!app.channels_dirty, "{key:?}");
            }
        }
    }

    #[test]
    fn editor_repeats_type_but_do_not_save_or_leak_modified_text() {
        let mut app = App::demo();
        app.start_edit_channel();
        app.edit_buffer = "X".into();
        let mut repeated_text = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        repeated_text.kind = KeyEventKind::Repeat;
        let mut repeated_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        repeated_enter.kind = KeyEventKind::Repeat;
        route_keys(
            &mut app,
            [
                repeated_text,
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT),
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
                repeated_enter,
                KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            ],
        );
        assert_eq!(app.mode, AppMode::EditChannel(0));
        assert_eq!(app.edit_buffer, "XaR");
        assert!(!app.channels_dirty);
    }

    #[test]
    fn suspended_dialog_ignores_view_shortcuts_and_workspace_actions() {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Channels);
        app.dialog_open = true;
        app.settings = None;
        let count = app.channels.len();
        route_keys(
            &mut app,
            [
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
            ],
        );
        assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
        assert_eq!(app.channels.len(), count);
        assert!(app.settings.is_none());
        assert!(!app.channels_dirty);
    }

    #[test]
    fn shift_tab_and_repeated_rows_route_without_repeated_tab_switches() {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Settings);
        app.channel_state.select(Some(0));
        let mut repeat = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        route_keys(
            &mut app,
            [
                KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
                repeat,
                repeat,
            ],
        );
        assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
        assert_eq!(app.channel_state.selected(), Some(2));
    }

    #[test]
    fn home_end_and_pages_use_the_rendered_table_viewport() {
        for (width, height, rows) in [(80, 24, 8), (120, 36, 19), (160, 48, 32), (220, 80, 64)] {
            let mut app = App::demo();
            assert_eq!(table_page_rows(&app, width, height), rows);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut keys = [
                KeyCode::End,
                KeyCode::Home,
                KeyCode::PageDown,
                KeyCode::Char('q'),
            ]
            .into_iter();
            run_app_with_events(&mut terminal, &mut app, |_| {
                Ok(Some(Event::Key(KeyEvent::new(
                    keys.next().unwrap(),
                    KeyModifiers::NONE,
                ))))
            })
            .unwrap();
            assert_eq!(
                app.channel_state.selected(),
                Some(rows.min(app.channels.len() - 1))
            );
        }
    }

    #[test]
    fn local_error_dismissal_restores_tab_without_entering_device_selection() {
        for simulator in [false, true] {
            let mut app = App::demo();
            app.simulator_mode = simulator;
            app.demo_mode = !simulator;
            app.last_main_tab = MainTab::DTMF;
            app.mode = AppMode::Error("local fixture error".into());
            route_keys(
                &mut app,
                [KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
            );
            assert_eq!(app.mode, AppMode::Main(MainTab::DTMF));
            assert!(!app.ble_scan_in_progress);
        }
    }

    fn run_read_all_keys(app: &mut App, keys: impl IntoIterator<Item = KeyEvent>) {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut keys = keys
            .into_iter()
            .chain([KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)]);
        run_app_with_events(&mut terminal, app, |_| {
            std::thread::sleep(Duration::from_millis(1));
            Ok(keys.next().map(Event::Key))
        })
        .unwrap();
    }

    #[test]
    fn read_all_shortcuts_route_through_actual_event_loop_on_every_main_tab() {
        let keys = [
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::SHIFT),
        ];
        for tab in [
            MainTab::Channels,
            MainTab::Settings,
            MainTab::Scanning,
            MainTab::MemoryGroups,
            MainTab::BandPlan,
            MainTab::DTMF,
            MainTab::Remote,
            MainTab::Codeplug,
            MainTab::BinFlash,
            MainTab::Debug,
            MainTab::Activity,
        ] {
            for key in keys {
                let mut app = App::demo();
                app.mode = AppMode::Main(tab);
                app.channels.clear();
                app.settings = None;
                app.scan_presets.clear();
                app.dtmf_presets.clear();
                app.band_plans.clear();
                app.group_labels.fill(String::new());
                run_read_all_keys(&mut app, [key]);
                assert_eq!(app.mode, AppMode::Main(tab), "{tab:?} {key:?}");
                assert!(app.codeplug_snapshot_is_current(), "{tab:?} {key:?}");
                assert!(!app.channels.is_empty());
                assert!(app.settings.is_some());
                assert!(!app.scan_presets.is_empty());
                assert!(!app.dtmf_presets.is_empty());
                assert!(!app.band_plans.is_empty());
                assert!(app.group_labels.iter().any(|label| !label.is_empty()));
            }
        }
    }

    #[test]
    fn read_all_event_loop_rejects_wrong_chords_repeat_release_and_keeps_lowercase_r() {
        let mut keys = vec![
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::SUPER),
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::ALT | KeyModifiers::SHIFT),
            KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        ];
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            for modifiers in [KeyModifiers::CONTROL, KeyModifiers::SHIFT] {
                let mut key = KeyEvent::new(KeyCode::Char('r'), modifiers);
                key.kind = kind;
                keys.push(key);
            }
        }
        for key in keys {
            let mut app = App::demo();
            app.settings = None;
            app.mode = AppMode::Main(MainTab::Settings);
            run_read_all_keys(&mut app, [key]);
            assert!(app.settings.is_none(), "unexpected full read: {key:?}");
            if key.code == KeyCode::Char('r') && key.modifiers.is_empty() {
                assert!(app.status_message.contains("hardware I/O disabled"));
            }
        }
    }

    #[test]
    fn read_all_event_loop_preserves_dirty_edits_and_remote_key_ownership() {
        let mut app = App::demo();
        app.settings = None;
        app.group_labels_dirty = true;
        run_read_all_keys(
            &mut app,
            [KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)],
        );
        assert!(app.settings.is_none());
        assert!(app.group_labels_dirty);
        assert!(app.status_message.contains("unsaved edits"));

        let mut app = App::demo();
        app.settings = None;
        app.remote_active = true;
        app.mode = AppMode::Main(MainTab::Remote);
        run_read_all_keys(
            &mut app,
            [KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)],
        );
        assert!(app.settings.is_none());
        assert!(!app.remote_active);
    }

    #[test]
    fn read_all_queued_shutdown_waits_for_atomic_completion() {
        for key in quit_keys() {
            let mut app = App::demo();
            app.settings = None;
            app.start_read_all();
            let mut shutdown = TuiShutdown::default();
            assert!(shutdown.handle_key(&mut app, key));
            assert!(!shutdown.ready(&app));
            assert_eq!(app.mode, AppMode::Reading);
            assert!(app.settings.is_none());
            let deadline = Instant::now() + Duration::from_secs(3);
            while !shutdown.ready(&app) && Instant::now() < deadline {
                app.update();
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(shutdown.ready(&app));
            assert!(app.settings.is_some());
            assert!(app.codeplug_snapshot_is_current());
        }
    }

    #[test]
    fn startup_explicit_port_selects_passively_without_discovery() {
        let port = "/dev/nictui-nonexistent-TEST";
        let app = initialize_tui_app(
            Some(port),
            false,
            DemoView::Channels,
            |_| panic!("explicit startup must not enumerate or probe serial ports"),
            |_| panic!("explicit startup must not discover BLE radios"),
        );
        assert_eq!(app.mode, AppMode::Main(MainTab::Channels));
        assert_eq!(app.protocol_port_name.as_deref(), Some(port));
        assert_eq!(app.ports, [port]);
        assert!(app.simulator_http_endpoint.is_none());
        assert_eq!(app.port_candidates.len(), 1);
        assert!(app.serial_port_candidates.is_empty());
        assert!(app.ble_port_candidates.is_empty());
        assert!(!app.ble_scan_in_progress);
        assert!(!app.activity_active());
        assert!(!app.remote_active);
        let candidate = app.selected_port_candidate().unwrap();
        assert_eq!(candidate.kind, nictui::device::PortKind::Candidate);
        assert!(!candidate.handshake_ok);
        assert!(candidate.firmware_variant.is_none());
        assert!(app.status_message.contains("Radio opens on first action."));
    }

    #[test]
    fn startup_bare_selection_keeps_discovery_order_and_platform_policy() {
        let calls = std::cell::RefCell::new(Vec::new());
        let app = initialize_tui_app(
            None,
            false,
            DemoView::Channels,
            |app| {
                assert_eq!(app.mode, AppMode::PortSelection);
                calls.borrow_mut().push("serial");
            },
            |app| {
                assert_eq!(app.mode, AppMode::PortSelection);
                calls.borrow_mut().push("ble");
            },
        );
        let expected = if should_auto_start_ble_scan() {
            vec!["serial", "ble"]
        } else {
            vec!["serial"]
        };
        assert_eq!(*calls.borrow(), expected);
        assert_eq!(app.mode, AppMode::PortSelection);
        assert!(app.protocol_port_name.is_none());
    }

    #[test]
    fn simulator_startup_isolated_from_discovery_and_can_open_each_view() {
        for view in [
            DemoView::Channels,
            DemoView::Settings,
            DemoView::Remote,
            DemoView::Activity,
        ] {
            let app = initialize_simulated_tui_app("sim://127.0.0.1:8788", view, None);
            assert!(!app.demo_mode);
            assert!(app.simulator_mode);
            assert_eq!(
                app.protocol_port_name.as_deref(),
                Some("sim://127.0.0.1:8788")
            );
            assert!(app.serial_port_candidates.is_empty());
            assert!(app.ble_port_candidates.is_empty());
            assert!(!app.ble_scan_in_progress);
            assert!(app.remote_tx.is_none());
            assert!(!app.remote_active);
            assert!(app.simulator_http_endpoint.is_none());
        }
    }

    #[test]
    fn simulator_startup_keeps_optional_http_pair_without_connecting() {
        let uart = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let target = format!("sim://{}", uart.local_addr().unwrap());
        let endpoint = format!("http://{}", http.local_addr().unwrap());
        for view in [
            DemoView::Channels,
            DemoView::Settings,
            DemoView::Remote,
            DemoView::Activity,
        ] {
            let app = initialize_simulated_tui_app(&target, view, Some(&endpoint));
            assert!(app.simulator_mode);
            assert!(!app.demo_mode);
            assert_eq!(app.protocol_port_name.as_deref(), Some(target.as_str()));
            assert_eq!(
                app.simulator_http_endpoint.as_deref(),
                Some(endpoint.as_str())
            );
            assert!(!app.remote_active);
            assert!(app.remote_tx.is_none());
            assert!(app.serial_port_candidates.is_empty());
            assert!(app.ble_port_candidates.is_empty());
            assert!(!app.ble_scan_in_progress);
        }
        for listener in [&uart, &http] {
            listener.set_nonblocking(true).unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn simulator_error_dismissal_keeps_the_view_and_local_target_for_retry() {
        for code in [KeyCode::Esc, KeyCode::Enter] {
            let mut app =
                initialize_simulated_tui_app("sim://127.0.0.1:8788", DemoView::Settings, None);
            app.mode = AppMode::Error("Simulator connection interrupted".into());
            run_read_all_keys(&mut app, [KeyEvent::new(code, KeyModifiers::NONE)]);
            assert_eq!(app.mode, AppMode::Main(MainTab::Settings));
            assert_eq!(
                app.protocol_port_name.as_deref(),
                Some("sim://127.0.0.1:8788")
            );
            assert!(
                app.port_candidates
                    .iter()
                    .all(|candidate| candidate.port_name.starts_with("sim://"))
            );
            assert!(!app.ble_scan_in_progress);
        }
    }

    #[test]
    fn simulator_session_never_falls_back_to_a_physical_target() {
        let mut app = initialize_simulated_tui_app("sim://127.0.0.1:8788", DemoView::Remote, None);
        app.refresh_ports();
        app.start_ble_scan(true);
        assert!(app.serial_port_candidates.is_empty());
        assert!(app.ble_port_candidates.is_empty());
        assert!(!app.ble_scan_in_progress);
        app.protocol_port_name = Some("ble://must-never-connect".into());
        app.remote_on();
        assert!(!app.remote_active);
        assert!(app.remote_tx.is_none());
        assert!(app.status_message.contains("Invalid simulator target"));
        app.start_read_all();
        assert_eq!(app.mode, AppMode::Main(MainTab::Remote));
        assert!(app.status_message.contains("Invalid simulator target"));
    }

    #[test]
    fn startup_demo_ignores_explicit_port_and_never_discovers_hardware() {
        for view in [
            DemoView::Channels,
            DemoView::Settings,
            DemoView::Activity,
            DemoView::Remote,
        ] {
            let app = initialize_tui_app(
                Some("/dev/nictui-nonexistent-TEST"),
                true,
                view,
                |_| panic!("demo must not enumerate or probe serial ports"),
                |_| panic!("demo must not discover BLE radios"),
            );
            assert!(app.demo_mode);
            assert!(app.simulator_http_endpoint.is_none());
            let expected = match view {
                DemoView::Settings => MainTab::Settings,
                DemoView::Activity => MainTab::Activity,
                DemoView::Remote => MainTab::Remote,
                _ => MainTab::Channels,
            };
            assert_eq!(app.mode, AppMode::Main(expected));
            assert_eq!(app.last_main_tab, expected);
            assert_eq!(
                app.last_non_remote_tab,
                if expected == MainTab::Remote {
                    MainTab::Channels
                } else {
                    expected
                }
            );
            assert_eq!(app.remote_active, expected == MainTab::Remote);
            assert_ne!(
                app.protocol_port_name.as_deref(),
                Some("/dev/nictui-nonexistent-TEST")
            );
        }
    }

    #[test]
    fn activity_shortcut_is_case_insensitive_without_a_ptt_collision() {
        for character in ['a', 'A'] {
            for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
                let key = KeyEvent::new(KeyCode::Char(character), modifiers);
                for tab in [MainTab::Channels, MainTab::Activity, MainTab::Remote] {
                    for active in [false, true] {
                        assert_eq!(
                            tab_shortcut_for_key(tab, active, key),
                            Some(MainTab::Activity)
                        );
                    }
                }
            }
        }
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ] {
            assert_eq!(
                tab_shortcut_for_key(
                    MainTab::Channels,
                    false,
                    KeyEvent::new(KeyCode::Char('A'), modifiers)
                ),
                None
            );
        }
        assert_eq!(
            tab_shortcut_for_key(
                MainTab::Remote,
                true,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
            ),
            Some(MainTab::Activity)
        );
        assert_eq!(
            remote_key_code(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            remote_key_code(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            Some(0x13)
        );
        for (key, tab) in [
            ('1', MainTab::Channels),
            ('2', MainTab::Settings),
            ('3', MainTab::Scanning),
            ('4', MainTab::MemoryGroups),
            ('5', MainTab::BandPlan),
            ('6', MainTab::DTMF),
            ('7', MainTab::Remote),
            ('8', MainTab::Codeplug),
            ('9', MainTab::BinFlash),
            ('0', MainTab::Debug),
        ] {
            assert_eq!(
                tab_shortcut_target(MainTab::Activity, false, KeyCode::Char(key)),
                Some(tab)
            );
        }
    }

    #[test]
    fn activity_start_stop_actions_are_press_only_but_rows_repeat() {
        for code in [
            KeyCode::Char('s'),
            KeyCode::Char('c'),
            KeyCode::Char('e'),
            KeyCode::Enter,
        ] {
            let mut key = KeyEvent::new(code, KeyModifiers::NONE);
            assert!(activity_key_event_allowed(key));
            key.kind = KeyEventKind::Repeat;
            assert!(!activity_key_event_allowed(key));
            key.kind = KeyEventKind::Release;
            assert!(!activity_key_event_allowed(key));
        }
        let mut key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        key.kind = KeyEventKind::Repeat;
        assert!(activity_key_event_allowed(key));
        key.modifiers = KeyModifiers::ALT;
        assert!(!activity_key_event_allowed(key));
    }

    #[test]
    fn quit_and_terminal_failure_wait_for_activity_worker_completion() {
        for terminal_failed in [false, true] {
            let mut app = App::demo();
            app.mode = AppMode::Main(MainTab::Activity);
            app.start_activity();
            let mut shutdown = TuiShutdown::default();
            if terminal_failed {
                shutdown
                    .terminal_failed(&mut app, anyhow::anyhow!("closed terminal"))
                    .unwrap();
            } else {
                assert!(shutdown.handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
                ));
            }
            assert!(!shutdown.ready(&app));
            assert!(app.activity_active());
            assert!(shutdown.handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE)
            ));
            let deadline = Instant::now() + Duration::from_secs(2);
            while !shutdown.ready(&app) && Instant::now() < deadline {
                app.update();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(shutdown.ready(&app));
            assert!(!app.activity_active());
        }
    }

    #[test]
    fn activity_keys_navigate_and_capture_in_hardware_free_event_loop() {
        let mut app = App::demo();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut events = [
            KeyCode::Char('A'),
            KeyCode::Char('s'),
            KeyCode::Down,
            KeyCode::Char('s'),
            KeyCode::Char('q'),
        ]
        .into_iter();
        run_app_with_events(&mut terminal, &mut app, |_| {
            Ok(events
                .next()
                .map(|code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE))))
        })
        .unwrap();
        assert_eq!(app.mode, AppMode::Main(MainTab::Activity));
        assert!(!app.activity_active());
        assert_eq!(app.activity_state.selected(), Some(0));
    }

    #[test]
    fn lowercase_activity_shortcut_stops_remote_without_sending_a_key() {
        for character in ['a', 'A'] {
            let mut app = App::demo();
            app.mode = AppMode::Main(MainTab::Remote);
            app.remote_active = true;
            app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Live;
            app.logs.clear();
            let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
            let mut events = [KeyCode::Char(character), KeyCode::Char('q')].into_iter();
            run_app_with_events(&mut terminal, &mut app, |_| {
                Ok(events
                    .next()
                    .map(|code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE))))
            })
            .unwrap();
            assert_eq!(app.mode, AppMode::Main(MainTab::Activity));
            assert_eq!(app.last_main_tab, MainTab::Activity);
            assert!(!app.remote_active);
            assert!(!app.activity_active());
            assert!(!app.logs.iter().any(|entry| entry.contains("PTT")));
        }
    }

    #[test]
    fn repeated_or_modified_activity_shortcuts_do_not_switch_views() {
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            let mut key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
            key.kind = kind;
            assert_eq!(tab_shortcut_for_key(MainTab::Remote, true, key), None);
        }
        for modifier in [
            KeyModifiers::ALT,
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
        ] {
            assert_eq!(
                tab_shortcut_for_key(
                    MainTab::Remote,
                    true,
                    KeyEvent::new(KeyCode::Char('a'), modifier)
                ),
                None
            );
        }
    }

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

        let press = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
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
        remote_route_frame_with_keyboard(events, false)
    }

    fn remote_route_frame_with_keyboard(
        events: Vec<Event>,
        key_up_supported: bool,
    ) -> (App, String) {
        let mut app = App::new();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.demo_mode = true;
        app.remote_keyboard_release_supported = key_up_supported;
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

    fn remote_ptt_position(area: ratatui::layout::Rect) -> (u16, u16) {
        (0..area.height)
            .flat_map(|row| (0..area.width).map(move |column| (column, row)))
            .find(|&(column, row)| {
                ui::views::main_layout::remote_key_at_terminal(area, column, row) == Some(0x13)
            })
            .expect("PTT must be visible")
    }

    fn demo_remote_for_holds() -> App {
        let mut app = App::demo();
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Live;
        app
    }

    #[test]
    fn mouse_ptt_stays_held_until_release_even_outside_the_button() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 50);
        let (column, row) = remote_ptt_position(area);
        let mut app = demo_remote_for_holds();
        let mut held = None;
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_remote_mouse(&mut app, &mut held, down, area));
        assert_eq!(held, Some(RemoteHoldSource::Mouse));
        assert_eq!(app.held_remote_key(), Some(0x13));
        assert!(!handle_remote_mouse(
            &mut app,
            &mut held,
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                ..down
            },
            area
        ));
        assert_eq!(app.held_remote_key(), Some(0x13));
        assert!(handle_remote_mouse(
            &mut app,
            &mut held,
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::CONTROL | KeyModifiers::ALT,
            },
            area
        ));
        assert_eq!(app.held_remote_key(), None);
        assert_eq!(held, None);
    }

    #[test]
    fn dragging_outside_ptt_or_moving_without_a_button_releases_it() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 50);
        let (column, row) = remote_ptt_position(area);
        for kind in [
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
        ] {
            let mut app = demo_remote_for_holds();
            let mut held = None;
            handle_remote_mouse(
                &mut app,
                &mut held,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            assert!(handle_remote_mouse(
                &mut app,
                &mut held,
                MouseEvent {
                    kind,
                    column: 0,
                    row: 0,
                    modifiers: KeyModifiers::NONE,
                },
                area
            ));
            assert_eq!(app.held_remote_key(), None);
        }
    }

    #[test]
    fn focus_loss_resize_and_mouse_up_release_before_another_radio_action() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 50);
        let (column, row) = remote_ptt_position(area);
        for release in [
            Event::FocusLost,
            Event::Resize(120, 50),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::ALT,
            }),
        ] {
            let (_, rendered) = remote_route_frame(vec![
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                }),
                release,
                Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            ]);
            assert!(rendered.contains("Demo key: down"), "{rendered}");
        }
    }

    #[test]
    fn only_key_up_capable_terminals_hold_keyboard_ptt() {
        let press = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let mut app = demo_remote_for_holds();
        let mut held = None;
        let mut space_down = false;
        assert!(!handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            press
        ));
        assert!(maybe_send_remote_key(&mut app, press));
        assert_eq!(app.held_remote_key(), None);
        app.remote_keyboard_release_supported = true;
        assert!(handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            press
        ));
        assert_eq!(held, Some(RemoteHoldSource::Keyboard));
        assert_eq!(app.held_remote_key(), Some(0x13));
        let mut repeat = press;
        repeat.kind = KeyEventKind::Repeat;
        assert!(handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            repeat
        ));
        assert_eq!(app.held_remote_key(), Some(0x13));
        release_remote_hold(&mut app, &mut held);
        assert_eq!(app.held_remote_key(), None);
        assert!(handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            repeat
        ));
        assert_eq!(
            app.held_remote_key(),
            None,
            "repeat must not rearm a released hold"
        );
    }

    #[test]
    fn cancelled_keyboard_hold_cannot_restart_on_repeated_press_without_keyup() {
        let press = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        for cancel in [Event::FocusLost, Event::Resize(120, 50)] {
            let (_, rendered) = remote_route_frame_with_keyboard(
                vec![Event::Key(press), cancel, Event::Key(press)],
                true,
            );
            assert!(!rendered.contains("Demo key held:"), "{rendered}");
        }
    }

    #[test]
    fn keyboard_hold_requires_fresh_press_after_recovery_or_tab_return() {
        let press = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let mut app = demo_remote_for_holds();
        app.remote_keyboard_release_supported = true;
        let mut held = None;
        let mut space_down = false;
        assert!(handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            press
        ));
        release_remote_hold(&mut app, &mut held);
        app.remote_active = false;
        app.mode = AppMode::Main(MainTab::Activity);
        assert!(!handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            press
        ));
        app.mode = AppMode::Main(MainTab::Remote);
        app.remote_active = true;
        assert!(handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            press
        ));
        assert_eq!(app.held_remote_key(), None);
        space_down = false;
        assert!(handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            press
        ));
        assert_eq!(app.held_remote_key(), Some(0x13));
    }

    #[test]
    fn enhanced_keyboard_release_is_handled_before_release_events_are_ignored() {
        let press = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let mut repeat = press;
        repeat.kind = KeyEventKind::Repeat;
        let mut release = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
        release.kind = KeyEventKind::Release;
        let (_, rendered) = remote_route_frame_with_keyboard(
            vec![
                Event::Key(press),
                Event::Key(repeat),
                Event::Key(release),
                Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            ],
            true,
        );
        assert!(rendered.contains("Demo key: down"), "{rendered}");
    }

    #[test]
    fn mouse_release_does_not_cancel_a_keyboard_owned_hold() {
        let mut app = demo_remote_for_holds();
        app.remote_keyboard_release_supported = true;
        let mut held = None;
        let mut space_down = false;
        handle_remote_keyboard_hold(
            &mut app,
            &mut held,
            &mut space_down,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert!(!handle_remote_mouse(
            &mut app,
            &mut held,
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            ratatui::layout::Rect::new(0, 0, 120, 50)
        ));
        assert_eq!(app.held_remote_key(), Some(0x13));
        release_remote_hold(&mut app, &mut held);
    }

    #[test]
    fn apple_terminal_and_dumb_terminals_do_not_enable_keyboard_holds() {
        for (program, term) in [
            (Some("Apple_Terminal"), Some("xterm-256color")),
            (None, Some("dumb")),
        ] {
            assert!(!query_keyboard_releases(program, term, || panic!(
                "unsupported terminal queried"
            )));
        }
        assert!(query_keyboard_releases(None, Some("xterm-kitty"), || Ok(
            true
        )));
        assert!(!query_keyboard_releases(
            None,
            Some("xterm-256color"),
            || Ok(false)
        ));
        assert!(!query_keyboard_releases(
            None,
            Some("xterm-256color"),
            || Err(io::Error::other("no reply"))
        ));
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
