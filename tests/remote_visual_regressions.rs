use nictui::{
    app::{App, AppMode, MainTab},
    protocol::RemotePacket,
    remote_display::RadioDisplay,
    ui::views::main_layout::{main_content_area, remote_key_at_terminal},
};
use ratatui::{
    Terminal, backend::TestBackend, buffer::Buffer, layout::Rect, style::Color, widgets::Widget,
};

const RESPONSIVE_SIZES: [(u16, u16); 7] = [
    (60, 20),
    (80, 24),
    (100, 30),
    (120, 40),
    (160, 48),
    (190, 80),
    (240, 80),
];

fn buffer_text(buffer: &Buffer, area: Rect) -> String {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn responsive_main_layout_keeps_all_key_labels_and_hit_targets_aligned() {
    let keys = [
        (0x0b, "Menu"),
        (0x02, "1"),
        (0x03, "2"),
        (0x04, "3"),
        (0x0f, "*"),
        (0x0c, "Up"),
        (0x05, "4"),
        (0x06, "5"),
        (0x07, "6"),
        (0x01, "0"),
        (0x0d, "Down"),
        (0x08, "7"),
        (0x09, "8"),
        (0x0a, "9"),
        (0x10, "#"),
        (0x0e, "Exit"),
        (0x12, "F"),
        (0x11, "B"),
        (0x13, "PTT"),
    ];
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Remote);
    app.remote_active = true;
    app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Armed;
    for simulated in [false, true] {
        app.simulator_mode = simulated;
        for enhanced in [false, true] {
            app.remote_keyboard_release_supported = enhanced;
            let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
            for (width, height) in RESPONSIVE_SIZES.into_iter().chain([(80, 24), (60, 20)]) {
                terminal.backend_mut().resize(width, height);
                let area = Rect::new(0, 0, width, height);
                let content = main_content_area(area);
                terminal.resize(area).unwrap();
                terminal
                    .draw(|frame| nictui::ui::ui(frame, &mut app))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let mut targets = std::collections::BTreeMap::<u8, Vec<(u16, u16)>>::new();
                for y in 0..height {
                    for x in 0..width {
                        if let Some(code) = remote_key_at_terminal(area, x, y) {
                            assert!(content.contains((x, y).into()), "hit outside body: {x},{y}");
                            targets.entry(code).or_default().push((x, y));
                        }
                    }
                }
                assert_eq!(targets.len(), keys.len(), "{width}x{height}");
                for (code, label) in keys {
                    let points = targets.get(&code).expect("every logical key is clickable");
                    let left = points.iter().map(|p| p.0).min().unwrap();
                    let top = points.iter().map(|p| p.1).min().unwrap();
                    let right = points.iter().map(|p| p.0).max().unwrap() + 1;
                    let bottom = points.iter().map(|p| p.1).max().unwrap() + 1;
                    let key = Rect::new(left, top, right - left, bottom - top);
                    assert_eq!(
                        points.len(),
                        usize::from(key.width) * usize::from(key.height)
                    );
                    assert!(key.width >= 5 && key.height >= 2, "{label}: {key:?}");
                    if width == 190 {
                        assert_eq!((key.width, key.height), (7, 4), "{label}: {key:?}");
                        assert_eq!(buffer[(key.x, key.y)].symbol(), "┌", "{label}");
                        assert_eq!(
                            buffer[(key.right() - 1, key.bottom() - 1)].symbol(),
                            "┘",
                            "{label}"
                        );
                    }
                    let text = buffer_text(buffer, key);
                    assert!(
                        text.contains(label),
                        "{label} clipped at {width}x{height}: {text}"
                    );
                    if code == 0x13 {
                        assert!(
                            text.contains(if enhanced { "Space" } else { "mouse" }),
                            "{text}"
                        );
                        assert!(!text.contains("a  PTT"));
                    }
                }
                let text = buffer_text(buffer, area);
                assert!(text.contains("PTT: hold mouse"), "{width}x{height}");
                assert!(text.contains(if enhanced {
                    "Space: hold PTT"
                } else {
                    "Space: tap PTT"
                }));
                assert_eq!(text.contains("PTT can transmit"), !simulated);
                assert_eq!(text.contains("Simulated PTT: no RF"), simulated);
            }
        }
    }
    assert!(app.remote_tx.is_none());
}

