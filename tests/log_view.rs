use crossterm::event::KeyCode;
use nictui::{
    app::{
        App, AppMode, MainTab,
        logs::{LogFilter, classify_log},
    },
    ui::views::debug::{log_page_rows, render_debug_logs},
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect, style::Color};

// Seeded demo state and in-memory rendering only; never start discovery or workers.
fn seeded(count: usize) -> App {
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Debug);
    app.logs = (0..count)
        .map(|n| format!("[12:00:00] RX: entry-{n:03}"))
        .collect();
    app
}

fn render(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| render_debug_logs(frame, app, frame.area()))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text(buffer: &Buffer) -> String {
    buffer
        .content
        .chunks(usize::from(buffer.area.width).max(1))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn body(buffer: &Buffer) -> String {
    text(buffer)
        .lines()
        .skip(6)
        .take(usize::from(buffer.area.height.saturating_sub(7)))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn newest_first_navigation_and_explicit_live_state() {
    let mut app = seeded(30);
    let buffer = render(&mut app, 80, 16);
    assert!(text(&buffer).contains("LIVE | Filter: All | 30/30 entries"));
    assert!(body(&buffer).find("entry-029").unwrap() < body(&buffer).find("entry-028").unwrap());
    let page = log_page_rows(Rect::new(0, 0, 80, 16));
    assert_eq!(page, 9);
    assert!(app.handle_log_key(KeyCode::Down, page));
    assert_eq!(app.log_view.scroll, 1);
    assert!(!app.log_view.follow);
    assert!(app.handle_log_key(KeyCode::PageDown, page));
    assert_eq!(app.log_view.scroll, 10);
    assert!(body(&render(&mut app, 80, 16)).contains("entry-019"));
    app.handle_log_key(KeyCode::PageUp, page);
    assert_eq!(app.log_view.scroll, 1);
    app.handle_log_key(KeyCode::Up, page);
    assert_eq!(app.log_view.scroll, 0);
    assert!(!app.log_view.follow);
    app.handle_log_key(KeyCode::Home, page);
    assert_eq!(app.log_view.scroll, 0);
    assert!(!app.log_view.follow);
    assert!(text(&render(&mut app, 80, 16)).contains("PAUSED"));
    app.handle_log_key(KeyCode::End, page);
    assert_eq!(app.log_view.scroll, 0);
    assert!(app.log_view.follow);
    app.handle_log_key(KeyCode::PageDown, usize::MAX);
    assert_eq!(app.log_view.scroll, 29);
    app.handle_log_key(KeyCode::PageUp, usize::MAX);
    assert_eq!(app.log_view.scroll, 0);
    app.handle_log_key(KeyCode::PageDown, 0);
    assert_eq!(app.log_view.scroll, 1);
}

#[test]
fn paused_viewport_survives_matching_and_nonmatching_appends_and_eviction() {
    let mut app = seeded(100);
    app.log_view.filter = LogFilter::Rx;
    render(&mut app, 80, 16);
    app.handle_log_key(KeyCode::PageDown, 9);
    let before = body(&render(&mut app, 80, 16));
    let offset = app.log_view.scroll;
    app.log("TX: not in this filter");
    assert_eq!(app.log_view.scroll, offset);
    assert_eq!(body(&render(&mut app, 80, 16)), before);
    app.log("RX: incoming");
    assert_eq!(app.log_view.scroll, offset + 1);
    assert_eq!(body(&render(&mut app, 80, 16)), before);
    assert_eq!(app.logs.len(), 100);
    for _ in 0..110 {
        app.log("RX: replacement");
    }
    let buffer = render(&mut app, 80, 16);
    assert!(body(&buffer).contains("replacement"));
    assert!(app.log_view.scroll < app.log_view.rows(&app.logs).len());
    assert!(!app.log_view.follow);
    app.handle_log_key(KeyCode::End, 9);
    app.log("RX: latest-live");
    assert_eq!(app.log_view.scroll, 0);
    assert!(body(&render(&mut app, 80, 16)).contains("latest-live"));
}

#[test]
fn home_pauses_at_newest_without_implicitly_resuming_on_append() {
    let mut app = seeded(12);
    render(&mut app, 80, 16);
    app.handle_log_key(KeyCode::Home, 9);
    let before = body(&render(&mut app, 80, 16));
    app.log("RX: new while paused");
    assert_eq!(app.log_view.scroll, 1);
    assert!(!app.log_view.follow);
    assert_eq!(body(&render(&mut app, 80, 16)), before);
    app.handle_log_key(KeyCode::End, 9);
    assert!(body(&render(&mut app, 80, 16)).contains("new while paused"));
    assert!(app.log_view.follow);
}

#[test]
fn long_entries_have_accessible_continuations_and_keep_anchor_on_append() {
    let mut app = seeded(0);
    app.logs.push_back(format!("RX: {}TAIL", "x".repeat(180)));
    render(&mut app, 40, 12);
    let rows = app.log_view.rows(&app.logs);
    assert!(rows.len() > 3);
    app.handle_log_key(KeyCode::PageDown, 3);
    let before = body(&render(&mut app, 40, 12));
    assert!(before.contains("TAIL"));
    let offset = app.log_view.scroll;
    app.log(&format!("RX: {}", "z".repeat(90)));
    assert!(app.log_view.scroll > offset + 1);
    assert_eq!(body(&render(&mut app, 40, 12)), before);
}

#[test]
fn filter_cycle_counts_empty_results_and_clear_are_local_only() {
    let mut app = seeded(0);
    for message in [
        "RX: [01]",
        "TX: [02]",
        "REMOTE TXT (1,2) MENU",
        "REMOTE CTRL 1",
        "Remote key Menu",
        "Remote command failed",
        "Unrelated",
    ] {
        app.logs.push_back(format!("[12:00:00] {message}"));
    }
    let rendered = text(&render(&mut app, 90, 18));
    assert!(rendered.contains("RX 1 | TX 1 | Remote 4 | Errors 1"));
    assert!(rendered.contains("RX/TX are data, not RF"));
    let channels = serde_json::to_value(&app.channels).unwrap();
    let remote_active = app.remote_active;
    for filter in [
        LogFilter::Rx,
        LogFilter::Tx,
        LogFilter::Remote,
        LogFilter::Errors,
        LogFilter::All,
    ] {
        app.handle_log_key(KeyCode::Char('f'), 11);
        assert_eq!(app.log_view.filter, filter);
        assert!(app.log_view.follow);
        assert_eq!(app.log_view.scroll, 0);
        let expected = app.logs.iter().filter(|line| filter.matches(line)).count();
        assert!(text(&render(&mut app, 90, 18)).contains(&format!("{expected}/7 entries")));
    }
    app.logs.retain(|line| !classify_log(line).tx);
    app.log_view.filter = LogFilter::Tx;
    assert!(
        text(&render(&mut app, 90, 18)).contains("No TX entries match. Press f to change filter.")
    );
    app.handle_log_key(KeyCode::Home, 11);
    app.handle_log_key(KeyCode::Char('f'), 11);
    assert!(!app.log_view.follow);
    app.handle_log_key(KeyCode::Char('c'), 11);
    assert!(app.logs.is_empty());
    assert!(app.log_view.follow);
    assert_eq!(app.log_view.scroll, 0);
    assert_eq!(serde_json::to_value(&app.channels).unwrap(), channels);
    assert_eq!(app.remote_active, remote_active);
    assert!(app.status_message.contains("Local log history cleared"));
    assert!(text(&render(&mut app, 90, 18)).contains("Local history is empty"));
}

#[test]
fn classifiers_anchor_to_message_and_remote_does_not_imply_control_evidence() {
    for line in [
        "REMOTE: status",
        "[12:00:00] REMOTE TXT RX: menu",
        "[12:00:00] REMOTE CTRL 0",
        "Remote Menu [raw-key] no control evidence",
    ] {
        assert!(LogFilter::Remote.matches(line), "{line}");
        assert!(!LogFilter::Rx.matches(line), "{line}");
        assert!(!LogFilter::Tx.matches(line), "{line}");
    }
    assert!(!LogFilter::Remote.matches("Info mentions REMOTE: in a payload"));
    assert!(!LogFilter::Remote.matches("Remoteish unrelated"));
    for line in [
        "ERROR: disconnected",
        "Read failed",
        "Remote session failure",
        "Request timed out",
        "timeout",
        "fatal",
    ] {
        assert!(LogFilter::Errors.matches(line), "{line}");
    }
    assert!(!LogFilter::Errors.matches("Remote key Menu; not confirmed"));
}

#[test]
fn preserve_colors_and_render_tiny_or_narrow_areas_without_panics() {
    let mut app = seeded(0);
    for line in ["RX: one", "TX: two", "REMOTE TXT three"] {
        app.logs.push_back(line.into());
    }
    let buffer = render(&mut app, 80, 16);
    assert_eq!(buffer[(1, 6)].fg, Color::Rgb(245, 182, 72));
    assert_eq!(buffer[(1, 7)].fg, Color::Rgb(134, 162, 255));
    assert_eq!(buffer[(1, 8)].fg, Color::Rgb(102, 214, 193));
    for (width, height) in [(1, 1), (2, 2), (20, 8), (60, 20), (190, 80)] {
        render(&mut app, width, height);
    }
}

#[test]
fn unrelated_keys_dialogs_help_and_other_tabs_are_not_consumed() {
    let mut app = seeded(4);
    for key in [
        KeyCode::Char('q'),
        KeyCode::F(1),
        KeyCode::Char('?'),
        KeyCode::Enter,
    ] {
        assert!(!app.handle_log_key(key, 4));
    }
    app.help.open = true;
    assert!(!app.handle_log_key(KeyCode::Char('c'), 4));
    app.help.open = false;
    app.dialog_open = true;
    assert!(!app.handle_log_key(KeyCode::Char('c'), 4));
    app.dialog_open = false;
    app.mode = AppMode::Main(MainTab::Channels);
    assert!(!app.handle_log_key(KeyCode::Char('c'), 4));
    assert_eq!(app.logs.len(), 4);
}
