use nictui::{protocol::RemotePacket, remote_display::RadioDisplay};
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

pub fn rect(display: &mut RadioDisplay, x: u8, y: u8, width: u8, height: u8, color: u16) {
    display.apply(&RemotePacket::DrawRectangle {
        x,
        y,
        width,
        height,
        color,
    });
}

pub fn label(
    display: &mut RadioDisplay,
    font_size: u8,
    x: u8,
    y: u8,
    text: &str,
    fg_color: u16,
    bg_color: u16,
) {
    display.apply(&RemotePacket::DisplayText {
        font_size,
        x,
        y,
        text: text.into(),
        fg_color,
        bg_color,
    });
}

// Synthetic replay using the programmer's native cell geometry, not a device capture.
pub fn menu() -> RadioDisplay {
    let mut display = RadioDisplay::default();
    rect(&mut display, 0, 0, 128, 128, 0x8000);
    rect(&mut display, 0, 20, 128, 10, 0xaa82);
    label(&mut display, 0, 0, 21, "CH003", 0xffff, 0xaa82);
    label(&mut display, 0, 45, 21, "K6BJ2M", 0x07ff, 0xaa82);
    rect(&mut display, 0, 30, 128, 28, 0xffff);
    rect(&mut display, 2, 32, 124, 24, 0);
    label(&mut display, 2, 7, 36, "User Menu", 0x07ff, 0);
    rect(&mut display, 0, 60, 128, 20, 0xffff);
    label(&mut display, 2, 4, 62, "Squelch", 0x8000, 0xffff);
    label(&mut display, 2, 100, 62, "4", 0x8000, 0xffff);
    label(&mut display, 2, 4, 84, "RF Gain", 0xffff, 0x8000);
    label(&mut display, 2, 100, 84, "0", 0xffff, 0x8000);
    label(&mut display, 0, 4, 119, "00/40", 0xffff, 0x8000);
    label(&mut display, 0, 86, 119, "2.52.22", 0xffff, 0x8000);
    display
}

pub fn render(display: &RadioDisplay, columns: u16) -> Buffer {
    let area = Rect::new(0, 0, columns, columns.div_ceil(2));
    let mut buffer = Buffer::empty(area);
    Widget::render(display, area, &mut buffer);
    buffer
}

fn contents(buffer: &Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

fn half_pixel(buffer: &Buffer, x: usize, y: usize) -> ratatui::style::Color {
    let cell = &buffer[(x as u16, (y / 2) as u16)];
    assert_eq!(
        cell.symbol(),
        "▀",
        "unexpected semantic/raster double render"
    );
    if y.is_multiple_of(2) {
        cell.fg
    } else {
        cell.bg
    }
}

fn assert_digit_geometry(
    buffer: &Buffer,
    columns: u16,
    source: (usize, usize, usize),
    ink: ratatui::style::Color,
) -> (usize, usize, usize, usize) {
    let scale = usize::from(columns);
    let (x, y, advance) = source;
    // Exclude the following glyph's first output pixel from this ink bbox.
    let (left, right) = (x * scale / 128, (x + advance) * scale / 128);
    let (top, bottom) = (y * scale / 128, ((y + 16) * scale).div_ceil(128));
    let mut bounds = (right, bottom, left, top);
    for y in top..bottom {
        for x in left..right {
            if half_pixel(buffer, x, y) == ink {
                bounds.0 = bounds.0.min(x);
                bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x + 1);
                bounds.3 = bounds.3.max(y + 1);
            }
        }
    }
    assert!(
        bounds.2 > bounds.0 && bounds.3 > bounds.1,
        "digit lost at {columns}"
    );
    let (width, height) = (bounds.2 - bounds.0, bounds.3 - bounds.1);
    // A common 3:5 form, allowing one output pixel's rounding on each axis.
    // This checks observed ink, independently of the renderer's fit algorithm.
    assert!(
        (width * 5).abs_diff(height * 3) <= 8,
        "stretched {width}x{height} digit at {columns}"
    );
    assert!(width * 5 <= height * 4, "squashed wide digit at {columns}");
    assert!(
        width < right - left,
        "digit consumes its advance/gap at {columns}"
    );
    (bounds.0, bounds.1, width, height)
}

