//! Synthetic 2.52.18 source-baseline fixtures. No radio, ROM or fonts are bundled.
use nictui::{
    app::{App, AppMode, MainTab},
    protocol::RemotePacket,
    remote_display::RadioDisplay,
};
use ratatui::{
    Terminal, backend::TestBackend, buffer::Buffer, layout::Rect, style::Color, widgets::Widget,
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

const SIZES: [(u16, u16); 6] = [
    (80, 24),
    (120, 36),
    (160, 48),
    (200, 64),
    (220, 80),
    (240, 80),
];
// Full source snapshot with synthetic CH001 names, battery and calibration data.
const IDLE: &str = "
650000808000000000650016802d82aa00006400003a1f7d82aa342e3257
00000064012616c5ff82aa53494d20434830312020202000000064811729
dfa782aa2031343500000064025729dfa782aa3530303030000000655536
0202dfa7000064000016d4fd82aa43483030310000006400001fdbfe82aa
2020202020000000660d012ccf7f82aa000064010c2c1f7d82aa20000000
64001d3a9f2e82aa202020200000006401371f9f2e82aa20200000006401
4e1f9f2e82aa20200000006401671f9f2e82aa20464d0000006401261f9f
2e82aa57000000650053802d00000000640000771f7d0000342e32570000
0064012653c5ff000053494d20434830312020202000000064811766dfa7
00002034333300000064025766dfa7000035303030300000006555730202
dfa7000064000053d4fd000043483030310000006400005cdbfe00002020
202020000000660d0169cf7f0000000064010c691f7d0000200000006400
1d779f2e0000202020200000006401375c9f2e0000202000000064014e5c
9f2e000020200000006401675c9f2e000020464d0000006401265c9f2e00
0057000000690b67000000006500028008000000006800000000660d0146
9f2e00000000660d0b469f2e00000000660d1546dbfe00000000660d1f46
dbfe00000000660f3346dbfe00000000640129465f4a000020000000690b
6700000000650002800800000000680000000070000064006746c73f0000
382e3256000000";

fn idle_bytes() -> Vec<u8> {
    let hex: String = IDLE.split_whitespace().collect();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

// Independent fixture decoder, not a second production protocol implementation.
fn packets(mut bytes: &[u8]) -> Vec<RemotePacket> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        let id = bytes[0];
        bytes = &bytes[1..];
        let word = |i| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let (packet, count) = match id {
            0x64 => {
                let length = bytes[7..].iter().position(|b| *b == 0).unwrap();
                (
                    RemotePacket::DisplayText {
                        font_size: bytes[0],
                        x: bytes[1],
                        y: bytes[2],
                        fg_color: word(3),
                        bg_color: word(5),
                        text: bytes[7..7 + length]
                            .iter()
                            .map(|b| if *b == b'@' { ' ' } else { char::from(*b) })
                            .collect(),
                    },
                    8 + length,
                )
            }
            0x65 => (
                RemotePacket::DrawRectangle {
                    x: bytes[0],
                    y: bytes[1],
                    width: bytes[2],
                    height: bytes[3],
                    color: word(4),
                },
                6,
            ),
            0x66 => (
                RemotePacket::DrawSymbol {
                    symbol_id: bytes[0],
                    x: bytes[1],
                    y: bytes[2],
                    fg_color: word(3),
                    bg_color: word(5),
                },
                7,
            ),
            0x67 => (
                RemotePacket::SignalStrength {
                    strength: bytes[0],
                    mode: bytes[1],
                    battery: 0,
                },
                2,
            ),
            0x68 => (
                RemotePacket::NoiseLevel {
                    level: bytes[0],
                    mode: bytes[1],
                },
                2,
            ),
            0x69 => (
                RemotePacket::SignalBarPos {
                    y: bytes[0],
                    aux: 0,
                },
                1,
            ),
            0x70..=0x7f => (
                RemotePacket::SmallStatus {
                    id,
                    value1: 0,
                    value2: 0,
                },
                0,
            ),
            0 | 0x4a => continue,
            _ => panic!("unknown fixture opcode {id:02x}"),
        };
        bytes = &bytes[count..];
        result.push(packet);
    }
    result
}

fn rgb(word: u16) -> Color {
    Color::Rgb(
        ((word & 31) << 3) as u8,
        (((word >> 5) & 63) << 2) as u8,
        ((word >> 11) << 3) as u8,
    )
}