#[test]
fn responsive_native_lcd_matches_widget_samples_without_mutating_source_pixels() {
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Remote);
    app.simulator_mode = true;
    app.remote_active = true;
    app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Armed;
    let pixels = (0..128 * 128)
        .map(|index| {
            if (index % 128 / 8 + index / 128 / 8) % 2 == 0 {
                0xffff
            } else {
                0x039f
            }
        })
        .collect::<Vec<_>>();
    app.remote_screen.display.set_native_frame(&pixels).unwrap();
    app.remote_screen.has_display_updates = true;
    let original = app.remote_screen.display.clone();
    let palette = [original.pixel(0, 0), original.pixel(8, 0)];
    for ((width, height), columns) in RESPONSIVE_SIZES
        .into_iter()
        .zip([20_u16, 28, 40, 60, 76, 128, 128])
    {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| nictui::ui::ui(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let content = main_content_area(buffer.area);
        let blocks = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|point| {
                let cell = &buffer[*point];
                palette.contains(&cell.fg) && palette.contains(&cell.bg)
            })
            .collect::<Vec<_>>();
        let left = blocks.iter().map(|p| p.0).min().unwrap();
        let top = blocks.iter().map(|p| p.1).min().unwrap();
        assert!(blocks.iter().all(|point| content.contains((*point).into())));
        assert_eq!(
            blocks.len(),
            usize::from(columns) * usize::from(columns.div_ceil(2)),
            "{width}x{height}"
        );
        let expected = render(&original, columns);
        for y in 0..expected.area.height {
            for x in 0..columns {
                assert_eq!(
                    buffer[(left + x, top + y)],
                    expected[(x, y)],
                    "{width}x{height} at {x},{y}"
                );
            }
        }
        let text = buffer_text(buffer, buffer.area);
        assert!(text.contains("Simulator LCD"));
        assert!(text.contains("Native 128x128 LCD / no RF") || text.contains("Native LCD / no RF"));
        for y in 0..128 {
            for x in 0..128 {
                assert_eq!(app.remote_screen.display.pixel(x, y), original.pixel(x, y));
            }
        }
    }
}

fn label(display: &mut RadioDisplay, font_size: u8, x: u8, y: u8, text: &str) {
    display.apply(&RemotePacket::DisplayText {
        font_size,
        x,
        y,
        text: text.into(),
        fg_color: 0xffff,
        bg_color: 0,
    });
}

fn render(display: &RadioDisplay, columns: u16) -> Buffer {
    let area = Rect::new(0, 0, columns, columns.div_ceil(2));
    let mut buffer = Buffer::empty(area);
    Widget::render(display, area, &mut buffer);
    buffer
}

#[test]
fn fractional_advances_do_not_mix_semantic_labels_with_raster_values() {
    // At 63 columns the first label cell projects to three pixels, while
    // the value projects to four. Neither may choose a different font scale.
    let mut display = RadioDisplay::default();
    label(&mut display, 2, 0, 32, "Level");
    label(&mut display, 2, 100, 32, "4");
    let buffer = render(&display, 63);
    let label_row = (0..buffer.area.height)
        .find(|y| {
            (0..63)
                .map(|x| buffer[(x, *y)].symbol())
                .collect::<String>()
                .contains("Level")
        })
        .expect("the label must remain readable");
    assert_eq!(buffer[(49, label_row)].symbol(), "4");
    for y in label_row.saturating_sub(2)..=label_row + 2 {
        if y != label_row {
            let cell = &buffer[(49, y)];
            assert_eq!(cell.fg, Color::Rgb(0, 0, 0), "second value baseline");
            assert_eq!(cell.bg, Color::Rgb(0, 0, 0), "second value baseline");
        }
    }
}

#[test]
fn high_bit_small_font_doubles_source_advance_and_changes_ink() {
    for (x, y, text) in [(0, 0, "VFO-A"), (103, 70, "7.4V"), (122, 124, "AB")] {
        let mut plain = RadioDisplay::default();
        let mut flagged = RadioDisplay::default();
        label(&mut plain, 0, x, y, text);
        label(&mut flagged, 0x80, x, y, text);
        assert!(
            (0..128).any(|y| (0..128).any(|x| plain.pixel(x, y) != flagged.pixel(x, y))),
            "flagged source font must not alias font zero"
        );
        for columns in [24, 28, 33, 40, 63, 64, 76, 96, 128] {
            let buffer = render(&flagged, columns);
            assert_eq!(buffer.area.width, columns);
        }
    }
}

