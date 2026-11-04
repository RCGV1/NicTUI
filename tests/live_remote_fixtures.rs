use anyhow::{Context, Result, bail, ensure};
use nictui::{protocol::RemotePacket, remote_display::RadioDisplay};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fmt::Write as _, path::Path};

const WIDTHS: [u16; 6] = [24, 33, 57, 74, 106, 128];

#[derive(Debug)]
enum Event {
    Packet {
        elapsed_ms: u64,
        packet: RemotePacket,
    },
    Diagnostic {
        kind: String,
        message: String,
    },
}

fn keys(value: &Value, allowed: &[&str]) -> Result<()> {
    let object = value.as_object().context("expected a JSON object")?;
    for key in object.keys() {
        ensure!(allowed.contains(&key.as_str()), "unexpected field {key:?}");
    }
    Ok(())
}

fn number(value: &Value, key: &str) -> Result<u64> {
    value[key]
        .as_u64()
        .with_context(|| format!("missing/invalid unsigned {key}"))
}

fn byte(value: &Value, key: &str) -> Result<u8> {
    u8::try_from(number(value, key)?).with_context(|| format!("{key} exceeds u8"))
}

fn word(value: &Value, key: &str) -> Result<u16> {
    u16::try_from(number(value, key)?).with_context(|| format!("{key} exceeds u16"))
}

fn string(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("missing/invalid {key}"))
}

// This is a capture adapter, not a second wire parser. Never infer omitted fields.
fn packet(value: &Value) -> Result<RemotePacket> {
    let kind = string(value, "kind")?;
    let fields: &[&str] = match kind.as_str() {
        "DisplayText" => &[
            "kind",
            "font_size",
            "x",
            "y",
            "fg_color",
            "bg_color",
            "text",
        ],
        "DrawRectangle" => &["kind", "x", "y", "width", "height", "color"],
        "DrawSymbol" => &["kind", "symbol_id", "x", "y", "fg_color", "bg_color"],
        "SignalStrength" => &["kind", "strength", "mode", "battery"],
        "NoiseLevel" => &["kind", "level", "mode"],
        "SignalBarPos" => &["kind", "y", "aux"],
        "SmallStatus" => &["kind", "id", "value1", "value2"],
        "UnknownFrame" => &["kind", "opcode", "payload"],
        _ => bail!("unsupported packet kind {kind:?}"),
    };
    keys(value, fields)?;
    Ok(match kind.as_str() {
        "DisplayText" => RemotePacket::DisplayText {
            font_size: byte(value, "font_size")?,
            x: byte(value, "x")?,
            y: byte(value, "y")?,
            fg_color: word(value, "fg_color")?,
            bg_color: word(value, "bg_color")?,
            text: string(value, "text")?,
        },
        "DrawRectangle" => RemotePacket::DrawRectangle {
            x: byte(value, "x")?,
            y: byte(value, "y")?,
            width: byte(value, "width")?,
            height: byte(value, "height")?,
            color: word(value, "color")?,
        },
        "DrawSymbol" => RemotePacket::DrawSymbol {
            symbol_id: byte(value, "symbol_id")?,
            x: byte(value, "x")?,
            y: byte(value, "y")?,
            fg_color: word(value, "fg_color")?,
            bg_color: word(value, "bg_color")?,
        },
        "SignalStrength" => RemotePacket::SignalStrength {
            strength: byte(value, "strength")?,
            mode: byte(value, "mode")?,
            battery: byte(value, "battery")?,
        },
        "NoiseLevel" => RemotePacket::NoiseLevel {
            level: byte(value, "level")?,
            mode: byte(value, "mode")?,
        },
        "SignalBarPos" => RemotePacket::SignalBarPos {
            y: byte(value, "y")?,
            aux: byte(value, "aux")?,
        },
        "SmallStatus" => RemotePacket::SmallStatus {
            id: byte(value, "id")?,
            value1: byte(value, "value1")?,
            value2: byte(value, "value2")?,
        },
        "UnknownFrame" => RemotePacket::UnknownFrame {
            opcode: byte(value, "opcode")?,
            payload: value["payload"]
                .as_array()
                .context("missing/invalid payload")?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .context("invalid payload byte")
                })
                .collect::<Result<_>>()?,
        },
        _ => unreachable!(),
    })
}