fn assert_native_projection(display: &RadioDisplay) {
    for columns in [128, 256] {
        let area = Rect::new(0, 0, columns, columns / 2);
        let mut buffer = Buffer::empty(area);
        Widget::render(display, area, &mut buffer);
        for y in 0..columns {
            for x in 0..columns {
                let cell = &buffer[(x, y / 2)];
                let actual = match cell.symbol() {
                    " " => cell.bg,
                    "▄" if y % 2 == 0 => cell.bg,
                    "▄" => cell.fg,
                    "▀" if y % 2 == 0 => cell.fg,
                    "▀" => cell.bg,
                    other => panic!("unexpected LCD glyph {other:?}"),
                };
                assert_eq!(
                    actual,
                    display.pixel(
                        usize::from(x) * 128 / usize::from(columns),
                        usize::from(y) * 128 / usize::from(columns)
                    )
                );
            }
        }
    }
}

fn render_sizes(display: &RadioDisplay, name: &str, source_packets: &[RemotePacket]) {
    let mut app = App::new();
    app.mode = AppMode::Main(MainTab::Remote);
    app.simulator_mode = true;
    app.remote_active = true;
    app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Armed;
    for packet in source_packets {
        app.event_tx
            .send(nictui::app::AppEvent::RemotePacket(packet.clone()))
            .unwrap();
        assert!(app.update());
    }
    app.remote_screen.display = display.clone();
    app.remote_screen.has_display_updates = true;
    for diagnostics in [false, true] {
        for (width, height) in SIZES {
            app.remote_screen.show_diagnostics = diagnostics;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| nictui::ui::ui(f, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|cell| matches!(cell.symbol(), "▀" | "▄")),
                "{name}, {width}x{height}"
            );
            if !diagnostics {
                let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                assert!(text.contains("PTT"), "{name}, {width}x{height}");
                if let Some(out) = std::env::var_os("NICTUI_REMOTE_VISUAL_OUT") {
                    export_svg(
                        buffer,
                        PathBuf::from(out).join(format!("{name}-{width}x{height}.svg")),
                    );
                    let blocks: Vec<_> = (0..height)
                        .flat_map(|y| (0..width).map(move |x| (x, y)))
                        .filter(|p| matches!(buffer[*p].symbol(), "▀" | "▄"))
                        .collect();
                    let left = blocks.iter().map(|p| p.0).min().unwrap();
                    let top = blocks.iter().map(|p| p.1).min().unwrap();
                    let right = blocks.iter().map(|p| p.0).max().unwrap() + 1;
                    let bottom = blocks.iter().map(|p| p.1).max().unwrap() + 1;
                    let mut crop = Buffer::empty(Rect::new(0, 0, right - left, bottom - top));
                    for y in top..bottom {
                        for x in left..right {
                            crop[(x - left, y - top)] = buffer[(x, y)].clone();
                        }
                    }
                    export_svg(
                        &crop,
                        PathBuf::from(std::env::var_os("NICTUI_REMOTE_VISUAL_OUT").unwrap())
                            .join(format!("{name}-{width}x{height}-lcd.svg")),
                    );
                }
            }
        }
    }
}

fn export_svg(buffer: &Buffer, path: PathBuf) {
    use std::fmt::Write;
    let color = |c| match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#111923".into(),
    };
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\">",
        buffer.area.width * 10,
        buffer.area.height * 20
    );
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let c = &buffer[(x, y)];
            let (px, py) = (x * 10, y * 20);
            write!(
                svg,
                "<rect x=\"{px}\" y=\"{py}\" width=\"10\" height=\"20\" fill=\"{}\"/>",
                color(c.bg)
            )
            .unwrap();
            if c.symbol() == "▀" {
                write!(
                    svg,
                    "<rect x=\"{px}\" y=\"{py}\" width=\"10\" height=\"10\" fill=\"{}\"/>",
                    color(c.fg)
                )
                .unwrap();
            } else if c.symbol() == "▄" {
                write!(
                    svg,
                    "<rect x=\"{px}\" y=\"{}\" width=\"10\" height=\"10\" fill=\"{}\"/>",
                    py + 10,
                    color(c.fg)
                )
                .unwrap();
            } else if c.symbol() != " " {
                let escaped = c
                    .symbol()
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                write!(svg,"<text x=\"{px}\" y=\"{}\" font-family=\"Menlo,monospace\" font-size=\"16\" fill=\"{}\">{escaped}</text>",py+16,color(c.fg)).unwrap();
            }
        }
    }
    svg.push_str("</svg>");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, svg).unwrap();
}

