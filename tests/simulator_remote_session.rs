//! Persistent App -> worker -> display comparisons for UART and paired HTTP LCD.
//! Tests never inject packets, pixels, or App events; the HTTP-paired path uses
//! the production polling worker, not an oracle replay inside the test.
//! No physical ports, host audio, terminal automation, or existing servers.
use std::{
    fs::{self, File},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nictui::{
    app::{App, AppMode, MainTab},
    remote::RemoteSessionPhase,
    remote_display::RadioDisplay,
};
use ratatui::style::Color;
use serde_json::{Value, json};

const SETTLE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(25);

struct Inputs {
    binary: PathBuf,
    source: PathBuf,
    rom: PathBuf,
}

impl Inputs {
    fn explicit_environment() -> Self {
        let required = |name| {
            PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name} explicitly")))
        };
        Self {
            binary: required("NICFW_SIM_TEST_BIN"),
            source: required("NICTUI_NATIVE_SOURCE"),
            rom: required("NICTUI_NATIVE_ROM"),
        }
    }
}

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nictui-remote-session-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Simulator {
    child: Child,
    directory: Directory,
    http: Option<SocketAddr>,
    uart: Option<SocketAddr>,
}

impl Simulator {
    fn spawn(inputs: &Inputs) -> Self {
        let directory = Directory::new();
        let child = Command::new(&inputs.binary)
            .args([
                "--bind",
                "127.0.0.1",
                "--http-port",
                "0",
                "--radio-port",
                "0",
            ])
            .arg("--font-source")
            .arg(&inputs.source)
            .arg("--font-rom")
            .arg(&inputs.rom)
            .env_remove("NICFW_SOURCE")
            .env_remove("NICFW_FONT_ROM")
            .stdin(Stdio::null())
            .stdout(File::create(directory.path("stdout")).unwrap())
            .stderr(File::create(directory.path("stderr")).unwrap())
            .spawn()
            .unwrap();
        let mut simulator = Self {
            child,
            directory,
            http: None,
            uart: None,
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let output = fs::read_to_string(simulator.directory.path("stdout")).unwrap();
            for line in output.lines() {
                if let Some(address) = line.strip_prefix("GUI: http://") {
                    simulator.http = Some(owned_address(address));
                }
                if let Some(address) = line.strip_prefix("UART: sim://") {
                    simulator.uart =
                        Some(owned_address(address.split_whitespace().next().unwrap()));
                }
            }
            if simulator.http.is_some() && simulator.uart.is_some() {
                assert_ne!(simulator.http, simulator.uart);
                return simulator;
            }
            assert!(
                simulator.child.try_wait().unwrap().is_none(),
                "simulator exited: {}",
                simulator.stderr()
            );
            assert!(
                Instant::now() < deadline,
                "simulator startup timeout: {output}\n{}",
                simulator.stderr()
            );
            thread::sleep(POLL);
        }
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.directory.path("stderr")).unwrap()
    }

    fn uri(&self) -> String {
        format!("sim://{}", self.uart.unwrap())
    }

    fn request(&self, path: &str, body: Option<Value>) -> Value {
        let address = self.http.unwrap();
        let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(500)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let method = if body.is_some() { "POST" } else { "GET" };
        let body = body.map_or_else(String::new, |body| body.to_string());
        write!(socket, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut response = String::new();
        socket
            .take(2 * 1024 * 1024)
            .read_to_string(&mut response)
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    fn state(&self) -> Value {
        self.request("/api/state", None)
    }

    fn stop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
        }
        self.child.wait().unwrap();
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn owned_address(text: &str) -> SocketAddr {
    let address: SocketAddr = text.parse().unwrap();
    assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(address.port(), 0);
    address
}

struct Session(App);

impl Session {
    fn new(inputs: &Inputs, simulator: &Simulator) -> Self {
        let mut app = App::new();
        app.simulator_mode = true;
        app.protocol_port_name = Some(simulator.uri());
        app.mode = AppMode::Main(MainTab::Remote);
        app.last_main_tab = MainTab::Remote;
        app.remote_screen.display =
            RadioDisplay::with_native_fonts(&inputs.source, &inputs.rom).unwrap();
        // UART telemetry omits these settings; these are the owned model defaults.
        app.remote_screen.display.set_meter_context(0, false, 0);
        Self(app)
    }

    fn http_pair(uart: &Simulator, http: &Simulator) -> Self {
        let mut app = App::new();
        app.simulator_mode = true;
        app.protocol_port_name = Some(uart.uri());
        app.simulator_http_endpoint = Some(format!("http://{}", http.http.unwrap()));
        app.mode = AppMode::Main(MainTab::Remote);
        app.last_main_tab = MainTab::Remote;
        assert!(!app.remote_screen.display.has_native_frame());
        assert!(!app.remote_screen.display.has_meter_context());
        Self(app)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.0.remote_off();
        let deadline = Instant::now() + SETTLE;
        while self.0.remote_active && Instant::now() < deadline {
            self.0.update();
            thread::sleep(POLL);
        }
    }
}

fn wait_app(app: &mut App, label: &str, predicate: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + SETTLE;
    loop {
        app.update();
        if predicate(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: phase={:?}, failure={:?}, status={}",
            app.remote_screen.phase,
            app.remote_screen.last_failure,
            app.status_message
        );
        thread::sleep(POLL);
    }
}

fn rgb(word: u16) -> Color {
    Color::Rgb(
        (word as u8 & 31) << 3,
        ((word >> 5) as u8 & 63) << 2,
        ((word >> 11) as u8 & 31) << 3,
    )
}

fn battery(state: &Value) -> Option<&str> {
    state["packets"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find_map(|packet| {
            let text = packet["text"].as_str()?;
            (packet["type"] == "DisplayText"
                && packet["x"] == 103
                && packet["y"].as_u64()? >= 60
                && text.ends_with('V'))
            .then_some(text)
        })
}

fn telemetry_difference(app: &App, state: &Value) -> Option<String> {
    // A full snapshot contains a clearing meter header before live telemetry.
    // Compare the final value of each packet kind, just as the stream applies it.
    for (kind, field, name, actual) in [
        ("SmallStatus", "id", "LEDs", app.remote_screen.leds),
        (
            "SignalStrength",
            "strength",
            "signal",
            app.remote_screen.signal_strength,
        ),
        (
            "NoiseLevel",
            "level",
            "noise",
            app.remote_screen.noise_level,
        ),
    ] {
        let packet = state["packets"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|packet| packet["type"] == kind)
            .unwrap();
        let mut expected = packet[field].as_u64().unwrap() as u8;
        if kind == "SmallStatus" {
            expected &= 15;
        }
        if expected != actual {
            return Some(format!("{name}: expected {expected}, App {actual}"));
        }
    }
    if let Some(expected) = battery(state)
        && app.remote_screen.battery_text.as_deref() != Some(expected)
    {
        return Some(format!(
            "battery: expected {expected:?}, App {:?}",
            app.remote_screen.battery_text
        ));
    }
    None
}

fn matches_native(
    app: &mut App,
    simulator: &Simulator,
    label: &str,
    stable_for: Duration,
) -> Value {
    let deadline = Instant::now() + SETTLE;
    let mut matched_since = None;
    loop {
        app.update();
        let state = simulator.state();
        app.update();
        assert_eq!(state["simulated"], true);
        assert_eq!(state["transmitting"], false, "test never sends PTT");
        assert_eq!(state["reference_firmware"], "2.52.18");
        assert_eq!(state["display_fidelity"], "source-assets");
        let frame = &state["framebuffer"];
        assert_eq!(frame["complete"], true, "{label}: {frame}");
        assert_eq!(
            (frame["width"].as_u64(), frame["height"].as_u64()),
            (Some(128), Some(128))
        );
        let expected = frame["pixels"].as_array().unwrap();
        assert_eq!(expected.len(), 128 * 128);
        let mut count = 0;
        let mut examples = Vec::new();
        for (index, expected) in expected.iter().enumerate() {
            let (x, y) = (index % 128, index / 128);
            let expected = rgb(u16::try_from(expected.as_u64().unwrap()).unwrap());
            let actual = app.remote_screen.display.pixel(x, y);
            if actual != expected {
                count += 1;
                if examples.len() < 12 {
                    examples.push(format!("({x},{y}) expected={expected:?} App={actual:?}"));
                }
            }
        }
        let telemetry = telemetry_difference(app, &state);
        if count == 0 && telemetry.is_none() && app.remote_controls_ready() {
            let since = matched_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= stable_for {
                if app.simulator_http_endpoint.is_some() {
                    assert!(app.remote_screen.display.has_native_frame());
                    assert!(app.remote_screen.simulator_lcd_error.is_none());
                } else {
                    assert!(app.remote_screen.display.uses_native_fonts());
                    assert!(app.remote_screen.display.has_meter_context());
                    assert!(app.remote_screen.display.native_font_coverage_complete());
                }
                assert!(app.remote_screen.last_failure.is_none());
                return state;
            }
        } else {
            matched_since = None;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: {count} native pixel mismatches; {}; telemetry={telemetry:?}; phase={:?}; native_http={}; lcd_error={:?}; status={}; delta={:?}",
            examples.join("; "),
            app.remote_screen.phase,
            app.remote_screen.display.has_native_frame(),
            app.remote_screen.simulator_lcd_error,
            app.status_message,
            app.remote_screen.last_delta
        );
        thread::sleep(POLL);
    }
}

fn send_key(app: &mut App, code: u8, label: &str) {
    // Hash switches the active A/B VFO without either PTT key.
    assert!(matches!(code, 0x0b..=0x0e | 0x10));
    wait_app(app, label, App::remote_controls_ready);
    app.send_key(code);
    assert!(
        !app.remote_controls_ready(),
        "{label}: action was not queued"
    );
    wait_app(app, label, App::remote_controls_ready);
    assert!(
        app.remote_screen
            .last_control_report
            .as_ref()
            .unwrap()
            .success,
        "{label}"
    );
}

fn key(app: &mut App, simulator: &Simulator, code: u8, label: &str) -> Value {
    send_key(app, code, label);
    matches_native(app, simulator, label, Duration::from_millis(100))
}

fn stop(app: &mut App, simulator: &Simulator) {
    app.remote_off();
    assert!(app.held_remote_key().is_none());
    wait_app(app, "Remote stop/quit release", |app| !app.remote_active);
    assert!(app.remote_tx.is_none());
    assert_eq!(app.remote_screen.phase, RemoteSessionPhase::Stopped);
    assert!(app.remote_screen.last_failure.is_none());
    let deadline = Instant::now() + SETTLE;
    loop {
        let state = simulator.state();
        if state["uart_connected"] == false {
            assert_eq!(state["uart_mode"], "idle");
            assert_eq!(state["transmitting"], false);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "owned simulator did not release UART: {state}"
        );
        thread::sleep(POLL);
    }
}

#[test]
#[ignore = "requires explicit NICFW_SIM_TEST_BIN, NICTUI_NATIVE_SOURCE, NICTUI_NATIVE_ROM; owned ports0 only"]
fn persistent_app_remote_session_matches_native_frames_through_actions_and_restart() {
    let inputs = Inputs::explicit_environment();
    let simulator = Simulator::spawn(&inputs);
    let mut session = Session::new(&inputs, &simulator);
    let app = &mut session.0;
    app.remote_on();
    let initial = matches_native(app, &simulator, "initial idle", Duration::from_millis(1100));
    assert_eq!(initial["radio_state"]["active_vfo"], 0);
    assert_eq!(app.remote_screen.leds, 0);
    let received = app.remote_screen.last_received.unwrap();
    matches_native(
        app,
        &simulator,
        "late idle repaint/dedupe",
        Duration::from_millis(1100),
    );
    assert!(app.remote_screen.last_received.unwrap() > received);

    key(app, &simulator, 0x0b, "open User Menu");
    key(app, &simulator, 0x0b, "edit Squelch");
    key(app, &simulator, 0x0d, "Squelch draft 1");
    key(app, &simulator, 0x0d, "Squelch draft 0");
    key(app, &simulator, 0x0b, "commit synthetic receive");
    let receive_a = key(app, &simulator, 0x0e, "leave menu / synthetic RX A");
    assert_eq!(receive_a["radio_state"]["menu_values"]["Squelch"], 0);
    assert_eq!(app.remote_screen.leds, 1);
    let receive_b = key(app, &simulator, 0x10, "switch A to B without PTT");
    assert_eq!(receive_b["radio_state"]["active_vfo"], 1);
    assert_eq!(app.remote_screen.leds, 4);

    stop(app, &simulator);
    app.remote_on();
    let resumed = matches_native(
        app,
        &simulator,
        "same App restart / RX B",
        Duration::from_millis(200),
    );
    assert_eq!(resumed["radio_state"]["active_vfo"], 1);
    assert_eq!(app.remote_screen.leds, 4);
    key(app, &simulator, 0x10, "switch B back to A");
    key(app, &simulator, 0x0b, "reopen User Menu");
    key(app, &simulator, 0x0b, "edit Squelch again");
    key(app, &simulator, 0x0c, "restore Squelch draft 1");
    key(app, &simulator, 0x0c, "restore Squelch draft 2");
    key(app, &simulator, 0x0b, "save restored Squelch");
    let idle = key(app, &simulator, 0x0e, "return to idle / LEDs off");
    assert_eq!(app.remote_screen.leds, 0);
    let before_battery = battery(&idle).unwrap().to_owned();
    let battery_time = app.remote_screen.last_battery_update.unwrap();
    for tick in 0..5 {
        simulator.request("/api/tick", Some(json!({"milliseconds": 1000})));
        matches_native(
            app,
            &simulator,
            &format!("late battery/LED repaint {tick}"),
            Duration::from_millis(80),
        );
    }
    assert_ne!(
        app.remote_screen.battery_text.as_deref(),
        Some(before_battery.as_str())
    );
    assert!(app.remote_screen.last_battery_update.unwrap() > battery_time);

    // Same stop path used by quit: release an accepted, non-PTT held key first.
    assert_eq!(
        app.remote_screen
            .last_control_report
            .as_ref()
            .unwrap()
            .label,
        "exit"
    );
    assert!(app.press_remote_key(0x0b));
    assert_eq!(app.held_remote_key(), Some(0x0b));
    wait_app(app, "held Menu reached actual worker", |app| {
        app.remote_screen
            .last_control_report
            .as_ref()
            .is_some_and(|report| report.label == "menu" && report.success)
    });
    assert_eq!(app.held_remote_key(), Some(0x0b));
    stop(app, &simulator);
}

#[test]
#[ignore = "requires explicit NICFW_SIM_TEST_BIN, NICTUI_NATIVE_SOURCE, NICTUI_NATIVE_ROM; owned ports0 only"]
fn paired_http_native_frames_follow_actual_uart_actions_and_restart() {
    let inputs = Inputs::explicit_environment();
    let simulator = Simulator::spawn(&inputs);
    let mut session = Session::http_pair(&simulator, &simulator);
    let app = &mut session.0;
    app.remote_on();
    matches_native(
        app,
        &simulator,
        "HTTP paired idle",
        Duration::from_millis(1100),
    );
    assert!(
        !app.remote_screen.display.has_meter_context(),
        "HTTP path must not need a guessed meter context"
    );
    key(app, &simulator, 0x0b, "HTTP frame / UART Menu");
    key(app, &simulator, 0x0e, "HTTP frame / UART Exit");
    let switched = key(app, &simulator, 0x10, "HTTP frame / UART switch B");
    assert_eq!(switched["radio_state"]["active_vfo"], 1);
    let channel = switched["radio_state"]["vfos"][1]["channel"]
        .as_u64()
        .unwrap();
    let moved = key(app, &simulator, 0x0c, "HTTP frame / UART channel Up");
    assert_eq!(
        moved["radio_state"]["vfos"][1]["channel"].as_u64().unwrap(),
        channel + 1
    );
    stop(app, &simulator);
    app.remote_on();
    let resumed = matches_native(
        app,
        &simulator,
        "HTTP pair same App restart",
        Duration::from_millis(200),
    );
    assert_eq!(resumed["radio_state"]["active_vfo"], 1);
    assert_eq!(
        resumed["radio_state"]["vfos"][1]["channel"]
            .as_u64()
            .unwrap(),
        channel + 1
    );
    key(app, &simulator, 0x10, "HTTP frame / UART return A");
    assert!(!app.remote_screen.display.has_meter_context());
    stop(app, &simulator);
}

#[test]
#[ignore = "requires explicit NICFW_SIM_TEST_BIN, NICTUI_NATIVE_SOURCE, NICTUI_NATIVE_ROM; two owned ports0 backends"]
fn mismatched_http_radio_is_rejected_while_uart_remains_usable() {
    let inputs = Inputs::explicit_environment();
    let radio = Simulator::spawn(&inputs);
    let wrong_radio = Simulator::spawn(&inputs);
    wrong_radio.request("/api/key", Some(json!({"key": 0x10, "action": "tap"})));
    let wrong_state = wrong_radio.request("/api/tick", Some(json!({"milliseconds": 500})));
    assert_eq!(wrong_state["radio_state"]["active_vfo"], 1);
    let mut session = Session::http_pair(&radio, &wrong_radio);
    let app = &mut session.0;
    app.remote_on();
    let deadline = Instant::now() + SETTLE;
    loop {
        app.update();
        assert!(
            !app.remote_screen.display.has_native_frame(),
            "another radio's native frame was accepted"
        );
        assert_eq!(app.mode, AppMode::Main(MainTab::Remote));
        if app.remote_controls_ready() && app.remote_screen.simulator_lcd_error.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "wrong-pair diagnostic not surfaced: phase={:?}, lcd_error={:?}, status={}",
            app.remote_screen.phase,
            app.remote_screen.simulator_lcd_error,
            app.status_message
        );
        thread::sleep(POLL);
    }
    assert!(
        app.remote_screen.has_display_updates,
        "UART fallback did not receive drawings"
    );
    assert!(app.remote_screen.last_received.is_some());
    send_key(app, 0x0b, "wrong HTTP pair / actual UART Menu");
    let packets = radio.state()["packets"].clone();
    assert!(
        packets
            .as_array()
            .unwrap()
            .iter()
            .any(|packet| packet["text"] == "User Menu")
    );
    send_key(app, 0x0e, "wrong HTTP pair / actual UART Exit");
    send_key(app, 0x10, "wrong HTTP pair / actual UART switch B");
    assert_eq!(radio.state()["radio_state"]["active_vfo"], 1);
    assert_eq!(
        wrong_radio.state()["radio_state"],
        wrong_state["radio_state"],
        "UART commands changed the wrong simulator"
    );
    assert!(!app.remote_screen.display.has_native_frame());
    assert!(app.remote_screen.simulator_lcd_error.is_some());
    assert!(app.remote_screen.last_failure.is_none());
    stop(app, &radio);
}

#[test]
#[ignore = "requires explicit NICFW_SIM_TEST_BIN, NICTUI_NATIVE_SOURCE, NICTUI_NATIVE_ROM; owned backend stop only"]
fn paired_http_backend_exit_keeps_remote_shutdown_within_300ms() {
    let inputs = Inputs::explicit_environment();
    let mut simulator = Simulator::spawn(&inputs);
    let mut session = Session::http_pair(&simulator, &simulator);
    let app = &mut session.0;
    app.remote_on();
    matches_native(
        app,
        &simulator,
        "HTTP pair before owned backend exit",
        Duration::from_millis(200),
    );
    simulator.stop();
    let started = Instant::now();
    app.remote_off();
    while app.remote_active && started.elapsed() < Duration::from_millis(300) {
        app.update();
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        !app.remote_active,
        "HTTP-paired worker did not stop within 300ms: {:?}, {}",
        app.remote_screen.phase, app.status_message
    );
    assert!(started.elapsed() < Duration::from_millis(300));
    assert!(app.remote_tx.is_none());
    assert!(app.held_remote_key().is_none());
    assert_eq!(app.mode, AppMode::Main(MainTab::Remote));
}

#[test]
#[ignore = "requires explicit NICFW_SIM_TEST_BIN, NICTUI_NATIVE_SOURCE, NICTUI_NATIVE_ROM; malformed HTTP never connects"]
fn malformed_app_http_endpoint_keeps_diagnostic_and_uart_controls_across_restart() {
    let inputs = Inputs::explicit_environment();
    let simulator = Simulator::spawn(&inputs);
    let mut session = Session::http_pair(&simulator, &simulator);
    let app = &mut session.0;
    // Exercise public App configuration, bypassing CLI validation deliberately.
    // DNS endpoints are rejected by parsing; only the owned numeric UART opens.
    app.simulator_http_endpoint = Some("http://localhost:12345".into());
    for cycle in 0..2 {
        app.remote_on();
        wait_app(app, "malformed HTTP / usable UART", |app| {
            app.remote_controls_ready() && app.remote_screen.simulator_lcd_error.is_some()
        });
        let diagnostic = app.remote_screen.simulator_lcd_error.clone().unwrap();
        assert!(diagnostic.contains("numeric loopback"), "{diagnostic}");
        assert_eq!(app.mode, AppMode::Main(MainTab::Remote));
        assert!(!app.remote_screen.display.has_native_frame());
        assert!(app.remote_screen.has_display_updates);
        assert!(app.remote_screen.last_received.is_some());
        send_key(app, 0x0b, "malformed HTTP / UART Menu");
        assert!(
            simulator.state()["packets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|packet| packet["text"] == "User Menu")
        );
        send_key(app, 0x0e, "malformed HTTP / UART Exit");
        let until = Instant::now() + Duration::from_millis(1100);
        while Instant::now() < until {
            app.update();
            assert_eq!(app.mode, AppMode::Main(MainTab::Remote));
            assert_eq!(
                app.remote_screen.simulator_lcd_error.as_deref(),
                Some(diagnostic.as_str()),
                "cycle {cycle}: diagnostic disappeared after startup/control/idle events"
            );
            assert!(!app.remote_screen.display.has_native_frame());
            assert!(app.remote_screen.last_failure.is_none());
            thread::sleep(POLL);
        }
        stop(app, &simulator);
    }
}