fn parse(input: &str) -> Result<Vec<(usize, Event)>> {
    let mut events = Vec::new();
    let mut last_time = 0;
    for (index, line) in input.lines().enumerate() {
        let line_number = index + 1;
        let event = (|| -> Result<Event> {
            ensure!(!line.trim().is_empty(), "blank JSONL record");
            let value: Value = serde_json::from_str(line)?;
            let kind = string(&value, "kind")?;
            if kind == "packet" {
                keys(&value, &["elapsed_ms", "kind", "packet"])?;
                let elapsed_ms = number(&value, "elapsed_ms")?;
                ensure!(elapsed_ms >= last_time, "elapsed_ms moved backwards");
                last_time = elapsed_ms;
                Ok(Event::Packet {
                    elapsed_ms,
                    packet: packet(&value["packet"])?,
                })
            } else {
                // The private library serializer also emits delta diagnostics.
                ensure!(
                    ["log", "status", "phase", "delta"].contains(&kind.as_str()),
                    "unsupported event kind {kind:?}"
                );
                keys(&value, &["elapsed_ms", "kind", "message"])?;
                if value.get("elapsed_ms").is_some() {
                    let elapsed_ms = number(&value, "elapsed_ms")?;
                    ensure!(elapsed_ms >= last_time, "elapsed_ms moved backwards");
                    last_time = elapsed_ms;
                }
                Ok(Event::Diagnostic {
                    kind,
                    message: string(&value, "message")?,
                })
            }
        })()
        .with_context(|| format!("capture line {line_number}"))?;
        events.push((line_number, event));
    }
    ensure!(!events.is_empty(), "capture has no records");
    Ok(events)
}

fn bounds(x: usize, y: usize, width: usize, height: usize) -> Result<()> {
    if width > 0 && height > 0 {
        ensure!(
            x + width <= 128 && y + height <= 128,
            "native clipping: ({x},{y}) {width}x{height} exceeds 128x128"
        );
    }
    Ok(())
}

fn geometry(packet: &RemotePacket, signal_y: &mut usize) -> Result<()> {
    match packet {
        RemotePacket::DisplayText {
            font_size,
            x,
            y,
            text,
            ..
        } => {
            ensure!(!text.contains('\0'), "embedded NUL in decoded text");
            ensure!(
                text.chars().all(|c| c.is_ascii_graphic() || c == ' '),
                "unsupported glyph/control character in {text:?}"
            );
            let slot = if *font_size == 129 {
                3
            } else {
                usize::from(*font_size & 3)
            };
            let (width, height) = [(6, 8), (8, 8), (8, 16), (16, 16)][slot];
            bounds(
                usize::from(*x),
                usize::from(*y),
                text.chars().count() * width,
                height,
            )
        }
        RemotePacket::DrawRectangle {
            x,
            y,
            width,
            height,
            ..
        } => bounds((*x).into(), (*y).into(), (*width).into(), (*height).into()),
        RemotePacket::DrawSymbol {
            symbol_id, x, y, ..
        } => {
            // Current renderer has 17 verified icon masks (IDs 0 through 16).
            ensure!(*symbol_id < 17, "unknown renderer symbol {symbol_id}");
            bounds((*x).into(), (*y).into(), 8, 8)
        }
        RemotePacket::SignalBarPos { y, .. } => {
            *signal_y = usize::from(*y);
            Ok(())
        }
        RemotePacket::SignalStrength { .. } => bounds(4, *signal_y, 120, 6),
        RemotePacket::NoiseLevel { .. } => bounds(4, *signal_y + 6, 120, 1),
        RemotePacket::UnknownFrame { opcode, payload } => {
            bail!("unknown frame {opcode:02X}: {payload:02X?}")
        }
        RemotePacket::SmallStatus { .. } => Ok(()),
    }
}