fn dual_display() -> RadioDisplay {
    // Independent synthetic state: no private capture or device dependency.
    let mut display = RadioDisplay::default();
    for (y, mhz, fraction, name) in [
        (22, " 145", "50000", "VFO-A"),
        (83, " 146", "52000", "VFO-B"),
    ] {
        label(&mut display, 0, 0, y, name);
        label(&mut display, 1, 38, y, "LOCAL TEST");
        label(&mut display, 1, 38, y + 9, "W");
        label(&mut display, 1, 103, y + 9, " FM");
        label(&mut display, 129, 23, y + 19, mhz);
        label(&mut display, 2, 87, y + 19, fraction);
        display.apply(&RemotePacket::DrawRectangle {
            x: 85,
            y: y + 32,
            width: 2,
            height: 2,
            color: 0xffff,
        });
        label(&mut display, 0, 0, y + 36, "2.0W");
    }
    display
}

#[test]
fn demo_remote_resize_preserves_controls_and_both_frequency_rows() {
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Remote);
    app.remote_active = true;
    app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Armed;
    app.remote_screen.last_control_report = None;
    app.remote_screen.display = dual_display();
    app.remote_screen.has_display_updates = true;
    for (width, height) in RESPONSIVE_SIZES.into_iter().chain([(80, 24)]) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| nictui::ui::ui(frame, &mut app))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("PTT"), "{width}x{height}");
        assert!(content.contains("mouse"), "{width}x{height}");
        assert!(!content.contains("a  PTT"));
        assert!(
            !content.contains("145.50000"),
            "frequency must retain its LCD scale"
        );
        assert!(
            !content.contains("146.52000"),
            "frequency must retain its LCD scale"
        );
    }
}

#[test]
fn desktop_preserves_every_frequency_digit_against_native_source_ink() {
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Remote);
    app.remote_active = true;
    app.remote_screen.display = dual_display();
    app.remote_screen.has_display_updates = true;
    let white = Color::Rgb(248, 252, 248);
    for (width, height, columns) in [(160, 48, 76), (240, 64, 108)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| nictui::ui::ui(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let blocks: Vec<_> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|point| buffer[*point].symbol() == "▀")
            .collect();
        let left = blocks.iter().map(|(x, _)| *x).min().unwrap();
        let top = blocks.iter().map(|(_, y)| *y).min().unwrap();
        assert_eq!(
            blocks.iter().map(|(x, _)| *x).max().unwrap() - left + 1,
            columns
        );
        for (native_y, value) in [(41, "14550000"), (102, "14652000")] {
            for (index, digit) in value.chars().enumerate() {
                let (native_x, ink_width) = if index < 3 {
                    (39 + index * 16, 9)
                } else {
                    (87 + (index - 3) * 8, 6)
                };
                let scale = usize::from(columns);
                // Compare every sample, not a restyled scale-invariant outline.
                let expected: Vec<Vec<bool>> = (native_y * scale / 128
                    ..((native_y + 16) * scale).div_ceil(128))
                    .map(|y| {
                        (native_x * scale / 128..((native_x + ink_width) * scale).div_ceil(128))
                            .map(|x| {
                                let samples: Vec<_> = (y * 128 / scale
                                    ..((y + 1) * 128).div_ceil(scale))
                                    .flat_map(|sy| {
                                        (x * 128 / scale..((x + 1) * 128).div_ceil(scale))
                                            .map(move |sx| (sx, sy))
                                    })
                                    .collect();
                                if samples.iter().any(|(sx, sy)| {
                                    (85..87).contains(sx)
                                        && (native_y + 13..native_y + 15).contains(sy)
                                }) {
                                    return true;
                                }
                                let samples: Vec<_> = samples
                                    .into_iter()
                                    .map(|(sx, sy)| {
                                        app.remote_screen.display.pixel(sx, sy) == white
                                    })
                                    .collect();
                                let ink = samples.iter().filter(|pixel| **pixel).count();
                                ink * 2 > samples.len() || (ink * 2 == samples.len() && !samples[0])
                            })
                            .collect()
                    })
                    .collect();
                let observed: Vec<Vec<bool>> = (native_y * scale / 128
                    ..((native_y + 16) * scale).div_ceil(128))
                    .map(|y| {
                        (native_x * scale / 128..((native_x + ink_width) * scale).div_ceil(128))
                            .map(|x| {
                                let cell = &buffer[(left + x as u16, top + (y / 2) as u16)];
                                assert_eq!(
                                    cell.symbol(),
                                    "▀",
                                    "frequency must remain raster at desktop sizes"
                                );
                                (if y % 2 == 0 { cell.fg } else { cell.bg }) == white
                            })
                            .collect()
                    })
                    .collect();
                assert_eq!(
                    observed, expected,
                    "{width}x{height}, source row {native_y}, digit {index}: {digit}"
                );
            }
        }
    }
}

