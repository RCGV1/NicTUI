//! External integration matrix. Run with NICFW_SIM_TEST_BIN=/path/to/nicfw-sim
//! cargo test --test simulator_workflows -- --ignored. No existing endpoint is used.
//! Default live-model readback, explicit frozen/blank EEPROM, CLI exports/import
//! validation, activity and remote are exercised. No physical device or host audio.
//! EEPROM writes/reboot/bootloader/live-mode EEPROM and BLE/ASL hardware are NOT
//! simulated capabilities: writes must fail, reboot gets no ACK, hardware is parse-only.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use nictui::app::{App, AppEvent, AppMode, MainTab};
use nictui::cli::Cli;
use nictui::device::{
    flash_firmware, update_dtmf_preset, update_scan_preset, write_band_plans, write_channels,
    write_codeplug, write_dtmf_presets, write_group_labels, write_scan_presets, write_settings,
};
use nictui::protocol::{EEPROM_SIZE, Endianness, RadioProtocol, ScanPreset};
use serde_json::{Value, json};

const LIMIT: Duration = Duration::from_secs(15);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nictui-simulator-workflows-{}-{}-{}",
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

// File-backed output avoids pipe-capacity deadlocks and blocking reader joins.
struct Process {
    child: Child,
    directory: Directory,
    deadline: Instant,
}

struct Output {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Output {
    fn success(&self) -> &Self {
        assert!(self.status.success(), "{}\n{}", self.stdout, self.stderr);
        self
    }

    fn json(&self) -> Value {
        self.success();
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}\n{}", self.stdout, self.stderr))
    }

    fn failure(&self) {
        assert!(
            !self.status.success(),
            "unexpected success: {}",
            self.stdout
        );
        assert!(!self.stderr.trim().is_empty(), "failure needs a diagnostic");
    }

    fn unsupported_programming(&self) {
        self.failure();
        assert!(
            self.stderr.contains("Unsupported simulator operation"),
            "{}",
            self.stderr
        );
        assert!(self.stderr.contains("read-only"), "{}", self.stderr);
        assert!(
            self.stderr.contains("reboot is not acknowledged"),
            "{}",
            self.stderr
        );
        assert!(
            self.stdout
                .lines()
                .all(|line| line.starts_with("Validated firmware image ")),
            "unexpected success output: {}",
            self.stdout
        );
        for success in [
            "write complete",
            "Updated ",
            "Rebooting radio",
            "transfer acknowledged",
        ] {
            assert!(!self.stderr.contains(success), "{}", self.stderr);
        }
    }
}

impl Process {
    fn spawn(command: &mut Command, limit: Duration) -> Self {
        let directory = Directory::new();
        let child = command
            .stdin(Stdio::null())
            .stdout(File::create(directory.path("stdout")).unwrap())
            .stderr(File::create(directory.path("stderr")).unwrap())
            .spawn()
            .unwrap();
        Self {
            child,
            directory,
            deadline: Instant::now() + limit,
        }
    }

    fn stdout(&self) -> String {
        fs::read_to_string(self.directory.path("stdout")).unwrap()
    }