fn render(display: &RadioDisplay, width: u16) -> Buffer {
    let area = Rect::new(0, 0, width, width.div_ceil(2));
    let mut buffer = Buffer::empty(area);
    Widget::render(display, area, &mut buffer);
    buffer
}

fn guarded(display: &RadioDisplay, width: u16) -> Result<()> {
    let area = Rect::new(2, 2, width, width.div_ceil(2));
    let mut buffer = Buffer::filled(
        Rect::new(0, 0, width + 4, area.height + 4),
        ratatui::buffer::Cell::new("!")
            .set_style(Style::default().fg(Color::Red))
            .clone(),
    );
    let before = buffer.clone();
    Widget::render(display, area, &mut buffer);
    let expected = render(display, width);
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            if area.contains((x, y).into()) {
                ensure!(
                    buffer[(x, y)] == expected[(x - 2, y - 2)],
                    "offset rendering differs at {x},{y}, width {width}"
                );
            } else {
                ensure!(
                    buffer[(x, y)] == before[(x, y)],
                    "render escaped canvas at {x},{y}, width {width}"
                );
            }
        }
    }
    Ok(())
}

fn rgb(color: Color) -> Result<String> {
    match color {
        Color::Rgb(r, g, b) => Ok(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Reset | Color::Black => Ok("#000000".into()),
        _ => bail!("unexpected non-RGB canvas color {color:?}"),
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn svg(buffer: &Buffer) -> Result<String> {
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">",
        buffer.area.width * 8,
        buffer.area.height * 16,
        buffer.area.width * 8,
        buffer.area.height * 16
    );
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let (px, py) = (x * 8, y * 16);
            write!(
                out,
                "<rect x=\"{px}\" y=\"{py}\" width=\"8\" height=\"16\" fill=\"{}\"/>",
                rgb(cell.bg)?
            )?;
            let fg = rgb(cell.fg)?;
            match cell.symbol() {
                " " => {}
                "\u{2580}" | "\u{2584}" | "\u{2588}" => {
                    let dy = if cell.symbol() == "\u{2584}" { 8 } else { 0 };
                    let height = if cell.symbol() == "\u{2588}" { 16 } else { 8 };
                    write!(
                        out,
                        "<rect x=\"{px}\" y=\"{}\" width=\"8\" height=\"{height}\" fill=\"{fg}\"/>",
                        py + dy
                    )?;
                }
                text => {
                    ensure!(
                        text.chars().count() == 1 && text.is_ascii(),
                        "unexpected canvas glyph {text:?}"
                    );
                    write!(
                        out,
                        "<text x=\"{px}\" y=\"{}\" fill=\"{fg}\" font-family=\"Menlo,monospace\" font-size=\"13\" textLength=\"8\" lengthAdjust=\"spacingAndGlyphs\">{}</text>",
                        py + 12,
                        xml(text)
                    )?;
                }
            }
        }
    }
    out.push_str("</svg>");
    Ok(out)
}

fn full_clear(packet: &RemotePacket) -> bool {
    matches!(
        packet,
        RemotePacket::DrawRectangle {
            x: 0,
            y: 0,
            width: 128,
            height: 128,
            ..
        }
    )
}

