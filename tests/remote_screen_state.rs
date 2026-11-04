use std::time::{Duration, Instant};

use nictui::app::{App, AppEvent};
use nictui::protocol::RemotePacket;
use nictui::remote::{
    RemoteEvidenceKind, RemoteSessionFailure, RemoteSessionFailureKind, RemoteSessionPhase,
};

#[test]
fn reconnect_clears_all_prior_session_evidence_but_preserves_diagnostics() {
    for phase in [RemoteSessionPhase::Opening, RemoteSessionPhase::Recovering] {
        let mut app = App::demo();
        app.remote_screen.show_diagnostics = true;
        app.remote_screen.unknown_packet_count = 42;
        app.remote_screen.has_display_updates = true;
        app.remote_screen.last_failure = Some(RemoteSessionFailure {
            kind: RemoteSessionFailureKind::StreamLost,
            summary: "Previous connection lost".to_string(),
            detail: "Old session".to_string(),
        });
        app.remote_screen
            .display
            .apply(&RemotePacket::DrawRectangle {
                x: 0,
                y: 0,
                width: 128,
                height: 128,
                color: 7,
            });
        assert!(app.remote_screen.battery_text.is_some());
        assert!(app.remote_screen.last_received.is_some());
        assert!(!app.remote_screen.elements.is_empty());

        app.event_tx.send(AppEvent::RemotePhase(phase)).unwrap();
        assert!(app.update());

        let screen = &app.remote_screen;
        assert_eq!(screen.phase, phase);
        assert!(screen.show_diagnostics);
        assert!(!screen.has_display_updates);
        assert!(screen.elements.is_empty());
        assert_eq!(screen.signal_strength, 0);
        assert_eq!(screen.noise_level, 0);
        assert_eq!(screen.leds, 0);
        assert_eq!(screen.battery_level, None);
        assert_eq!(screen.battery_text, None);
        assert_eq!(screen.last_small_status, None);
        assert_eq!(screen.last_signal_update, None);
        assert_eq!(screen.last_noise_update, None);
        assert_eq!(screen.last_battery_update, None);
        assert_eq!(screen.last_text_update, None);
        assert_eq!(screen.last_status_update, None);
        assert_eq!(screen.last_led_update, None);
        assert_eq!(screen.last_received, None);
        assert!(screen.last_control_report.is_none());
        assert!(screen.last_failure.is_none());
        assert!(screen.last_delta.is_none());
        assert_eq!(screen.unknown_packet_count, 0);
        let blank = nictui::remote_display::RadioDisplay::default();
        for y in 0..128 {
            for x in 0..128 {
                assert_eq!(screen.display.pixel(x, y), blank.pixel(x, y));
            }
        }
    }
}

#[test]
fn reconnect_status_only_does_not_resurrect_old_battery_or_screen_content() {
    let mut app = App::demo();
    for event in [
        AppEvent::RemotePhase(RemoteSessionPhase::Opening),
        AppEvent::RemotePhase(RemoteSessionPhase::Armed),
        AppEvent::RemoteReceived(Instant::now()),
        AppEvent::RemotePhase(RemoteSessionPhase::Live),
        AppEvent::RemotePacket(RemotePacket::SmallStatus {
            id: 0x72,
            value1: 0,
            value2: 0,
        }),
    ] {
        app.event_tx.send(event).unwrap();
    }
    app.update();
    assert_eq!(app.remote_screen.phase, RemoteSessionPhase::Live);
    assert!(app.remote_screen.last_status_update.is_some());
    assert!(app.remote_screen.last_received.is_some());
    assert_eq!(app.remote_screen.leds, 2);
    assert_eq!(app.remote_screen.battery_text, None);
    assert_eq!(app.remote_screen.last_battery_update, None);
    assert_eq!(app.remote_screen.last_text_update, None);
    assert_eq!(app.remote_screen.last_signal_update, None);
    assert_eq!(app.remote_screen.last_noise_update, None);
    assert!(!app.remote_screen.has_display_updates);
    assert_eq!(app.remote_screen.elements.len(), 1);
}

#[test]
fn telemetry_deltas_do_not_confirm_a_remote_command() {
    let mut app = App::demo();
    let mut report = app.remote_screen.last_control_report.clone().unwrap();
    report.evidence = RemoteEvidenceKind::NoControlEvidence;
    report.reaction.as_mut().unwrap().deltas = 4;
    app.event_tx.send(AppEvent::RemoteControl(report)).unwrap();
    app.update();
    assert_eq!(
        app.status_message,
        "Radio updates received; menu not confirmed"
    );
    assert!(!app.status_message.contains("confirmed by the radio"));
}

#[test]
fn control_status_is_plain_language_and_keeps_transport_details_in_logs() {
    for (success, evidence, expected) in [
        (
            true,
            RemoteEvidenceKind::NoTelemetry,
            "Menu sent; awaiting radio response",
        ),
        (
            true,
            RemoteEvidenceKind::NoControlEvidence,
            "Radio updates received; Menu not confirmed",
        ),
        (
            true,
            RemoteEvidenceKind::ControlConfirmed,
            "Menu confirmed by the radio",
        ),
        (
            false,
            RemoteEvidenceKind::CommandFailed,
            "Menu failed; check the radio connection",
        ),
        (
            false,
            RemoteEvidenceKind::ControlConfirmed,
            "Menu failed; check the radio connection",
        ),
        (
            true,
            RemoteEvidenceKind::CommandFailed,
            "Menu failed; check the radio connection",
        ),
    ] {
        let mut app = App::demo();
        let mut report = app.remote_screen.last_control_report.clone().unwrap();
        report.label = "Menu".to_string();
        report.success = success;
        report.evidence = evidence;
        report.reaction = None;
        report.detail = "step1 8A FF | raw diagnostic detail".to_string();
        app.event_tx.send(AppEvent::RemoteControl(report)).unwrap();
        app.update();
        assert_eq!(app.status_message, expected);
        assert!(!app.status_message.contains("raw-key"));
        assert!(!app.status_message.contains("8A FF"));
        assert!(app.logs.iter().any(|line| line.contains("[raw-key]")
            && line.contains("step1 8A FF | raw diagnostic detail")));
    }
}

#[test]
fn receive_freshness_does_not_move_backwards_or_imply_a_drawing() {
    let mut app = App::new();
    let now = Instant::now();
    for time in [now, now - Duration::from_secs(3)] {
        app.event_tx.send(AppEvent::RemoteReceived(time)).unwrap();
    }
    app.update();
    assert_eq!(app.remote_screen.last_received, Some(now));
    assert!(!app.remote_screen.has_display_updates);
    assert!(app.remote_screen.last_text_update.is_none());
    assert!(app.remote_screen.elements.is_empty());
}