#[test]
fn proportional_menu_letters_and_values_share_cap_height_baseline_and_clear_gaps() {
    use ratatui::style::Color;
    let display = menu();
    for columns in [64, 65, 74, 76, 80] {
        let buffer = render(&display, columns);
        if columns == 76 {
            let text = contents(&buffer);
            for label in ["User Menu", "Squelch", "RF Gain", "4", "0"] {
                assert!(
                    text.contains(label),
                    "missing coherent label/value: {label}"
                );
            }
            continue;
        }
        let scale = usize::from(columns);
        for (source_x, row, fg) in [
            (7, 36, Color::Rgb(248, 252, 0)),
            (4, 62, Color::Rgb(0, 0, 128)),
            (4, 84, Color::Rgb(248, 252, 248)),
        ] {
            let letter = assert_digit_geometry(&buffer, columns, (source_x, row, 8), fg);
            if source_x == 4 {
                let value = assert_digit_geometry(&buffer, columns, (100, row, 8), fg);
                assert_eq!(letter.3, value.3, "letter/value cap height differs");
                assert_eq!(
                    letter.1 + letter.3,
                    value.1 + value.3,
                    "letter/value baseline differs"
                );
            }
        }
        assert_digit_geometry(&buffer, columns, (47, 36, 8), Color::Rgb(248, 252, 0));
        assert_digit_geometry(&buffer, columns, (100, 62, 8), Color::Rgb(0, 0, 128));
        assert_digit_geometry(&buffer, columns, (100, 84, 8), Color::Rgb(248, 252, 248));
        let text = contents(&buffer);
        for label in ["User Menu", "Squelch", "RF Gain"] {
            assert!(
                !text.contains(label),
                "duplicate ASCII over proportional glyphs"
            );
        }
        // Clearing the value removes every one of its output rows, not just a
        // semantic character; the separate label must remain byte-for-byte equal.
        let mut erased = display.clone();
        rect(&mut erased, 100, 62, 8, 16, 0xffff);
        let erased = render(&erased, columns);
        let left = 100 * scale / 128;
        let right = (108 * scale).div_ceil(128);
        for y in 0..buffer.area.height {
            for x in 0..columns {
                if !(left..right).contains(&usize::from(x)) {
                    assert_eq!(buffer[(x, y)], erased[(x, y)]);
                }
            }
        }
    }
}

#[test]
fn split_frequency_ink_keeps_aspect_baseline_decimal_gaps_and_no_underlying_raster() {
    use ratatui::style::Color;
    let ink = Color::Rgb(248, 248, 160);
    let mut display = RadioDisplay::default();
    for (y, bg, mhz, fraction) in [(41, 0, " 162", "55000"), (86, 0xaa82, " 453", "47500")] {
        rect(&mut display, 0, y - 19, 128, 45, bg);
        label(&mut display, 129, 23, y, mhz, 0xa7df, bg);
        label(&mut display, 2, 87, y, fraction, 0xa7df, bg);
        rect(&mut display, 85, y + 13, 2, 2, 0xa7df);
    }
    for columns in [63, 64, 65, 74, 76, 80, 95, 96, 127, 128] {
        let buffer = render(&display, columns);
        assert!(!contents(&buffer).chars().any(|ch| ch.is_ascii_digit()));
        let scale = usize::from(columns);
        for (source_y, bg) in [(41, Color::Rgb(0, 0, 0)), (86, Color::Rgb(16, 80, 168))] {
            // Downsampling may merge a one-native-pixel gap. Do not reshape
            // digits or move the decimal to manufacture terminal-scale gutters.
            for source_x in [39, 55, 71, 87, 95, 103, 111, 119] {
                assert!(
                    (source_x * scale / 128..((source_x + 8) * scale).div_ceil(128)).any(|x| {
                        (source_y * scale / 128..((source_y + 16) * scale).div_ceil(128))
                            .any(|y| half_pixel(&buffer, x, y) == ink)
                    }),
                    "missing digit at {source_x},{source_y}, {columns}"
                );
            }
            assert_eq!(
                half_pixel(&buffer, 85 * scale / 128, (source_y + 13) * scale / 128),
                ink
            );
            // The received blank MHz cell has no residual raster/second baseline.
            for y in source_y * scale / 128..((source_y + 16) * scale).div_ceil(128) {
                for x in 23 * scale / 128..39 * scale / 128 {
                    assert_eq!(
                        half_pixel(&buffer, x, y),
                        bg,
                        "residual frequency ink ({x},{y}) at {columns}"
                    );
                }
            }
        }
    }
}