fn replay(input: &str, output: &Path, snapshots: &BTreeSet<usize>) -> Result<()> {
    let events = parse(input)?;
    let last_packet_line = events
        .iter()
        .rev()
        .find_map(|(line, event)| matches!(event, Event::Packet { .. }).then_some(*line));
    for requested in snapshots {
        ensure!(
            events
                .iter()
                .any(|(line, event)| line == requested && matches!(event, Event::Packet { .. })),
            "snapshot line {requested} is not a packet"
        );
    }
    let mut display = RadioDisplay::default();
    let mut signal_y = 0;
    let mut issues = Vec::new();
    let mut diagnostics = Vec::new();
    let mut artifacts = Vec::new();
    let mut terminals = WIDTHS
        .into_iter()
        .map(|w| Terminal::new(TestBackend::new(w, w.div_ceil(2))).map(|t| (w, t)))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut count = 0;
    let mut display_packets = 0;
    let mut full_canvas_fill_seen = false;
    for (line, event) in events {
        let Event::Packet { elapsed_ms, packet } = event else {
            if let Event::Diagnostic { kind, message } = event {
                diagnostics.push(json!({"line": line, "kind": kind, "message": message}));
            }
            continue;
        };
        count += 1;
        if matches!(
            &packet,
            RemotePacket::DisplayText { .. }
                | RemotePacket::DrawRectangle { .. }
                | RemotePacket::DrawSymbol { .. }
                | RemotePacket::SignalStrength { .. }
                | RemotePacket::NoiseLevel { .. }
        ) {
            display_packets += 1;
        }
        full_canvas_fill_seen |= full_clear(&packet);
        if let Err(error) = geometry(&packet, &mut signal_y) {
            issues.push(format!("line {line}: {error:#}"));
        }
        display.apply(&packet);
        for (width, terminal) in &mut terminals {
            terminal.draw(|frame| {
                let area = frame.area();
                display.render(frame, area);
            })?;
            let expected = render(&display, *width);
            ensure!(
                terminal.backend().buffer() == &expected,
                "stale backend cells at line {line}, width {width}"
            );
            guarded(&display, *width).with_context(|| format!("line {line}"))?;
            if full_clear(&packet) {
                let mut fresh = RadioDisplay::default();
                fresh.apply(&packet);
                ensure!(
                    expected == render(&fresh, *width),
                    "stale semantic overlay after full clear at line {line}, width {width}"
                );
            }
            if display_packets > 0 && (Some(line) == last_packet_line || snapshots.contains(&line))
            {
                let name = format!("line-{line:06}-w{width}.svg");
                std::fs::write(output.join(&name), svg(&expected)?)?;
                artifacts.push(
                    json!({"line": line, "elapsed_ms": elapsed_ms, "columns": width, "svg": name}),
                );
            }
        }
    }
    let manifest = json!({
        "packet_count": count, "widths": WIDTHS, "artifacts": artifacts,
        "issues": issues, "diagnostics": diagnostics,
        "outcome": if count == 0 { "diagnostic_only_no_screen_data" }
            else if display_packets == 0 { "telemetry_only_no_screen_data" }
            else if !full_canvas_fill_seen { "partial_display_only" }
            else { "display_updates_not_independently_verified" },
        "display_packet_count": display_packets,
        "full_canvas_fill_seen": full_canvas_fill_seen,
        "initial_state": "RadioDisplay default black; uncaptured pixels are unknown, not evidence of a black radio screen. No synthetic menus or channel baseline were applied.",
        "scope": "Ordered RadioDisplay canvas replay, not full-app freshness/control state.",
        "review_required": "Inspect diagnostics and real-screen checkpoints for capture errors and stale partial overlays. No independent device-screen oracle is available."
    });
    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    ensure!(
        issues.is_empty(),
        "capture failed validation: {}",
        issues.join("; ")
    );
    Ok(())
}

#[test]
#[ignore = "requires an explicit private offline capture path; no device connection"]
fn replay_private_capture() -> Result<()> {
    let input = std::env::var_os("NICTUI_REMOTE_FIXTURE").context("set NICTUI_REMOTE_FIXTURE")?;
    let output =
        std::env::var_os("NICTUI_REMOTE_REPLAY_OUT").context("set NICTUI_REMOTE_REPLAY_OUT")?;
    let private = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".live-debug")
        .canonicalize()?;
    let input = Path::new(&input).canonicalize()?;
    ensure!(
        input.starts_with(&private),
        "capture must remain under ignored .live-debug"
    );
    // The runner creates an exclusive new directory. Never overwrite capture/backups.
    let output = Path::new(&output).canonicalize()?;
    ensure!(
        output.starts_with(&private) && output != private,
        "output must be under ignored .live-debug"
    );
    ensure!(
        std::fs::read_dir(&output)?.next().is_none(),
        "output must be an empty new directory"
    );
    let snapshots = std::env::var("NICTUI_REMOTE_SNAPSHOT_LINES")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect::<std::result::Result<BTreeSet<usize>, _>>()?;
    replay(&std::fs::read_to_string(input)?, &output, &snapshots)
}

