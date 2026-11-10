use super::*;

fn ready_app() -> (App, mpsc::Receiver<RemoteControlCommand>) {
    let mut app = App::new();
    app.mode = AppMode::Main(MainTab::Remote);
    app.remote_active = true;
    app.remote_screen.phase = RemoteSessionPhase::Live;
    let (tx, rx) = mpsc::channel();
    app.remote_tx = Some(tx);
    update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Live);
    (app, rx)
}

#[test]
fn remote_hold_acceptance_and_repeats_are_explicit() {
    let (mut app, rx) = ready_app();
    assert!(app.press_remote_key(0x13));
    assert_eq!(app.held_remote_key(), Some(0x13));
    assert!(!app.remote_controls_ready());
    assert!(!app.press_remote_key(0x13));
    assert!(!app.press_remote_key(0x0B));
    app.send_key(0x13);
    let command = rx.try_recv().unwrap();
    assert_eq!(command.bytes_hex(), "90");
    assert_eq!(command.estimated_duration(), std::time::Duration::ZERO);
    assert!(rx.try_recv().is_err());
}

#[test]
fn remote_hold_release_bypasses_pending_readiness_and_mode_gates() {
    let (mut app, rx) = ready_app();
    assert!(app.press_remote_key(0x13));
    assert!(app.remote_input_state.lock().unwrap().pending);
    rx.try_recv().unwrap();
    app.remote_input_state.lock().unwrap().ready = false;
    app.mode = AppMode::Main(MainTab::Channels);
    app.dialog_open = true;
    app.remote_screen.phase = RemoteSessionPhase::Recovering;
    app.release_remote_key();
    assert_eq!(rx.try_recv().unwrap().bytes_hex(), "FF");
    assert_eq!(app.held_remote_key(), None);
    app.release_remote_key();
    assert!(rx.try_recv().is_err());
}

#[test]
fn remote_hold_blur_dialog_and_shutdown_release_without_new_press() {
    for reason in 0..4 {
        let (mut app, rx) = ready_app();
        assert!(app.press_remote_key(0x13));
        rx.try_recv().unwrap();
        match reason {
            0 => app.mode = AppMode::Main(MainTab::Channels),
            1 => app.dialog_open = true,
            2 => app.remote_stop_signal.store(true, Ordering::SeqCst),
            _ => app.remote_active = false,
        }
        app.release_remote_key_if_inactive();
        assert_eq!(rx.try_recv().unwrap().bytes_hex(), "FF");
        assert_eq!(app.held_remote_key(), None);
        assert!(!app.press_remote_key(0x13));
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn remote_hold_recovery_discards_edges_and_never_resumes_hold() {
    let (mut app, rx) = ready_app();
    assert!(app.press_remote_key(0x13));
    app.release_remote_key();
    update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Recovering);
    assert_eq!(app.held_remote_key(), None);
    assert!(!app.remote_input_state.lock().unwrap().pending);
    app.release_remote_key();
    assert!(!app.press_remote_key(0x13));
    update_remote_input_phase(&app.remote_input_state, &rx, RemoteSessionPhase::Armed);
    assert_eq!(app.held_remote_key(), None);
    assert!(rx.try_recv().is_err());
}

#[test]
fn remote_hold_stop_releases_before_dropping_sender() {
    let (mut app, rx) = ready_app();
    assert!(app.press_remote_key(0x13));
    app.remote_off();
    assert_eq!(app.held_remote_key(), None);
    assert!(app.remote_stop_signal.load(Ordering::SeqCst));
    assert!(app.remote_tx.is_none());
    assert_eq!(rx.try_recv().unwrap().bytes_hex(), "90");
    assert_eq!(rx.try_recv().unwrap().bytes_hex(), "FF");
    assert!(rx.try_recv().is_err());
}

#[test]
fn remote_hold_legacy_tap_is_bounded_and_does_not_claim_a_hold() {
    let (mut app, rx) = ready_app();
    assert!(!app.remote_keyboard_release_supported);
    app.send_key(0x13);
    assert_eq!(app.held_remote_key(), None);
    let command = rx.try_recv().unwrap();
    assert_eq!(command.bytes_hex(), "90 FF");
    assert_eq!(
        command.steps[0].pause_after,
        std::time::Duration::from_millis(400)
    );
    assert!(app.status_message.contains("tap (400ms)"));
    assert!(app.status_message.contains("mouse hold"));
}

#[test]
fn remote_hold_demo_has_no_transport_and_releases_on_stop() {
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Remote);
    app.remote_on();
    assert!(app.press_remote_key(0x13));
    assert!(!app.press_remote_key(0x13));
    assert_eq!(app.held_remote_key(), Some(0x13));
    assert!(app.remote_tx.is_none());
    assert!(app.event_rx.try_recv().is_err());
    app.remote_off();
    assert_eq!(app.held_remote_key(), None);
    assert!(!app.remote_active);
}

#[test]
fn remote_hold_failed_queue_does_not_claim_acceptance() {
    let (mut app, rx) = ready_app();
    drop(rx);
    assert!(!app.press_remote_key(0x13));
    assert_eq!(app.held_remote_key(), None);
    assert!(app.remote_stop_signal.load(Ordering::SeqCst));
}
