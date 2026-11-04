use nictui::{protocol::RemotePacket, remote_display::RadioDisplay};
use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

const BLACK: Color = Color::Rgb(0, 0, 0);
const NUMERIC_FONTS: [u8; 3] = [2, 3, 129];
const COLUMNS: [u16; 6] = [64, 65, 76, 95, 96, 128];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InkBounds {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

impl InkBounds {
    fn width(self) -> u16 {
        self.right - self.left + 1
    }

    fn height(self) -> u16 {
        self.bottom - self.top + 1
    }

    fn aspect(self) -> f64 {
        f64::from(self.width()) / f64::from(self.height())
    }
}

fn bounds(points: impl IntoIterator<Item = (u16, u16)>) -> InkBounds {
    let mut points = points.into_iter();
    let (x, y) = points.next().expect("numeric ink must not disappear");
    points.fold(
        InkBounds {
            left: x,
            top: y,
            right: x,
            bottom: y,
        },
        |mut bounds, (x, y)| {
            bounds.left = bounds.left.min(x);
            bounds.top = bounds.top.min(y);
            bounds.right = bounds.right.max(x);
            bounds.bottom = bounds.bottom.max(y);
            bounds
        },
    )
}

fn text(display: &mut RadioDisplay, font: u8, x: u8, y: u8, value: &str, bg: u16) {
    display.apply(&RemotePacket::DisplayText {
        font_size: font,
        x,
        y,
        fg_color: 0xffff,
        bg_color: bg,
        text: value.into(),
    });
}

fn zero(font: u8) -> RadioDisplay {
    let mut display = RadioDisplay::default();
    text(&mut display, font, 16, 16, "0", 0);
    display
}

fn native_bounds(display: &RadioDisplay) -> InkBounds {
    bounds((0..128).flat_map(|y| {
        (0..128).filter_map(move |x| {
            (display.pixel(usize::from(x), usize::from(y)) != BLACK).then_some((x, y))
        })
    }))
}

fn render_area(display: &RadioDisplay, area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, area.right() + 2, area.bottom() + 2));
    Widget::render(display, area, &mut buffer);
    buffer
}

fn render(display: &RadioDisplay, columns: u16) -> Buffer {
    render_area(display, Rect::new(0, 0, columns, columns.div_ceil(2)))
}

fn assert_measurable(buffer: &Buffer) {
    assert!(
        !buffer
            .content
            .iter()
            .any(|cell| cell.symbol().chars().any(|ch| ch.is_ascii_digit())),
        "ASCII fallback is not a measured raster shape; do not silently skip this mode"
    );
}

fn half_pixel(buffer: &Buffer, x: u16, y: u16) -> Color {
    let cell = &buffer[(x, y / 2)];
    match cell.symbol() {
        "▀" => {
            if y.is_multiple_of(2) {
                cell.fg
            } else {
                cell.bg
            }
        }
        "▄" => {
            if y.is_multiple_of(2) {
                cell.bg
            } else {
                cell.fg
            }
        }
        "█" => cell.fg,
        " " => cell.bg,
        symbol => panic!("cannot measure terminal font ink for {symbol:?}"),
    }
}

fn rendered_bounds(buffer: &Buffer) -> InkBounds {
    assert_measurable(buffer);
    bounds((0..buffer.area.height * 2).flat_map(|y| {
        (0..buffer.area.width).filter_map(move |x| {
            let cell = &buffer[(x, y / 2)];
            (cell.symbol() != " " && half_pixel(buffer, x, y) != BLACK).then_some((x, y))
        })
    }))
}

fn assert_tall_zero(bounds: InkBounds, context: &str) {
    // Broad perceptual limits, not a copy of a glyph grid or scaling formula.
    assert!(
        (0.35..=0.90).contains(&bounds.aspect()),
        "{context}: zero is flattened or needle-thin: {bounds:?}, aspect {}",
        bounds.aspect()
    );
}

#[test]
fn native_numeric_zero_ink_is_not_shaped_like_its_advance_cell() {
    for font in NUMERIC_FONTS {
        let display = zero(font);
        let ink = native_bounds(&display);
        assert_tall_zero(ink, &format!("native font {font}"));
    }
}

fn assert_matching_cap_and_baseline(letters_and_digits: &[InkBounds], context: &str) {
    let first = letters_and_digits[0];
    for next in &letters_and_digits[1..] {
        // Rounded caps may differ by a pixel, but not by a separate font scale.
        assert!(
            first.height().abs_diff(next.height()) <= 1,
            "{context}: same-font S/0/4 cap heights disagree: {letters_and_digits:?}"
        );
        assert!(
            first.bottom.abs_diff(next.bottom) <= 1,
            "{context}: same-font S/0/4 baselines disagree: {letters_and_digits:?}"
        );
    }
}

#[test]
fn native_font_two_uppercase_and_digits_share_cap_height_and_baseline() {
    let shapes = ["S", "0", "4"].map(|glyph| {
        let mut display = RadioDisplay::default();
        text(&mut display, 2, 16, 16, glyph, 0);
        native_bounds(&display)
    });
    assert_matching_cap_and_baseline(&shapes, "native font 2");
}

#[test]
fn terminal_font_two_uppercase_and_digits_share_cap_height_and_baseline() {
    for columns in COLUMNS {
        let shapes = ["S", "0", "4"].map(|glyph| {
            let mut display = RadioDisplay::default();
            text(&mut display, 2, 16, 16, glyph, 0);
            rendered_bounds(&render(&display, columns))
        });
        assert_matching_cap_and_baseline(&shapes, &format!("font 2, {columns} columns"));
    }
}