fn record(packet: Value) -> String {
    json!({"elapsed_ms": 0, "kind": "packet", "packet": packet}).to_string()
}

#[test]
fn lossless_text_fields_and_arrival_order() -> Result<()> {
    let first = record(json!({"kind":"DisplayText", "font_size":129,"x":23,"y":41,
        "fg_color":4660,"bg_color":43981,"text":" A@<&\"\u{e9}"}));
    let second =
        record(json!({"kind":"DrawRectangle","x":23,"y":41,"width":16,"height":16,"color":7}));
    let events = parse(&format!(
        "{first}\n{{\"kind\":\"log\",\"message\":\"raw RX\"}}\n{second}\n"
    ))?;
    assert!(
        matches!(&events[0].1, Event::Packet { packet: RemotePacket::DisplayText {
        font_size:129, x:23, y:41, fg_color:4660, bg_color:43981, text }, .. } if text == " A@<&\"\u{e9}")
    );
    assert_eq!(events[2].0, 3);
    assert!(matches!(
        events[2].1,
        Event::Packet {
            packet: RemotePacket::DrawRectangle { color: 7, .. },
            ..
        }
    ));
    Ok(())
}

#[test]
fn all_packet_fields_are_required_without_debug_or_pretty_log_inference() -> Result<()> {
    for value in [
        json!({"kind":"DisplayText","font_size":2,"x":1,"y":2,"fg_color":65535,"bg_color":0,"text":"A"}),
        json!({"kind":"DrawRectangle","x":0,"y":0,"width":128,"height":128,"color":32768}),
        json!({"kind":"DrawSymbol","symbol_id":12,"x":1,"y":2,"fg_color":31,"bg_color":0}),
        json!({"kind":"SignalStrength","strength":4,"mode":1,"battery":0}),
        json!({"kind":"NoiseLevel","level":5,"mode":0}),
        json!({"kind":"SignalBarPos","y":10,"aux":0}),
        json!({"kind":"SmallStatus","id":112,"value1":0,"value2":7}),
        json!({"kind":"UnknownFrame","opcode":250,"payload":[0,255,64]}),
    ] {
        packet(&value)?;
        for key in value
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| key.as_str() != "kind")
        {
            let mut incomplete = value.clone();
            incomplete.as_object_mut().unwrap().remove(key);
            assert!(
                packet(&incomplete).is_err(),
                "silently defaulted {key}: {value}"
            );
        }
    }
    for bad in ["REMOTE TXT ( 23, 41) A", "{", "", "\n"] {
        assert!(parse(bad).is_err());
    }
    assert!(packet(&json!({"kind":"NoiseLevel","level":256,"mode":0})).is_err());
    assert!(packet(&json!({"kind":"NoiseLevel","level":1,"mode":0,"padding":0})).is_err());
    assert!(parse("{\"kind\":\"status\",\"message\":\"finished\"}").is_ok());
    Ok(())
}

#[test]
fn bootstrap_only_diagnostics_are_not_replaced_by_invented_screen_packets() -> Result<()> {
    let events = parse(concat!(
        "{\"kind\":\"phase\",\"message\":\"opening\"}\n",
        "{\"kind\":\"status\",\"message\":\"bootstrap failed\"}\n",
        "{\"kind\":\"log\",\"message\":\"TX/RX diagnostic\"}\n",
        "{\"kind\":\"delta\",\"message\":\"confirmed serializer diagnostic\"}\n",
    ))?;
    assert_eq!(events.len(), 4);
    assert!(
        events
            .iter()
            .all(|(_, event)| matches!(event, Event::Diagnostic { .. }))
    );
    Ok(())
}