    fn finish(mut self) -> Output {
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Output {
                    status,
                    stdout: self.stdout(),
                    stderr: fs::read_to_string(self.directory.path("stderr")).unwrap(),
                };
            }
            if Instant::now() >= self.deadline {
                self.stop();
                panic!(
                    "child exceeded deadline: {}\n{}",
                    self.stdout(),
                    fs::read_to_string(self.directory.path("stderr")).unwrap()
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
        }
        self.child.wait().unwrap();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Simulator {
    process: Process,
    gui: SocketAddr,
    uart: SocketAddr,
}

fn literal_loopback(endpoint: &str) -> SocketAddr {
    let address: SocketAddr = endpoint.parse().unwrap();
    assert_eq!(address.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(address.port(), 0);
    address
}

impl Simulator {
    fn spawn(extra: &[&str]) -> Self {
        let binary = std::env::var_os("NICFW_SIM_TEST_BIN")
            .expect("set NICFW_SIM_TEST_BIN to the separately installed nicfw-sim binary");
        let mut command = Command::new(binary);
        command.args([
            "--bind",
            "127.0.0.1",
            "--http-port",
            "0",
            "--radio-port",
            "0",
        ]);
        command.args(extra);
        let mut process = Process::spawn(&mut command, Duration::from_secs(8));
        loop {
            let output = process.stdout();
            let gui = output
                .lines()
                .find_map(|line| line.strip_prefix("GUI: http://"));
            let uart = output
                .lines()
                .find_map(|line| line.strip_prefix("UART: sim://"))
                .and_then(|line| line.split_whitespace().next());
            if let (Some(gui), Some(uart)) = (gui, uart) {
                return Self {
                    gui: literal_loopback(gui),
                    uart: literal_loopback(uart),
                    process,
                };
            }
            assert!(
                process.child.try_wait().unwrap().is_none(),
                "simulator exited: {output}"
            );
            assert!(
                Instant::now() < process.deadline,
                "simulator startup timed out: {output}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn fixture(path: &Path) -> Self {
        Self::spawn(&["--eeprom-image", path.to_str().unwrap()])
    }

    fn uri(&self) -> String {
        format!("sim://{}", self.uart)
    }

    fn socket(&self, address: SocketAddr) -> TcpStream {
        assert!(address == self.gui || address == self.uart);
        assert_eq!(address.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        let socket = TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        socket.set_nodelay(true).unwrap();
        socket
    }

    fn http(&self, path: &str, body: Option<Value>) -> Value {
        let mut stream = self.socket(self.gui);
        let payload = body.map(|value| value.to_string());
        let method = if payload.is_some() { "POST" } else { "GET" };
        let payload = payload.unwrap_or_default();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}", self.gui, payload.len()).unwrap();
        let mut response = String::new();
        stream
            .take(2 * 1024 * 1024)
            .read_to_string(&mut response)
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    fn state(&self) -> Value {
        self.http("/api/state", None)
    }

    fn wait_state(&self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let state = self.state();
            if predicate(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "state deadline exceeded: {state}"
            );
            thread::sleep(Duration::from_millis(40));
        }
    }

    fn key(&self, key: u8, action: &str) -> Value {
        self.http("/api/key", Some(json!({"key": key, "action": action})))
    }

    fn safe_short_key(&self, key: u8) {
        assert!(
            matches!(key, 0x0b..=0x0e),
            "only Menu/Up/Down/Exit, never PTT"
        );
        self.key(key, "press");
        // Source debounce is six ticks at 60 Hz, not a 10 ms cadence.
        self.http("/api/tick", Some(json!({"milliseconds": 150})));
        self.key(key, "release");
        let state = self.http("/api/tick", Some(json!({"milliseconds": 150})));
        assert_eq!(state["transmitting"], false);
        assert!(state["gui_hold"].is_null());
    }

    fn cli_process(&self, args: &[&str]) -> Process {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
        command.args(args).args(["--port", &self.uri()]);
        Process::spawn(&mut command, LIMIT)
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.wait_state(|state| state["uart_connected"] == false);
        let output = self.cli_process(args).finish();
        self.wait_state(|state| state["uart_connected"] == false);
        output
    }
}

fn read_all_app(simulator: &Simulator) -> App {
    simulator.wait_state(|state| state["uart_connected"] == false);
    let mut app = App::new();
    app.simulator_mode = true;
    app.protocol_port_name = Some(simulator.uri());
    app.mode = AppMode::Main(MainTab::Channels);
    app.last_main_tab = MainTab::Channels;
    app.last_non_remote_tab = MainTab::Channels;
    app.start_read_all();
    assert_eq!(app.mode, AppMode::Reading, "{}", app.status_message);
    let deadline = Instant::now() + LIMIT;
    while app.mode == AppMode::Reading {
        app.update();
        assert!(
            Instant::now() < deadline,
            "Read All timed out: {}",
            app.status_message
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        app.mode,
        AppMode::Main(MainTab::Channels),
        "{}",
        app.status_message
    );
    assert_eq!(app.progress, 1.0);
    assert!(app.codeplug_snapshot_is_current());
    let state = simulator.wait_state(|state| state["uart_connected"] == false);
    assert_eq!(state["programming_disabled"], false);
    assert_eq!(state["transmitting"], false);
    app
}

fn mhz(raw_10hz: u64) -> String {
    format!("{}.{:05}", raw_10hz / 100_000, raw_10hz % 100_000)
}

fn assert_app_matches_live_model(app: &App, state: &Value) {
    assert_eq!(state["simulated"], true);
    assert_eq!(state["source"], "nicfw-source-model");
    assert_eq!(
        state["eeprom_source"],
        "synthetic-model-readback-no-real-radio"
    );
    assert_eq!(state["eeprom_live_model"], true);
    assert_eq!(state["eeprom_read_only"], true);
    assert_eq!(state["transmitting"], false);
    let model = &state["radio_state"];
    assert_eq!(app.channels.len(), 198);
    // Independent source model contract: one shared bank, not a second copy of
    // NicTUI's decoder. Slots 1-99 are VHF; slots 100-198 are UHF, step 12.5 kHz.
    for (index, channel) in app.channels.iter().enumerate() {
        let number = index as u16 + 1;
        let raw = if number < 100 {
            14_550_000 + u64::from(number - 1) * 1250
        } else {
            43_350_000 + u64::from(number - 100) * 1250
        };
        assert_eq!(channel.channel_num, number);
        assert_eq!(channel.rx_freq, mhz(raw), "channel {number}");
        assert_eq!(channel.tx_freq, mhz(raw), "channel {number}");
    }
    let image = app.codeplug_data.as_ref().unwrap();
    assert_eq!(image.len(), EEPROM_SIZE);
    let decoded = nictui::device::inspect_codeplug(image).unwrap();
    assert_eq!(decoded.vfo_memories.len(), 2);
    for (index, memory) in decoded.vfo_memories.iter().enumerate() {
        let vfo = &model["vfos"][index];
        let saved = vfo["saved_frequency_10hz"].as_u64().unwrap();
        assert_eq!(memory.channel.rx_freq, mhz(saved));
        assert_eq!(memory.channel.tx_freq, mhz(saved));
        if vfo["channel_mode"] == true {
            let number = vfo["channel"].as_u64().unwrap() as usize;
            assert_eq!(
                app.channels[number - 1].rx_freq,
                mhz(vfo["frequency_10hz"].as_u64().unwrap())
            );
        }
    }
    let settings = json!(app.settings.as_ref().unwrap());
    assert_eq!(settings["magic"], 0xd82f);
    assert_eq!(settings["active_vfo"], model["active_vfo"]);
    for (field, title) in [
        ("squelch", "Squelch"),
        ("step", "Step"),
        ("dual_watch", "Dual Watch"),
        ("ptt_mode", "PTT Mode"),
        ("tx_mod_meter", "TX Mod Meter"),
        ("mic_gain", "Mic Gain"),
        ("tx_deviation", "TX Deviation"),
        ("batt_style", "Batt Style"),
        ("scan_range", "VFO Scan Range"),
        ("scan_persist", "Scan Persist"),
        ("scan_resume", "Scan Resume"),
        ("ultra_scan", "Scan Ultra"),
        ("tone_monitor", "Tone Monitor"),
        ("lcd_brightness", "LCD Brightness"),
        ("lcd_timeout", "LCD Timeout"),
        ("breathe", "Heartbeat"),
        ("dtmf_dev", "DTMF Volume"),
        ("gamma", "LCD Gamma"),
        ("repeater_tone", "Repeater Tone"),
        ("bluetooth", "Bluetooth"),
        ("power_save", "Power Save"),
        ("key_tones", "Key Tones"),
        ("ste", "Sq Tail Elim"),
        ("rf_gain", "RF Gain"),
        ("s_bar_style", "S-Bar Style"),
        ("sq_noise_lev", "Sq Noise Lev"),
        ("vox", "VOX"),
        ("vox_tail", "VOX Tail"),
        ("tx_timeout", "TX Timeout"),
        ("dimmer", "Dim Brightness"),
        ("dtmf_speed", "DTMF Speed"),
        ("noise_gate", "Noise Gate"),
        ("scan_update", "Scan Update"),
        ("asl", "ASL Support"),
        ("disable_fmt", "Disable FMT"),
        ("pin", "PIN"),
        ("pin_action", "PIN Action"),
        ("af_filters", "AF Filters"),
        ("if_freq", "IF (kHz)"),
        ("s_bar_always_on", "SBar AlwaysOn"),
        ("vfo_lock_active", "VFO DW Lock"),
        ("dual_watch_delay", "DualWatch Delay"),
        ("sub_tone_deviation", "SubTone Dev"),
        ("rx_split", "RX VHF->UHF"),
        ("tx_split", "TX VHF->UHF"),
    ] {
        let expected = &model["menu_values"][title];
        assert!(expected.is_number(), "missing HTTP model setting {title}");
        assert_eq!(&settings[field], expected, "{field} / {title}");
    }
    for (index, vfo) in model["vfos"].as_array().unwrap().iter().enumerate() {
        let persisted = &app.settings.as_ref().unwrap().vfo_state[index];
        assert_eq!(json!(persisted.group), vfo["group"]);
        assert_eq!(persisted.mode != 0, vfo["channel_mode"].as_bool().unwrap());
        assert_eq!(
            u64::from(persisted.group_mode_channels[usize::from(persisted.group)]),
            vfo["channel"].as_u64().unwrap() + 1
        );
    }
    assert_eq!(app.group_labels.len(), 15);
    assert_eq!(&app.group_labels[..2], &["SIM A", "SIM B"]);
    assert_eq!(app.scan_presets.len(), 20);
    assert!(app.scan_presets.iter().all(|preset| preset.start_freq == 0));
    assert_eq!(app.band_plans.len(), 20);
    assert_eq!(app.band_plans[0].start_freq, 1_800_000);
    assert_eq!(app.band_plans[0].end_freq, 130_000_000);
    assert_eq!(app.dtmf_presets.len(), 20);
    assert!(
        app.dtmf_presets
            .iter()
            .all(|preset| preset.label.is_empty() && preset.digits.is_empty())
    );
}

fn change_only_local_model(simulator: &Simulator) -> Value {
    let before = simulator.state();
    assert_eq!(before["uart_connected"], false);
    assert_eq!(before["radio_state"]["vfos"][0]["channel_mode"], true);
    // Source Exit cycles memory -> group -> VFO. Up changes only synthetic VFO A.
    for key in [0x0e, 0x0e, 0x0c] {
        simulator.safe_short_key(key);
    }
    // First menu item is Squelch. Commit one bounded menu increment, then leave.
    for key in [0x0b, 0x0b, 0x0c, 0x0b, 0x0e] {
        simulator.safe_short_key(key);
    }
    let after = simulator.state();
    let old = &before["radio_state"];
    let new = &after["radio_state"];
    assert_eq!(new["vfos"][0]["channel_mode"], false);
    assert_eq!(
        new["vfos"][0]["saved_frequency_10hz"].as_u64().unwrap(),
        old["vfos"][0]["saved_frequency_10hz"].as_u64().unwrap()
            + old["menu_values"]["Step"].as_u64().unwrap()
    );
    assert_eq!(
        new["menu_values"]["Squelch"].as_u64().unwrap(),
        old["menu_values"]["Squelch"].as_u64().unwrap() + 1
    );
    assert_eq!(after["transmitting"], false);
    after
}

fn fixture(directory: &Directory) -> (PathBuf, Vec<u8>) {
    // App::demo() constructs synthetic data without device enumeration or I/O.
    let mut image = App::demo().codeplug_data.unwrap();
    assert_eq!(image.len(), EEPROM_SIZE);
    // Demo scan frequencies are display examples, not valid import units. Model
    // all twenty slots explicitly so erased records are not imported as data.
    for index in 0..20u8 {
        let preset = ScanPreset {
            index,
            start_freq: 14_500_000 + u32::from(index) * 100_000,
            range: 100,
            step: 1250,
            resume: 2,
            persist: 1,
            label: format!("SIM {index}"),
            ..Default::default()
        };
        let offset = 0x1b00 + usize::from(index) * 20;
        image[offset..offset + 20]
            .copy_from_slice(&RadioProtocol::pack_scan_preset(&preset, Endianness::Big));
    }
    // Demo leaves unused DTMF records erased; zero records export/import cleanly.
    image[0x1cf0 + 8 * 12..0x1de0].fill(0);
    // VOX tail is stored in 0.1 s units; the demo's 200 exceeds its 5..=50 range.
    image[0x1953..0x1955].copy_from_slice(&10u16.to_be_bytes());
    image[0x1961] = 0; // IF frequency is an enum index (0..=6), not MHz.
    image[0x1a00..0x1a02].copy_from_slice(&0xa46du16.to_be_bytes());
    let path = directory.path("synthetic.nfw");
    fs::write(&path, &image).unwrap();
    (path, image)
}

fn ack(stream: &mut TcpStream, command: u8) {
    stream.write_all(&[command]).unwrap();
    let mut reply = [0];
    stream.read_exact(&mut reply).unwrap();
    assert_eq!(reply, [command]);
}

fn block(stream: &mut TcpStream, index: u8) -> [u8; 32] {
    stream.write_all(&[0x30, index]).unwrap();
    let mut reply = [0; 34];
    stream.read_exact(&mut reply).unwrap();
    assert_eq!(reply[0], 0x30);
    assert_eq!(
        reply[33],
        reply[1..33]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
    );
    reply[1..33].try_into().unwrap()
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn default_model_all_cli_sections_and_app_read_all_match_http_source_state() {
    let simulator = Simulator::spawn(&[]);
    let state = simulator.state();
    let app = read_all_app(&simulator);
    assert_app_matches_live_model(&app, &state);
    assert_eq!(
        simulator.cli(&["probe", "--json"]).json()["nicsure_ready"],
        true
    );
    for (section, expected) in [
        ("channels", json!(app.channels)),
        ("settings", json!(app.settings)),
        ("groups", json!(app.group_labels)),
        ("scan-presets", json!(app.scan_presets)),
        ("band-plan", json!(app.band_plans)),
        ("dtmf", json!(app.dtmf_presets)),
    ] {
        assert_eq!(
            simulator.cli(&[section, "read"]).json(),
            expected,
            "default {section}"
        );
    }
    let directory = Directory::new();
    let backup = directory.path("default-model.nfw");
    simulator
        .cli(&["codeplug", "backup", "--output", backup.to_str().unwrap()])
        .success();
    assert_eq!(fs::read(backup).unwrap(), app.codeplug_data.unwrap());
    assert_eq!(
        simulator.state()["radio_state"],
        state["radio_state"],
        "reads changed model state"
    );
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn default_model_safe_controls_refresh_vfo_and_settings_on_subsequent_reads() {
    let simulator = Simulator::spawn(&[]);
    let before = read_all_app(&simulator);
    assert_app_matches_live_model(&before, &simulator.state());
    let state = change_only_local_model(&simulator);
    let after = read_all_app(&simulator);
    assert_app_matches_live_model(&after, &state);
    assert_ne!(after.codeplug_data, before.codeplug_data);
    assert_eq!(
        json!(after.channels),
        json!(before.channels),
        "VFO tuning must not rewrite the shared channel bank"
    );
    assert_eq!(
        after.settings.as_ref().unwrap().squelch,
        before.settings.as_ref().unwrap().squelch + 1
    );
    assert_eq!(
        simulator.cli(&["settings", "read"]).json(),
        json!(after.settings)
    );
    assert_eq!(simulator.state()["radio_state"], state["radio_state"]);
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn blank_and_fixture_replay_all_256_blocks_with_source_framing() {
    let directory = Directory::new();
    let (path, image) = fixture(&directory);
    for (simulator, expected) in [
        (
            Simulator::spawn(&["--blank-eeprom"]),
            vec![0xff; EEPROM_SIZE],
        ),
        (Simulator::fixture(&path), image),
    ] {
        assert_eq!(simulator.state()["eeprom_read_only"], true);
        let mut uart = simulator.socket(simulator.uart);
        ack(&mut uart, 0x01); // ping, packet mode stop, programming pause
        ack(&mut uart, 0x4b);
        ack(&mut uart, 0x45);
        assert_eq!(simulator.state()["programming_disabled"], true);
        for index in 0..=255u8 {
            assert_eq!(
                block(&mut uart, index).as_slice(),
                &expected[usize::from(index) * 32..(usize::from(index) + 1) * 32]
            );
        }
        ack(&mut uart, 0x46);
        assert_eq!(simulator.state()["programming_disabled"], false);
    }
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn cli_reads_every_programming_section_and_roundtrips_exports_without_writes() {
    let directory = Directory::new();
    let (path, image) = fixture(&directory);
    let simulator = Simulator::fixture(&path);
    let demo = App::demo();
    let decoded = nictui::device::inspect_codeplug(&image).unwrap();
    let probe = simulator.cli(&["probe", "--json"]).json();
    assert_eq!(probe["handshake"], "ok");
    assert_eq!(probe["nicsure_ready"], true);
    let channels = simulator
        .cli(&["channels", "read", "--format", "json"])
        .json();
    assert_eq!(channels.as_array().unwrap().len(), demo.channels.len());
    assert_eq!(channels[0]["name"], demo.channels[0].name);
    let one = simulator
        .cli(&["channels", "get", "--channel", "1", "--format", "json"])
        .json();
    assert!(one.to_string().contains(&demo.channels[0].name));
    let settings = simulator.cli(&["settings", "read"]).json();
    assert_eq!(settings["magic"], 0xd82f);
    assert_eq!(settings["squelch"], demo.settings.as_ref().unwrap().squelch);
    simulator
        .cli(&["settings", "get", "--setting", "Squelch"])
        .json();
    assert_eq!(
        simulator.cli(&["groups", "read"]).json(),
        json!(decoded.group_labels)
    );
    simulator.cli(&["groups", "get", "--group", "1"]).json();
    simulator.cli(&["bluetooth", "status"]).json();
    for (section, expected) in [
        ("scan-presets", json!(decoded.scan_presets)),
        ("band-plan", json!(decoded.band_plans)),
        ("dtmf", json!(decoded.dtmf_presets)),
    ] {
        let records = simulator.cli(&[section, "read"]).json();
        assert_eq!(records, expected, "{section}");
        simulator.cli(&[section, "get", "--index", "0"]).json();
        let export = directory.path(&format!("{section}.json"));
        simulator
            .cli(&[section, "read", "--output", export.to_str().unwrap()])
            .success();
        simulator
            .cli(&[
                section,
                "write",
                "--input",
                export.to_str().unwrap(),
                "--validate-only",
            ])
            .success();
    }
    let csv = directory.path("channels.csv");
    simulator
        .cli(&[
            "channels",
            "export",
            "--format",
            "csv",
            "--output",
            csv.to_str().unwrap(),
        ])
        .success();
    assert!(
        fs::read_to_string(&csv)
            .unwrap()
            .contains(&demo.channels[0].name)
    );
    simulator
        .cli(&[
            "channels",
            "import",
            "--input",
            csv.to_str().unwrap(),
            "--validate-only",
        ])
        .success();
    let settings_path = directory.path("settings.json");
    simulator
        .cli(&[
            "settings",
            "read",
            "--output",
            settings_path.to_str().unwrap(),
        ])
        .success();
    simulator
        .cli(&[
            "settings",
            "write",
            "--input",
            settings_path.to_str().unwrap(),
            "--validate-only",
        ])
        .success();
    let backup = directory.path("backup.nfw");
    simulator
        .cli(&["codeplug", "backup", "--output", backup.to_str().unwrap()])
        .success();
    assert_eq!(fs::read(&backup).unwrap(), image);
    let mut inspect = Command::new(env!("CARGO_BIN_EXE_nictui"));
    inspect.args([
        "codeplug",
        "inspect",
        "--input",
        backup.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(
        Process::spawn(&mut inspect, LIMIT).finish().json()["size"],
        EEPROM_SIZE
    );
    assert_eq!(simulator.state()["programming_disabled"], false);
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn minimal_fixture_is_readable_but_blank_firmware_guard_fails_clearly() {
    let minimal = Simulator::spawn(&["--minimal-eeprom"]);
    let channel = minimal
        .cli(&["channels", "get", "--channel", "1", "--format", "json"])
        .json();
    assert!(channel.to_string().contains("SIM ONLY"));
    let blank = Simulator::spawn(&["--blank-eeprom"]);
    let output = blank.cli(&["settings", "read"]);
    output.failure();
    assert!(output.stderr.contains("unsupported"), "{}", output.stderr);
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn unsupported_write_and_reboot_never_change_the_frozen_image() {
    let directory = Directory::new();
    let (path, image) = fixture(&directory);
    let simulator = Simulator::fixture(&path);
    let output = simulator.cli(&["groups", "set", "--group", "1", "--label", "NOPE"]);
    output.unsupported_programming();
    assert_eq!(simulator.state()["eeprom_live_model"], false);
    let changed = change_only_local_model(&simulator);
    assert_eq!(
        changed["eeprom_source"],
        "local-snapshot-frozen-replay-no-real-radio"
    );
    let mut uart = simulator.socket(simulator.uart);
    // Source protocol rejects reboot silently: no CLI reboot success is asserted.
    uart.write_all(&[0x49]).unwrap();
    let error = uart.read(&mut [0]).unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ));
    ack(&mut uart, 0x01);
    for index in 0..=255u8 {
        assert_eq!(
            block(&mut uart, index).as_slice(),
            &image[usize::from(index) * 32..(usize::from(index) + 1) * 32]
        );
    }
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn activity_capture_and_remote_menu_are_real_cli_sessions_on_owned_radio() {
    let simulator = Simulator::spawn(&[]);
    let capture = simulator
        .cli(&["activity", "monitor", "--duration", "2", "--json"])
        .json();
    assert!(capture.to_string().contains("146.520"), "{capture}");
    simulator.wait_state(|state| state["uart_mode"] == "idle");
    let output = simulator.cli(&[
        "remote",
        "capture",
        "--duration",
        "2",
        "--send",
        "menu",
        "--send-interval-ms",
        "150",
        "--recover-retries",
        "0",
    ]);
    output.success();
    assert!(output.stdout.contains("User Menu"), "{}", output.stdout);
    simulator.wait_state(|state| state["uart_mode"] == "idle");
    let again = simulator.cli(&["remote", "key", "--key", "exit"]);
    again.success();
    assert!(again.stdout.contains("Evidence:"), "{}", again.stdout);
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn gui_hold_lease_uart_watchdog_and_reconnect_are_isolated() {
    let first = Simulator::spawn(&["--incremental-remote-start", "--minimal-eeprom"]);
    let second = Simulator::spawn(&["--blank-eeprom"]);
    assert_ne!(first.uart, second.uart);
    assert_ne!(first.gui, second.gui);
    {
        let mut first_uart = first.socket(first.uart);
        let mut second_uart = second.socket(second.uart);
        assert_eq!(&block(&mut first_uart, 2)[20..28], b"SIM ONLY");
        assert_eq!(block(&mut second_uart, 2), [0xff; 32]);
    }
    first.wait_state(|state| state["uart_connected"] == false);
    second.wait_state(|state| state["uart_connected"] == false);
    let untouched = second.state()["packets"].clone();
    let pressed = first.key(0x0b, "press");
    assert_eq!(pressed["gui_hold"], 0x0b);
    first.wait_state(|state| state["gui_hold"].is_null());
    assert_eq!(second.state()["packets"], untouched);
    let mut uart = first.socket(first.uart);
    ack(&mut uart, 0x01);
    ack(&mut uart, 0x4b);
    ack(&mut uart, 0x4a);
    assert_eq!(first.state()["uart_mode"], "remote");
    // Advance the source model's ten-second watchdog, never a physical radio clock.
    for _ in 0..11 {
        first.http("/api/tick", Some(json!({"milliseconds": 1000})));
    }
    assert_eq!(first.state()["uart_mode"], "idle");
    assert_eq!(second.state()["uart_connected"], false);
    drop(uart);
    first.wait_state(|state| state["uart_connected"] == false);
    let mut uart = first.socket(first.uart);
    ack(&mut uart, 0x01);
    ack(&mut uart, 0x4a);
    assert_eq!(first.state()["uart_mode"], "remote");
    ack(&mut uart, 0x4b);
}

#[test]
fn generated_fixture_passes_actual_cli_offline_import_validation() {
    let directory = Directory::new();
    let (path, image) = fixture(&directory);
    let decoded = nictui::device::inspect_codeplug(&image).unwrap();
    for (section, data) in [
        ("channels", json!(decoded.channels)),
        ("settings", json!(decoded.settings.unwrap())),
        ("scan-presets", json!(decoded.scan_presets)),
        ("band-plan", json!(decoded.band_plans)),
        ("dtmf", json!(decoded.dtmf_presets)),
    ] {
        let input = directory.path(&format!("{section}.json"));
        fs::write(&input, serde_json::to_vec(&data).unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
        command.args([
            section,
            "write",
            "--input",
            input.to_str().unwrap(),
            "--validate-only",
            "--port",
            "sim://127.0.0.1:0",
        ]);
        Process::spawn(&mut command, LIMIT).finish().success();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
    command.args([
        "codeplug",
        "restore",
        "--input",
        path.to_str().unwrap(),
        "--validate-only",
        "--port",
        "sim://127.0.0.1:0",
    ]);
    Process::spawn(&mut command, LIMIT).finish().success();
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn backend_disconnect_during_cli_backup_is_bounded_and_not_successful() {
    let directory = Directory::new();
    let (path, _) = fixture(&directory);
    let mut simulator = Simulator::fixture(&path);
    let backup = directory.path("incomplete.nfw");
    let command =
        simulator.cli_process(&["codeplug", "read", "--output", backup.to_str().unwrap()]);
    simulator.wait_state(|state| state["programming_disabled"] == true);
    simulator.process.stop();
    let output = command.finish();
    output.failure();
    assert!(!backup.exists(), "a failed read must not publish a backup");
}

#[test]
fn hardware_only_commands_parse_without_dispatch_or_device_discovery() {
    // Parsing only: do not scan BLE, enumerate/open serial devices, or flash.
    for arguments in [
        vec!["bluetooth", "scan", "--timeout", "1", "--json"],
        vec!["bluetooth", "doctor", "--name", "SIM-ONLY"],
        vec!["bluetooth", "connect", "--name", "SIM-ONLY"],
        vec!["bluetooth", "disconnect", "--device", "SIM-ONLY"],
        vec![
            "firmware",
            "flash",
            "--input",
            "synthetic.bin",
            "--validate-only",
        ],
        vec![
            "codeplug",
            "restore",
            "--input",
            "synthetic.nfw",
            "--no-reboot",
        ],
        vec![
            "settings",
            "set",
            "--setting",
            "Squelch",
            "--value",
            "3",
            "--no-reboot",
        ],
        vec!["channels", "clear", "--channel", "1", "--reboot"],
        vec![
            "asl",
            "monitor",
            "--port",
            "parse-only-never-opened",
            "--duration",
            "1",
        ],
        vec![
            "remote",
            "live-read",
            "--address",
            "0x0ca0",
            "--blocks",
            "1",
        ],
    ] {
        Cli::try_parse_from(std::iter::once("nictui").chain(arguments)).unwrap();
    }
    assert!(
        Cli::try_parse_from(["nictui", "asl", "monitor", "--port", "sim://127.0.0.1:1"]).is_err()
    );
}

#[test]
fn simulator_device_mutations_never_connect_to_unused_owned_listener() {
    const CHILD_TARGET: &str = "NICTUI_DEVICE_MUTATION_GUARD_CHILD";
    if let Ok(target) = std::env::var(CHILD_TARGET) {
        literal_loopback(target.strip_prefix("sim://").unwrap());
        let directory = Directory::new();
        let (_, image) = fixture(&directory);
        let decoded = nictui::device::inspect_codeplug(&image).unwrap();
        let quiet = |_| panic!("unsupported simulator mutation reported progress");
        macro_rules! rejected {
            ($operation:expr) => {{
                let error = $operation.unwrap_err().to_string();
                assert!(error.contains("Unsupported simulator operation"), "{error}");
                assert!(error.contains("read-only"), "{error}");
            }};
        }
        rejected!(write_codeplug(&target, &image, true, quiet));
        rejected!(write_channels(
            &target,
            &decoded.channels,
            &[],
            Endianness::Big,
            true,
            quiet
        ));
        rejected!(write_channels(
            &target,
            &[],
            &[],
            Endianness::Big,
            true,
            quiet
        ));
        rejected!(write_settings(
            &target,
            decoded.settings.as_ref().unwrap(),
            Endianness::Big,
            true,
            quiet
        ));
        rejected!(write_scan_presets(
            &target,
            &decoded.scan_presets,
            Endianness::Big,
            quiet
        ));
        rejected!(update_scan_preset(&target, &decoded.scan_presets[0], quiet));
        rejected!(write_band_plans(
            &target,
            &decoded.band_plans,
            Endianness::Big,
            quiet
        ));
        rejected!(write_group_labels(&target, &decoded.group_labels, quiet));
        rejected!(write_dtmf_presets(&target, &decoded.dtmf_presets, quiet));
        rejected!(update_dtmf_preset(&target, &decoded.dtmf_presets[0], quiet));
        rejected!(flash_firmware(&target, &[0; 32], quiet));
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "simulator_device_mutations_never_connect_to_unused_owned_listener",
            "--nocapture",
        ])
        .env(
            CHILD_TARGET,
            format!("sim://{}", listener.local_addr().unwrap()),
        );
    // Run API calls in a killable child so regressions cannot enter a 60 s flash wait.
    Process::spawn(&mut command, Duration::from_secs(3))
        .finish()
        .success();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn cli_reboot_writes_and_flash_fail_without_connecting_to_unused_owned_listener() {
    let directory = Directory::new();
    let (codeplug, image) = fixture(&directory);
    let decoded = nictui::device::inspect_codeplug(&image).unwrap();
    let channels = directory.path("channels.json");
    fs::write(&channels, serde_json::to_vec(&decoded.channels).unwrap()).unwrap();
    let firmware = directory.path("synthetic.bin");
    fs::write(&firmware, [0; 32]).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let target = format!("sim://{}", listener.local_addr().unwrap());
    for arguments in [
        vec![
            "channels",
            "write",
            "--input",
            channels.to_str().unwrap(),
            "--reboot",
        ],
        vec!["channels", "clear", "--channel", "1", "--reboot"],
        vec![
            "channels",
            "clear-range",
            "--start",
            "1",
            "--end",
            "2",
            "--reboot",
        ],
        vec!["codeplug", "restore", "--input", codeplug.to_str().unwrap()],
        vec!["firmware", "flash", "--input", firmware.to_str().unwrap()],
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nictui"));
        command.args(arguments).args(["--port", &target]);
        Process::spawn(&mut command, Duration::from_secs(3))
            .finish()
            .unsupported_programming();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn cli_all_programming_mutations_fail_bounded_without_false_success() {
    let directory = Directory::new();
    let (codeplug, image) = fixture(&directory);
    let decoded = nictui::device::inspect_codeplug(&image).unwrap();
    for (name, data) in [
        ("channels", json!(decoded.channels)),
        ("channel", json!(decoded.channels[0])),
        ("settings", json!(decoded.settings.unwrap())),
        ("scan-presets", json!(decoded.scan_presets)),
        ("scan-preset", json!(decoded.scan_presets[0])),
        ("band-plan", json!(decoded.band_plans)),
        ("band", json!(decoded.band_plans[0])),
        ("dtmf", json!(decoded.dtmf_presets)),
        ("dtmf-preset", json!(decoded.dtmf_presets[0])),
    ] {
        fs::write(
            directory.path(&format!("{name}.json")),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
    }
    let firmware = directory.path("synthetic.bin");
    fs::write(&firmware, [0; 32]).unwrap();
    let simulator = Simulator::fixture(&codeplug);
    let channel = directory.path("channel.json");
    let channels = directory.path("channels.json");
    let settings = directory.path("settings.json");
    let scans = directory.path("scan-presets.json");
    let scan = directory.path("scan-preset.json");
    let bands = directory.path("band-plan.json");
    let band = directory.path("band.json");
    let dtmf = directory.path("dtmf.json");
    let dtmf_preset = directory.path("dtmf-preset.json");
    for arguments in [
        vec![
            "channels",
            "write",
            "--input",
            channels.to_str().unwrap(),
            "--reboot",
        ],
        vec![
            "channels",
            "update",
            "--channel",
            "1",
            "--input",
            channel.to_str().unwrap(),
            "--reboot",
        ],
        vec!["channels", "clear", "--channel", "1", "--reboot"],
        vec![
            "channels",
            "clear-range",
            "--start",
            "1",
            "--end",
            "2",
            "--reboot",
        ],
        vec!["settings", "write", "--input", settings.to_str().unwrap()],
        vec!["settings", "set", "--setting", "Squelch", "--value", "3"],
        vec!["bluetooth", "on"],
        vec!["bluetooth", "off"],
        vec!["groups", "set", "--group", "1", "--label", "NOPE"],
        vec!["scan-presets", "write", "--input", scans.to_str().unwrap()],
        vec![
            "scan-presets",
            "update",
            "--index",
            "0",
            "--input",
            scan.to_str().unwrap(),
        ],
        vec!["band-plan", "write", "--input", bands.to_str().unwrap()],
        vec![
            "band-plan",
            "update",
            "--index",
            "0",
            "--input",
            band.to_str().unwrap(),
        ],
        vec!["dtmf", "write", "--input", dtmf.to_str().unwrap()],
        vec![
            "dtmf",
            "update",
            "--index",
            "0",
            "--input",
            dtmf_preset.to_str().unwrap(),
        ],
        vec!["codeplug", "restore", "--input", codeplug.to_str().unwrap()],
        vec!["firmware", "flash", "--input", firmware.to_str().unwrap()],
    ] {
        simulator.wait_state(|state| state["uart_connected"] == false);
        let mut process = simulator.cli_process(&arguments);
        process.deadline = Instant::now() + Duration::from_secs(3);
        process.finish().unsupported_programming();
    }
    simulator.wait_state(|state| state["uart_connected"] == false);
    assert_eq!(simulator.state()["programming_disabled"], false);
    let mut uart = simulator.socket(simulator.uart);
    for index in 0..=255u8 {
        assert_eq!(
            block(&mut uart, index).as_slice(),
            &image[usize::from(index) * 32..(usize::from(index) + 1) * 32]
        );
    }
}

#[test]
#[ignore = "requires external NICFW_SIM_TEST_BIN; only owned ephemeral loopback endpoints"]
fn app_individual_section_reads_complete_against_owned_simulator() {
    let directory = Directory::new();
    let (path, image) = fixture(&directory);
    let expected = nictui::device::inspect_codeplug(&image).unwrap();
    let simulator = Simulator::fixture(&path);
    type ReadAction = (MainTab, fn(&mut App));
    let actions: [ReadAction; 7] = [
        (MainTab::Channels, App::start_read_channels),
        (MainTab::Settings, App::start_read_settings),
        (MainTab::Scanning, App::start_read_presets),
        (MainTab::MemoryGroups, App::start_read_group_labels),
        (MainTab::BandPlan, App::start_read_bandplan),
        (MainTab::DTMF, App::start_read_dtmf),
        (MainTab::Codeplug, App::start_read_codeplug),
    ];
    for (tab, start_read) in actions {
        simulator.wait_state(|state| state["uart_connected"] == false);
        let mut app = App::new();
        app.simulator_mode = true;
        app.protocol_port_name = Some(simulator.uri());
        app.mode = AppMode::Main(tab);
        app.last_main_tab = tab;
        app.last_non_remote_tab = tab;
        start_read(&mut app);
        assert_eq!(
            app.mode,
            AppMode::Reading,
            "{tab:?}: {}",
            app.status_message
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        if tab == MainTab::MemoryGroups {
            let mut events = Vec::new();
            loop {
                let event = app
                    .event_rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .expect("owned simulator group read did not complete");
                if let AppEvent::Error(error) = &event {
                    panic!("owned simulator group read failed: {error}");
                }
                let complete = matches!(&event, AppEvent::ReadGroupLabelsOnlyComplete(_));
                events.push(event);
                if complete {
                    break;
                }
            }
            for event in events {
                app.event_tx.send(event).unwrap();
            }
        }
        while matches!(app.mode, AppMode::Reading) {
            app.update();
            assert!(Instant::now() < deadline, "{tab:?}: {}", app.status_message);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(app.mode, AppMode::Main(tab), "{}", app.status_message);
        assert_eq!(app.progress, 1.0, "{tab:?}");
        match tab {
            MainTab::Channels => {
                assert_eq!(json!(app.channels), json!(expected.channels));
                assert_eq!(app.group_labels, expected.group_labels);
            }
            MainTab::Settings => assert_eq!(json!(app.settings), json!(expected.settings)),
            MainTab::Scanning => {
                assert_eq!(json!(app.scan_presets), json!(expected.scan_presets));
                assert_eq!(app.group_labels, expected.group_labels);
            }
            MainTab::MemoryGroups => {
                assert_eq!(app.group_labels, expected.group_labels);
                assert_eq!(app.last_non_remote_tab, MainTab::MemoryGroups);
                assert_eq!(app.status_message, "Memory group names read complete");
                assert!(app.channels.is_empty());
                assert!(app.settings.is_none());
                assert!(app.scan_presets.is_empty());
                assert!(app.band_plans.is_empty());
                assert!(app.dtmf_presets.is_empty());
            }
            MainTab::BandPlan => assert_eq!(json!(app.band_plans), json!(expected.band_plans)),
            MainTab::DTMF => assert_eq!(json!(app.dtmf_presets), json!(expected.dtmf_presets)),
            MainTab::Codeplug => {
                assert_eq!(app.codeplug_data.as_deref(), Some(image.as_slice()));
                assert_eq!(json!(app.channels), json!(expected.channels));
                assert_eq!(json!(app.settings), json!(expected.settings));
                assert_eq!(json!(app.scan_presets), json!(expected.scan_presets));
                assert_eq!(json!(app.band_plans), json!(expected.band_plans));
                assert_eq!(json!(app.dtmf_presets), json!(expected.dtmf_presets));
                assert_eq!(app.group_labels, expected.group_labels);
                assert!(app.codeplug_snapshot_is_current());
            }
            _ => unreachable!(),
        }
        simulator.wait_state(|state| state["uart_connected"] == false);
        assert_eq!(simulator.state()["programming_disabled"], false);
    }
}