#[test]
fn owned_source_snapshot_preserves_native_geometry_and_raster_frequency() {
    let bytes = idle_bytes();
    assert_eq!(bytes.len(), 547);
    let mut display = RadioDisplay::default();
    for packet in packets(&bytes) {
        display.apply(&packet);
    }
    for y in [54, 115] {
        for x in [85, 86] {
            assert_eq!(display.pixel(x, y), rgb(0xa7df));
        }
    }
    assert_eq!(display.pixel(127, 30), rgb(0xaa82));
    assert_native_projection(&display);
    render_sizes(&display, "approximate-source", &packets(&bytes));
    for columns in [28, 52, 76, 108, 128] {
        let area = Rect::new(0, 0, columns, columns.div_ceil(2));
        let mut buffer = Buffer::empty(area);
        Widget::render(&display, area, &mut buffer);
        assert!(
            buffer.content.iter().all(|cell| cell.symbol() == "▀"),
            "the entire LCD, including small labels, must share one raster projection"
        );
    }
}

fn http(port: u16, path: &str, body: Option<&str>) -> serde_json::Value {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let method = if body.is_some() { "POST" } else { "GET" };
    let body = body.unwrap_or("");
    write!(socket,"{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
    let mut bytes = Vec::new();
    socket.read_to_end(&mut bytes).unwrap();
    let boundary = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert!(
        bytes.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes[boundary..]).unwrap()
}

struct OwnedSim(Child);
impl Drop for OwnedSim {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "owned simulator only: set NICFW_SIM_BIN, NICTUI_NATIVE_SOURCE and NICTUI_NATIVE_ROM"]
fn owned_tcp_stream_matches_http_native_frame_and_simulated_actions() {
    let source = PathBuf::from(std::env::var_os("NICTUI_NATIVE_SOURCE").unwrap());
    let rom = PathBuf::from(std::env::var_os("NICTUI_NATIVE_ROM").unwrap());
    let port = || {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    let (http_port, tcp_port) = (port(), port());
    assert_ne!(http_port, tcp_port);
    let _sim = OwnedSim(
        Command::new(std::env::var_os("NICFW_SIM_BIN").unwrap())
            .args([
                "--http-port",
                &http_port.to_string(),
                "--radio-port",
                &tcp_port.to_string(),
                "--font-source",
            ])
            .arg(&source)
            .arg("--font-rom")
            .arg(&rom)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let mut socket = (0..100)
        .find_map(|_| match TcpStream::connect(("127.0.0.1", tcp_port)) {
            Ok(s) => Some(s),
            Err(_) => {
                std::thread::sleep(Duration::from_millis(20));
                None
            }
        })
        .expect("owned simulator did not start");
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    socket.write_all(&[0x4a]).unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        match socket.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!(bytes.first(), Some(&0x4a));
    let startup = packets(&bytes);
    for channel in ["CH001", "CH100"] {
        assert!(
            startup.iter().any(|packet| matches!(packet,
            RemotePacket::DisplayText { text, .. } if text == channel)),
            "default model must display its two actual memory slots: {channel}"
        );
    }
    let mut display = RadioDisplay::with_native_fonts(&source, &rom).unwrap();
    display.set_meter_context(0, false, 0); // Known owned simulator defaults, absent from UART.
    for packet in startup {
        display.apply(&packet);
    }
    let compare = |display: &RadioDisplay, snapshot: &serde_json::Value| {
        assert_eq!(snapshot["reference_firmware"], "2.52.18");
        assert_eq!(snapshot["framebuffer"]["complete"], true);
        let pixels = snapshot["framebuffer"]["pixels"].as_array().unwrap();
        assert_eq!(pixels.len(), 128 * 128);
        for (i, pixel) in pixels.iter().enumerate() {
            assert_eq!(
                display.pixel(i % 128, i / 128),
                rgb(pixel.as_u64().unwrap() as u16),
                "native pixel {},{}",
                i % 128,
                i / 128
            );
        }
    };
    compare(&display, &http(http_port, "/api/state", None));
    assert_native_projection(&display);
    render_sizes(&display, "native-source", &packets(&bytes));
    // Each action is confined to this owned process. PTT is never sent to hardware.
    for key in 1..=0x13 {
        let wire = match key {
            1..=0x10 => key + 0x7f,
            0x11 => 0x91,
            0x12 => 0x92,
            0x13 => 0x90,
            _ => unreachable!(),
        };
        socket.write_all(&[wire]).unwrap();
        std::thread::sleep(Duration::from_millis(140));
        let held = http(http_port, "/api/state", None);
        if key == 0x13 {
            assert_eq!(held["transmitting"], true, "owned simulated PTT hold");
        }
        socket.write_all(&[0xff]).unwrap();
        std::thread::sleep(Duration::from_millis(140));
        let snapshot = http(http_port, "/api/state", None);
        assert_eq!(snapshot["transmitting"], false, "key {key}");
        for (edge, snapshot) in [("held", held), ("released", snapshot)] {
            let mut frame = RadioDisplay::with_native_fonts(&source, &rom).unwrap();
            frame.set_meter_context(0, false, 0);
            let source_packets: Vec<_> = snapshot["packets"]
                .as_array()
                .unwrap()
                .iter()
                .map(json_packet)
                .collect();
            for packet in &source_packets {
                frame.apply(packet);
            }
            compare(&frame, &snapshot);
            render_sizes(&frame, &format!("key-{key:02x}-{edge}"), &source_packets);
        }
        socket.write_all(&[0x8d]).unwrap();
        std::thread::sleep(Duration::from_millis(120));
        socket.write_all(&[0xff]).unwrap();
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn json_packet(p: &serde_json::Value) -> RemotePacket {
    let b = |name: &str| p[name].as_u64().unwrap() as u8;
    let w = |name: &str| p[name].as_u64().unwrap() as u16;
    match p["type"].as_str().unwrap() {
        "DisplayText" => RemotePacket::DisplayText {
            font_size: b("font_size"),
            x: b("x"),
            y: b("y"),
            fg_color: w("fg_color"),
            bg_color: w("bg_color"),
            text: p["text"].as_str().unwrap().into(),
        },
        "DrawRectangle" => RemotePacket::DrawRectangle {
            x: b("x"),
            y: b("y"),
            width: b("width"),
            height: b("height"),
            color: w("color"),
        },
        "DrawSymbol" => RemotePacket::DrawSymbol {
            symbol_id: b("symbol_id"),
            x: b("x"),
            y: b("y"),
            fg_color: w("fg_color"),
            bg_color: w("bg_color"),
        },
        "SignalStrength" => RemotePacket::SignalStrength {
            strength: b("strength"),
            mode: b("mode"),
            battery: 0,
        },
        "NoiseLevel" => RemotePacket::NoiseLevel {
            level: b("level"),
            mode: b("mode"),
        },
        "SignalBarPos" => RemotePacket::SignalBarPos { y: b("y"), aux: 0 },
        "SmallStatus" => RemotePacket::SmallStatus {
            id: b("id"),
            value1: 0,
            value2: 0,
        },
        other => panic!("unknown packet {other}"),
    }
}

#[test]
#[ignore = "private unchanged-C goldens: set NICTUI_NATIVE_SOURCE, NICTUI_NATIVE_ROM and NICTUI_FONT_ORACLE"]
fn native_fonts_match_private_unchanged_c_cases() {
    let source = PathBuf::from(std::env::var_os("NICTUI_NATIVE_SOURCE").unwrap());
    let rom = PathBuf::from(std::env::var_os("NICTUI_NATIVE_ROM").unwrap());
    let goldens: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("NICTUI_FONT_ORACLE").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(goldens["rom_address_bias"], 1);
    let cases = goldens["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 42);
    for case in cases {
        let mut display = RadioDisplay::with_native_fonts(&source, &rom).unwrap();
        let wire: Vec<u8> = case["wire_packets"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| {
                p.as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b.as_u64().unwrap() as u8)
            })
            .collect();
        let source_packets = packets(&wire);
        for packet in &source_packets {
            display.apply(packet);
        }
        for (i, pixel) in case["pixels"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                display.pixel(i % 128, i / 128),
                rgb(pixel.as_u64().unwrap() as u16),
                "{} at {},{}",
                case["name"],
                i % 128,
                i / 128
            );
        }
    }
}

#[test]
#[ignore = "private source scene streams and C font executable: set NICTUI_SOURCE_SCENES, NICTUI_C_FONT_ORACLE and native font paths"]
fn nine_full_source_scenes_match_independent_c_glyph_composition() {
    let source = PathBuf::from(std::env::var_os("NICTUI_NATIVE_SOURCE").unwrap());
    let rom = PathBuf::from(std::env::var_os("NICTUI_NATIVE_ROM").unwrap());
    let scenes = PathBuf::from(std::env::var_os("NICTUI_SOURCE_SCENES").unwrap());
    let oracle = std::env::var_os("NICTUI_C_FONT_ORACLE").unwrap();
    for (name, hash) in [
        ("idle-a", 0xa5f45ebcf50d768eu64),
        ("idle-b", 0x2cb738bfb90a34ae),
        ("receive-a", 0x832db8f097912eed),
        ("single-a", 0xf1bd8156a989ad2d),
        ("single-b", 0xb58fe07768a9a67f),
        ("hybrid-b", 0x57dd5220d7a58b5d),
        ("vfo-a", 0xf50469d8a2ec7d3a),
        ("flags", 0xdf3ceffb23c9e0c5),
        ("tx-a", 0x3912c17e9728c2b8),
    ] {
        let wire = std::fs::read(scenes.join(format!("{name}.bin"))).unwrap();
        assert_eq!(
            wire.iter()
                .fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(*b))
                    .wrapping_mul(0x100000001b3)),
            hash,
            "fixture drift: {name}"
        );
        let mut display = RadioDisplay::with_native_fonts(&source, &rom).unwrap();
        display.set_meter_context(0, false, 0);
        let mut expected = vec![0u16; 128 * 128];
        let mut meter_y = 11;
        let source_packets = packets(&wire);
        for packet in &source_packets {
            let mut fill = |x: usize, y: usize, w: usize, h: usize, c: u16| {
                for yy in y.min(128)..(y + h).min(128) {
                    for xx in x.min(128)..(x + w).min(128) {
                        expected[yy * 128 + xx] = c;
                    }
                }
            };
            let font_call = match &packet {
                RemotePacket::DisplayText {
                    font_size,
                    x,
                    y,
                    fg_color,
                    bg_color,
                    text,
                } => Some((
                    "text",
                    *font_size,
                    *x,
                    *y,
                    *fg_color,
                    *bg_color,
                    text.as_str(),
                )),
                RemotePacket::DrawSymbol {
                    symbol_id,
                    x,
                    y,
                    fg_color,
                    bg_color,
                } => Some(("symbol", *symbol_id, *x, *y, *fg_color, *bg_color, "")),
                RemotePacket::DrawRectangle {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => {
                    if u16::from(*width) * u16::from(*height) < 0x8000 {
                        fill(
                            usize::from(*x),
                            usize::from(*y),
                            usize::from(*width),
                            usize::from(*height),
                            *color,
                        );
                    }
                    None
                }
                RemotePacket::SignalBarPos { y, .. } => {
                    meter_y = usize::from(*y);
                    None
                }
                RemotePacket::SignalStrength { strength, mode, .. } => {
                    // Known segment/no-FM/black settings of these source fixtures.
                    let rx = mode & 1 == 0;
                    let level = if rx {
                        usize::from(*strength) * 27 / 16
                    } else {
                        usize::from(*strength)
                    };
                    for i in (0..120).step_by(2) {
                        let color = if i >= level {
                            0x8210
                        } else if rx {
                            i.min(31) as u16 | ((if i < 90 { 61 } else { 240 - 2 * i }) as u16) << 5
                        } else {
                            0x1f
                        };
                        fill(i + 4, meter_y, 1, 6, color);
                    }
                    None
                }
                RemotePacket::NoiseLevel { level, mode } => {
                    let width = usize::from(*level).min(120);
                    fill(
                        4,
                        meter_y + 7,
                        width,
                        1,
                        if *mode == 0 { 0xf9ad } else { 0x07e0 },
                    );
                    fill(width, meter_y + 7, 120 - width, 1, 0);
                    None
                }
                _ => None,
            };
            if let Some((kind, id, x, y, fg, bg, text)) = font_call {
                // Original C font/LCD code draws into a sentinel canvas. Only
                // written pixels are composed; no glyph logic is shared here.
                let output = Command::new(&oracle)
                    .arg(&rom)
                    .args([
                        kind,
                        &id.to_string(),
                        &x.to_string(),
                        &y.to_string(),
                        &fg.to_string(),
                        &bg.to_string(),
                        "4660",
                        text,
                    ])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(output.stdout.len(), 128 * 128 * 2);
                for (i, bytes) in output.stdout.chunks_exact(2).enumerate() {
                    let color = u16::from_le_bytes([bytes[0], bytes[1]]);
                    if color != 0x1234 {
                        expected[i] = color;
                    }
                }
            }
            display.apply(packet);
        }
        for (i, pixel) in expected.into_iter().enumerate() {
            assert_eq!(
                display.pixel(i % 128, i / 128),
                rgb(pixel),
                "{name} pixel {},{}",
                i % 128,
                i / 128
            );
        }
        assert_native_projection(&display);
        render_sizes(&display, name, &source_packets);
    }
}