#[test]
fn clipping_and_unknowns_are_failures_not_silent_successes() {
    let mut y = 0;
    for value in [
        json!({"kind":"DisplayText","font_size":2,"x":124,"y":0,"fg_color":1,"bg_color":0,"text":"A"}),
        json!({"kind":"DrawRectangle","x":127,"y":0,"width":2,"height":1,"color":0}),
        json!({"kind":"DrawSymbol","symbol_id":17,"x":0,"y":0,"fg_color":1,"bg_color":0}),
        json!({"kind":"UnknownFrame","opcode":250,"payload":[1,2]}),
    ] {
        assert!(geometry(&packet(&value).unwrap(), &mut y).is_err());
    }
    geometry(&RemotePacket::SignalBarPos { y: 127, aux: 0 }, &mut y).unwrap();
    assert!(geometry(&RemotePacket::NoiseLevel { level: 1, mode: 0 }, &mut y).is_err());
}

#[test]
fn timestamps_validate_but_never_reorder_equal_time_packets() -> Result<()> {
    let first = record(json!({"kind":"NoiseLevel","level":9,"mode":0}));
    let second = record(json!({"kind":"NoiseLevel","level":2,"mode":0}));
    let events = parse(&format!("{first}\n{second}"))?;
    assert!(matches!(
        events[0].1,
        Event::Packet {
            packet: RemotePacket::NoiseLevel { level: 9, .. },
            ..
        }
    ));
    assert!(matches!(
        events[1].1,
        Event::Packet {
            packet: RemotePacket::NoiseLevel { level: 2, .. },
            ..
        }
    ));
    let later = first.replace("\"elapsed_ms\":0", "\"elapsed_ms\":5");
    assert!(parse(&format!("{later}\n{second}")).is_err());
    assert!(parse(&format!("{first}\n\n{second}")).is_err());
    Ok(())
}

#[test]
fn persistent_order_clear_and_canvas_guards() -> Result<()> {
    let label = RemotePacket::DisplayText {
        font_size: 2,
        x: 8,
        y: 32,
        fg_color: 0x07ff,
        bg_color: 0,
        text: "OLD MENU".into(),
    };
    let clear = RemotePacket::DrawRectangle {
        x: 0,
        y: 0,
        width: 128,
        height: 128,
        color: 0x8000,
    };
    let mut forward = RadioDisplay::default();
    forward.apply(&label);
    forward.apply(&clear);
    let mut fresh = RadioDisplay::default();
    fresh.apply(&clear);
    let mut reverse = fresh.clone();
    reverse.apply(&label);
    for width in WIDTHS {
        assert_eq!(
            render(&forward, width),
            render(&fresh, width),
            "stale overlay at {width}"
        );
        assert_ne!(
            render(&forward, width),
            render(&reverse, width),
            "packet order lost at {width}"
        );
        guarded(&forward, width)?;
        guarded(&reverse, width)?;
        let serialized = svg(&render(&reverse, width))?;
        assert!(serialized.starts_with("<svg ") && serialized.ends_with("</svg>"));
    }
    Ok(())
}

#[test]
fn destructive_partial_draw_does_not_resurrect_semantic_suffix() {
    let mut display = RadioDisplay::default();
    display.apply(&RemotePacket::DisplayText {
        font_size: 2,
        x: 4,
        y: 32,
        fg_color: 0xffff,
        bg_color: 0,
        text: "Squelch".into(),
    });
    display.apply(&RemotePacket::DrawRectangle {
        x: 4,
        y: 32,
        width: 8,
        height: 8,
        color: 0,
    });
    for width in WIDTHS {
        let buffer = render(&display, width);
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(
            !text.contains("Squelch") && !text.contains("quelch"),
            "stale partial overlay at {width}: {text}"
        );
    }
}