#[test]
fn decimal_packet_adds_only_a_dot_without_resizing_the_last_mhz_digit() {
    use ratatui::style::Color;
    let mut display = RadioDisplay::default();
    label(&mut display, 129, 23, 41, " 162", 0xa7df, 0);
    label(&mut display, 2, 87, 41, "55000", 0xa7df, 0);
    let mut with_dot = display.clone();
    rect(&mut with_dot, 85, 54, 2, 2, 0xa7df);
    for columns in [63, 64, 65, 74, 76, 80, 95] {
        let before = render(&display, columns);
        let after = render(&with_dot, columns);
        assert!(!contents(&after).chars().any(|ch| ch.is_ascii_digit()));
        let mut changes = 0;
        for y in 0..usize::from(columns) {
            for x in 0..usize::from(columns) {
                if half_pixel(&before, x, y) != half_pixel(&after, x, y) {
                    changes += 1;
                    assert_eq!(
                        half_pixel(&before, x, y),
                        Color::Rgb(0, 0, 0),
                        "decimal erased/reshaped digit ink"
                    );
                    assert!(
                        (83 * usize::from(columns) / 128
                            ..(87 * usize::from(columns)).div_ceil(128))
                            .contains(&x),
                        "decimal moved digit body/fraction"
                    );
                }
            }
        }
        assert!(
            (1..=4).contains(&changes),
            "missing/phantom source decimal at {columns}"
        );
    }
}

#[test]
fn substituted_text_preserves_flat_packet_band_edges_and_native_top_padding() {
    for columns in [64, 65, 74, 76, 80] {
        for (y, height, color, font, text_y, text) in [
            (67, 45, 0xaa82, 0, 67, "VFO-B"),
            (60, 20, 0xffff, 2, 62, "Squelch"),
        ] {
            let mut background = RadioDisplay::default();
            rect(
                &mut background,
                0,
                0,
                128,
                128,
                if color == 0xffff { 0x8000 } else { 0 },
            );
            rect(&mut background, 0, y, 128, height, color);
            let mut labelled = background.clone();
            label(
                &mut labelled,
                font,
                if font == 0 { 0 } else { 4 },
                text_y,
                text,
                if color == 0xffff { 0x8000 } else { 0xffff },
                color,
            );
            let reference = render(&background, columns);
            let rendered = render(&labelled, columns);
            let scale = usize::from(columns);
            let top = usize::from(y) * scale / 128;
            let bottom = usize::from(y + height) * scale / 128;
            let mut edges = vec![top.saturating_sub(1), top, bottom.saturating_sub(1), bottom];
            if font == 2 {
                // Preserve the native two-pixel inset above the large label.
                edges.push((usize::from(y) * scale).div_ceil(128));
            }
            for pixel_y in edges {
                for x in 0..columns {
                    let actual = &rendered[(x, (pixel_y / 2) as u16)];
                    let expected = &reference[(x, (pixel_y / 2) as u16)];
                    assert_eq!(actual.symbol(), "▀", "label escaped into band edge");
                    assert_eq!(
                        if pixel_y % 2 == 0 {
                            actual.fg
                        } else {
                            actual.bg
                        },
                        if pixel_y % 2 == 0 {
                            expected.fg
                        } else {
                            expected.bg
                        },
                        "band {y} edge ({x},{pixel_y}), {columns} columns"
                    );
                }
            }
        }
    }
}