#[test]
fn terminal_numeric_zeros_stay_taller_than_wide() {
    for font in NUMERIC_FONTS {
        let display = zero(font);
        for columns in COLUMNS {
            assert_tall_zero(
                rendered_bounds(&render(&display, columns)),
                &format!("font {font}, {columns} columns"),
            );
        }
    }
}

#[test]
fn numeric_aspect_size_and_baseline_do_not_jump_at_render_mode_boundaries() {
    for font in NUMERIC_FONTS {
        let display = zero(font);
        for (before_columns, after_columns) in [(63, 64), (95, 96), (127, 128)] {
            let before = rendered_bounds(&render(&display, before_columns));
            let after = rendered_bounds(&render(&display, after_columns));
            assert!(
                (before.aspect() - after.aspect()).abs() <= 0.20,
                "font {font}, {before_columns}/{after_columns}: {before:?} -> {after:?}"
            );
            // A one-column resize may quantize a stroke, but must not restyle
            // a one-row character into an unrelated multi-row digit.
            for (a, b) in [
                (before.width(), after.width()),
                (before.height(), after.height()),
                (before.bottom, after.bottom),
            ] {
                let scaled_before = i32::from(a) * i32::from(after_columns);
                let scaled_after = i32::from(b) * i32::from(before_columns);
                assert!(
                    (scaled_after - scaled_before).abs() <= 2 * i32::from(after_columns),
                    "font {font}, {before_columns}/{after_columns}: ink/baseline jumps {before:?} -> {after:?}"
                );
            }
        }
    }
}

#[test]
fn height_limited_viewports_preserve_measured_numeric_shapes() {
    for font in NUMERIC_FONTS {
        let display = zero(font);
        for columns in [64, 76, 96, 128] {
            let expected = rendered_bounds(&render(&display, columns));
            let area = Rect::new(3, 2, columns + 37, columns / 2);
            let actual = rendered_bounds(&render_area(&display, area));
            assert_eq!(actual.width(), expected.width(), "font {font}, {columns}");
            assert_eq!(actual.height(), expected.height(), "font {font}, {columns}");
            assert_eq!(actual.left - expected.left, area.x + 18);
            assert_eq!(actual.top - expected.top, area.y * 2);
        }
    }
}

fn frequency(bg: u16) -> RadioDisplay {
    let mut display = RadioDisplay::default();
    display.apply(&RemotePacket::DrawRectangle {
        x: 0,
        y: 0,
        width: 128,
        height: 128,
        color: bg,
    });
    text(&mut display, 129, 23, 41, " 000", bg);
    text(&mut display, 2, 87, 41, "00000", bg);
    display
}

fn decimal(display: &mut RadioDisplay) {
    display.apply(&RemotePacket::DrawRectangle {
        x: 85,
        y: 54,
        width: 2,
        height: 2,
        color: 0xffff,
    });
}

#[test]
fn received_decimal_changes_only_its_native_patch_not_digit_bodies() {
    for bg in [0, 0xaa82] {
        let before = frequency(bg);
        let mut after = before.clone();
        decimal(&mut after);
        let mut changes = 0;
        for y in 0..128 {
            for x in 0..128 {
                if before.pixel(x, y) != after.pixel(x, y) {
                    changes += 1;
                    assert!(
                        (85..87).contains(&x) && (54..56).contains(&y),
                        "decimal changed native digit at {x},{y}"
                    );
                }
            }
        }
        assert_eq!(changes, 4);
    }
}

#[test]
fn received_decimal_does_not_reshape_mhz_ink_or_move_fraction_digits() {
    for bg in [0, 0xaa82] {
        let before = frequency(bg);
        let mut after = before.clone();
        decimal(&mut after);
        for columns in COLUMNS {
            let before = render(&before, columns);
            let after = render(&after, columns);
            assert_measurable(&before);
            assert_measurable(&after);
            // Allow the received 2x2 patch plus its local sampling gutter.
            // This excludes the MHz digit body and every following digit.
            let gutter_left = 83 * columns / 128;
            let gutter_right = (87 * columns).div_ceil(128);
            let gutter_top = 53 * columns / 128;
            let gutter_bottom = (57 * columns).div_ceil(128);
            let mut changes = 0;
            for y in 0..columns.div_ceil(2) * 2 {
                for x in 0..columns {
                    let changed = half_pixel(&before, x, y) != half_pixel(&after, x, y);
                    if changed {
                        changes += 1;
                        assert!(
                            (gutter_left..gutter_right).contains(&x)
                                && (gutter_top..gutter_bottom).contains(&y),
                            "decimal reshaped digit body/neighbor at {x},{y}, {columns} columns, bg {bg:#x}"
                        );
                    }
                }
            }
            assert!(
                changes > 0,
                "received decimal disappeared at {columns} columns"
            );
        }
    }
}

#[test]
fn wire_advances_match_individually_positioned_digits() {
    for (font, advance) in [(2, 8), (3, 16), (129, 16)] {
        let mut run = RadioDisplay::default();
        text(&mut run, font, 16, 16, "000", 0);
        let mut positioned = RadioDisplay::default();
        for index in 0..3 {
            text(&mut positioned, font, 16 + index * advance, 16, "0", 0);
        }
        for y in 0..128 {
            for x in 0..128 {
                assert_eq!(
                    run.pixel(x, y),
                    positioned.pixel(x, y),
                    "font {font}: wire advance at {x},{y}"
                );
            }
        }
        for columns in COLUMNS {
            assert_eq!(
                render(&run, columns),
                render(&positioned, columns),
                "font {font}, {columns} columns"
            );
        }
    }
}