#[test]
fn nonuniform_fractional_demo_rows_use_complete_coherent_text_without_ghost_ink() {
    let app = App::demo();
    for columns in [76, 77, 78, 79] {
        let buffer = render(&app.remote_screen.display, columns);
        let content: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        for (native_y, expected) in [(24, "145.33500"), (80, "146.52000")] {
            assert!(content.contains(expected), "{columns}: missing {expected}");
            let text_row = (0..buffer.area.height)
                .find(|y| {
                    (0..columns)
                        .map(|x| buffer[(x, *y)].symbol())
                        .collect::<String>()
                        .contains(expected)
                })
                .unwrap();
            let top = native_y * columns / 256;
            let bottom = ((native_y + 16) * columns).div_ceil(256);
            for y in top..bottom {
                if y == text_row {
                    continue;
                }
                for x in 4 * columns / 128..(76 * columns).div_ceil(128) {
                    let cell = &buffer[(x, y)];
                    assert_eq!(
                        cell.fg,
                        Color::Rgb(0, 0, 0),
                        "ghost raster at {columns}, {x},{y}"
                    );
                    assert_eq!(
                        cell.bg,
                        Color::Rgb(0, 0, 0),
                        "ghost raster at {columns}, {x},{y}"
                    );
                }
            }
        }
    }
}

#[test]
fn native_and_integer_scaled_frequency_rasters_remain_pixel_exact() {
    let mut display = RadioDisplay::default();
    label(&mut display, 2, 4, 24, "145.33500");
    label(&mut display, 2, 4, 80, "146.52000");
    for columns in [128, 256] {
        let buffer = render(&display, columns);
        for y in 0..columns {
            for x in 0..columns {
                let cell = &buffer[(x, y / 2)];
                assert_eq!(cell.symbol(), "▀");
                assert_eq!(
                    if y % 2 == 0 { cell.fg } else { cell.bg },
                    display.pixel(
                        usize::from(x) * 128 / usize::from(columns),
                        usize::from(y) * 128 / usize::from(columns)
                    )
                );
            }
        }
    }
}

#[test]
#[ignore = "optional offline TestBackend images; set NICTUI_REMOTE_VISUAL_OUT"]
fn export_demo_remote_images() {
    use std::fmt::Write;
    let output = std::path::PathBuf::from(
        std::env::var_os("NICTUI_REMOTE_VISUAL_OUT").expect("set NICTUI_REMOTE_VISUAL_OUT"),
    );
    std::fs::create_dir_all(&output).unwrap();
    let rgb = |color| match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#111923".into(),
    };
    let mut app = App::demo();
    app.mode = AppMode::Main(MainTab::Remote);
    app.remote_active = true;
    app.remote_screen.phase = nictui::remote::RemoteSessionPhase::Armed;
    app.remote_screen.last_control_report = None;
    app.remote_screen.display = dual_display();
    app.remote_screen.has_display_updates = true;
    for (width, height) in RESPONSIVE_SIZES {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| nictui::ui::ui(frame, &mut app))
            .unwrap();
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\">",
            width * 10,
            height * 20
        );
        for y in 0..height {
            for x in 0..width {
                let cell = &terminal.backend().buffer()[(x, y)];
                let (px, py) = (x * 10, y * 20);
                write!(
                    svg,
                    "<rect x=\"{px}\" y=\"{py}\" width=\"10\" height=\"20\" fill=\"{}\"/>",
                    rgb(cell.bg)
                )
                .unwrap();
                let fg = rgb(cell.fg);
                if cell.symbol() == "▀" {
                    write!(
                        svg,
                        "<rect x=\"{px}\" y=\"{py}\" width=\"10\" height=\"10\" fill=\"{fg}\"/>"
                    )
                    .unwrap();
                } else if cell.symbol() != " " {
                    let escaped = cell
                        .symbol()
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;");
                    write!(svg, "<text x=\"{px}\" y=\"{}\" fill=\"{fg}\" font-family=\"Menlo,monospace\" font-size=\"16\">{escaped}</text>", py + 16).unwrap();
                }
            }
        }
        svg.push_str("</svg>");
        std::fs::write(output.join(format!("dual-{width}x{height}.svg")), svg).unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer_text(buffer, buffer.area);
        std::fs::write(output.join(format!("dual-{width}x{height}.txt")), text).unwrap();
    }
}