#[test]
fn normal_font_two_menu_labels_remain_readable_on_contrasting_rows() {
    let display = menu();
    for columns in [24, 32, 33, 57, 62] {
        let buffer = render(&display, columns);
        let text = contents(&buffer);
        for label in ["User Menu", "Squelch", "RF Gain", "00/40", "2.52.22"] {
            assert!(
                text.contains(label),
                "missing {label:?} at {columns}: {text}"
            );
        }
        // The narrow status strip need not contain a whole aligned terminal row
        // at tiny sizes. Preserve its border instead of painting text over it.
        if columns >= 57 {
            for label in ["CH003", "K6BJ2M"] {
                assert!(
                    text.contains(label),
                    "missing {label:?} at {columns}: {text}"
                );
            }
        }
        let selected = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "S")
            .unwrap();
        assert_eq!(selected.fg, ratatui::style::Color::Rgb(0, 0, 128));
        assert_eq!(selected.bg, ratatui::style::Color::Rgb(248, 252, 248));
    }
}

#[test]
fn separated_menu_values_use_the_same_compact_height_and_baseline_as_labels() {
    let display = menu();
    for columns in [57, 62] {
        let buffer = render(&display, columns);
        for (label_y, initial, value) in [(62_usize, "S", "4"), (84, "R", "0")] {
            let row = ((label_y + 8) * usize::from(columns) / 256) as u16;
            let label_x = 4 * columns / 128;
            let value_x = 100 * columns / 128;
            assert_eq!(
                buffer[(label_x, row)].symbol(),
                initial,
                "{columns}, y={label_y}"
            );
            assert_eq!(
                buffer[(value_x, row)].symbol(),
                value,
                "{columns}, y={label_y}"
            );
            assert_eq!(buffer[(label_x, row)].fg, buffer[(value_x, row)].fg);
            assert_eq!(buffer[(label_x, row)].bg, buffer[(value_x, row)].bg);
            let mut without_value = display.clone();
            rect(
                &mut without_value,
                100,
                label_y as u8,
                8,
                16,
                if label_y == 62 { 0xffff } else { 0x8000 },
            );
            let background = render(&without_value, columns);
            // One ASCII cell, with no residual multi-row raster value underneath.
            for y in (label_y * usize::from(columns) / 256) as u16
                ..((label_y + 16) * usize::from(columns)).div_ceil(256) as u16
            {
                for x in value_x..(108 * usize::from(columns)).div_ceil(128) as u16 {
                    let cell = &buffer[(x, y)];
                    if (x, y) != (value_x, row) {
                        assert_eq!(
                            cell,
                            &background[(x, y)],
                            "residual value ink at ({x},{y}), {columns}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn background_only_partial_draws_do_not_fragment_menu_text() {
    let fresh = menu();
    let mut updated = fresh.clone();
    // Native last row is cell padding, on both black and white backgrounds.
    rect(&mut updated, 7, 51, 72, 1, 0);
    rect(&mut updated, 4, 77, 56, 1, 0xffff);
    for columns in [24, 33, 64, 74, 80, 128] {
        assert_eq!(
            render(&updated, columns),
            render(&fresh, columns),
            "{columns}"
        );
    }
}

#[test]
fn selection_and_single_character_updates_match_a_fresh_menu() {
    let mut updated = menu();
    for _ in 0..8 {
        label(&mut updated, 2, 100, 62, "9", 0x8000, 0xffff);
        label(&mut updated, 2, 100, 62, "4", 0x8000, 0xffff);
        rect(&mut updated, 0, 60, 128, 20, 0x8000);
        label(&mut updated, 2, 4, 62, "Squelch", 0xffff, 0x8000);
        label(&mut updated, 2, 100, 62, "4", 0xffff, 0x8000);
        rect(&mut updated, 0, 60, 128, 20, 0xffff);
        label(&mut updated, 2, 4, 62, "Squelch", 0x8000, 0xffff);
        label(&mut updated, 2, 100, 62, "4", 0x8000, 0xffff);
    }
    for columns in [24, 33, 64, 74, 80, 128] {
        assert_eq!(
            render(&updated, columns),
            render(&menu(), columns),
            "{columns}"
        );
    }
}

#[test]
fn destructive_partial_draw_keeps_damage_local_and_does_not_restore_letters() {
    let mut display = menu();
    rect(&mut display, 4, 62, 8, 8, 0xffff);
    let text = contents(&render(&display, 33));
    assert!(!text.contains("Squelch"));
    assert!(!text.contains("quelch"));
    assert!(text.contains("User Menu"));
    assert!(text.contains("RF Gain"));
    assert!(
        text.contains('4'),
        "separate value must not be invalidated: {text}"
    );
    label(&mut display, 2, 4, 62, "S", 0x8000, 0xffff);
    assert_eq!(render(&display, 33), render(&menu(), 33));
    rect(&mut display, 4, 62, 8, 16, 0xffff);
    let text = contents(&render(&display, 33));
    assert!(
        !text.contains("quelch"),
        "a fully erased cell must not split its label"
    );
    assert!(text.contains("User Menu"));
    assert!(text.contains('4'));
}

#[test]
fn fragmented_normal_text_packets_have_identical_regions_and_pixels() {
    let whole = menu();
    let mut fragmented = whole.clone();
    rect(&mut fragmented, 7, 36, 72, 16, 0);
    for (index, ch) in "User Menu".chars().enumerate() {
        label(
            &mut fragmented,
            2,
            7 + index as u8 * 8,
            36,
            &ch.to_string(),
            0x07ff,
            0,
        );
    }
    for columns in [24, 33, 57, 64, 74, 80, 106, 128] {
        assert_eq!(
            render(&fragmented, columns),
            render(&whole, columns),
            "{columns}"
        );
    }
}

#[test]
fn normal_letters_keep_subpixel_ink_on_black_and_white() {
    use ratatui::style::Color;
    for (fg, bg) in [(0x07ff, 0), (0x8000, 0xffff)] {
        let ink = Color::Rgb(
            ((fg & 31) << 3) as u8,
            (((fg >> 5) & 63) << 2) as u8,
            ((fg >> 11) << 3) as u8,
        );
        for ch in 'A'..='z' {
            if !ch.is_ascii_alphabetic() {
                continue;
            }
            let mut display = RadioDisplay::default();
            rect(&mut display, 0, 0, 128, 128, bg);
            label(&mut display, 2, 16, 32, &ch.to_string(), fg, bg);
            for columns in [57, 62] {
                assert!(contents(&render(&display, columns)).contains(ch));
            }
            let columns = 106_usize;
            let buffer = render(&display, columns as u16);
            let mut blended = 0;
            for row in 0..columns {
                let y0 = row * 128 / columns;
                let y1 = ((row + 1) * 128).div_ceil(columns);
                if y0 < 32 || y1 > 48 {
                    continue;
                }
                for column in 0..columns {
                    let x0 = column * 128 / columns;
                    let x1 = ((column + 1) * 128).div_ceil(columns);
                    if x0 < 16 || x1 > 24 {
                        continue;
                    }
                    let mut sums = [0_usize; 3];
                    let mut total = 0;
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let dy = ((y + 1) * columns).min((row + 1) * 128)
                                - (y * columns).max(row * 128);
                            let dx = ((x + 1) * columns).min((column + 1) * 128)
                                - (x * columns).max(column * 128);
                            let Color::Rgb(r, g, b) = display.pixel(x, y) else {
                                unreachable!()
                            };
                            for (sum, channel) in sums.iter_mut().zip([r, g, b]) {
                                *sum += usize::from(channel) * dx * dy;
                            }
                            total += dx * dy;
                        }
                    }
                    let expected = Color::Rgb(
                        (sums[0] / total) as u8,
                        (sums[1] / total) as u8,
                        (sums[2] / total) as u8,
                    );
                    let cell = &buffer[(column as u16, (row / 2) as u16)];
                    assert_eq!(cell.symbol(), "▀");
                    assert_eq!(
                        if row % 2 == 0 { cell.fg } else { cell.bg },
                        expected,
                        "{ch}, ({column},{row}), bg {bg:#x}"
                    );
                    blended += usize::from(expected != display.pixel(0, 0) && expected != ink);
                }
            }
            assert!(blended > 0, "no fractional stroke coverage for {ch}");
        }
    }
}

#[test]
fn native_menu_raster_matches_all_pixels_and_clips_each_font_cell() {
    let display = menu();
    let buffer = render(&display, 128);
    // Small labels can use terminal text; normal font2 regions at native scale cannot.
    for y in 36..100 {
        for x in 0..128 {
            let cell = &buffer[(x as u16, (y / 2) as u16)];
            assert_eq!(cell.symbol(), "▀");
            assert_eq!(
                if y % 2 == 0 { cell.fg } else { cell.bg },
                display.pixel(x, y)
            );
        }
    }
    for font in [0, 1, 2, 3, 129] {
        let mut clipped = RadioDisplay::default();
        label(&mut clipped, font, 124, 124, "MixedCase", 0xffff, 0);
        assert_eq!(clipped.pixel(123, 124), ratatui::style::Color::Rgb(0, 0, 0));
        assert_eq!(clipped.pixel(128, 128), ratatui::style::Color::Rgb(0, 0, 0));
        render(&clipped, 33);
    }
}

#[test]
fn symbol_ink_in_letter_padding_disables_compact_substitution_until_redrawn() {
    let mut display = RadioDisplay::default();
    label(&mut display, 2, 16, 32, "A", 0xffff, 0);
    label(&mut display, 2, 64, 32, "OK", 0xffff, 0);
    // Only the letter's blank rightmost column intersects the icon background.
    // Its later foreground strokes still change the completed letter cell.
    display.apply(&RemotePacket::DrawSymbol {
        symbol_id: 0,
        x: 23,
        y: 32,
        fg_color: 0x07e0,
        bg_color: 0,
    });
    for columns in [24, 33, 57, 62] {
        let text = contents(&render(&display, columns));
        assert!(!text.contains('A'), "stale letter at {columns}: {text}");
        assert!(text.contains("OK"), "unrelated label at {columns}: {text}");
    }
    let native = render(&display, 128);
    for y in 32..40 {
        for x in 16..31 {
            let cell = &native[(x, y / 2)];
            assert_eq!(cell.symbol(), "▀");
            assert_eq!(
                if y % 2 == 0 { cell.fg } else { cell.bg },
                display.pixel(x.into(), y.into()),
                "overlap at ({x},{y})"
            );
        }
    }
    rect(&mut display, 16, 32, 15, 16, 0);
    label(&mut display, 2, 16, 32, "A", 0xffff, 0);
    let mut fresh = RadioDisplay::default();
    label(&mut fresh, 2, 16, 32, "A", 0xffff, 0);
    label(&mut fresh, 2, 64, 32, "OK", 0xffff, 0);
    for columns in [24, 33, 74, 96, 128] {
        assert_eq!(render(&display, columns), render(&fresh, columns));
    }
}

#[test]
fn later_letter_ink_in_padding_respects_packet_order() {
    let mut display = RadioDisplay::default();
    label(&mut display, 2, 16, 32, "A", 0xffff, 0);
    label(&mut display, 2, 64, 32, "OK", 0xffff, 0);
    label(&mut display, 0, 16, 47, "M", 0x07ff, 0);
    for columns in [24, 33, 57, 62] {
        let text = contents(&render(&display, columns));
        assert!(!text.contains('A'), "stale letter at {columns}: {text}");
        assert!(text.contains("OK"), "unrelated label at {columns}: {text}");
    }
    // Reversing the draw order restores the large letter, not the small one.
    label(&mut display, 2, 16, 32, "A", 0xffff, 0);
    for columns in [24, 33, 57, 62] {
        let text = contents(&render(&display, columns));
        assert!(text.contains('A'), "missing redraw at {columns}: {text}");
        assert!(
            !text.contains('M'),
            "stale small letter at {columns}: {text}"
        );
    }
}

#[test]
fn wide_viewports_center_a_square_native_canvas_without_stretching() {
    let display = menu();
    let reference = render(&display, 96);
    for width in [128, 160] {
        let area = Rect::new(3, 2, width, 48);
        let guard = ratatui::buffer::Cell::new("!").clone();
        let mut buffer = Buffer::filled(Rect::new(0, 0, width + 6, 52), guard.clone());
        Widget::render(&display, area, &mut buffer);
        let left = area.x + (width - 96) / 2;
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                if !area.contains((x, y).into()) {
                    assert_eq!(buffer[(x, y)], guard, "escaped ({x},{y})");
                } else if (left..left + 96).contains(&x) {
                    assert_eq!(buffer[(x, y)], reference[(x - left, y - area.y)]);
                } else {
                    assert_eq!(buffer[(x, y)].symbol(), " ");
                    assert_eq!(buffer[(x, y)].bg, ratatui::style::Color::Rgb(0, 0, 0));
                }
            }
        }
    }
}
