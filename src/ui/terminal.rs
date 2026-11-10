use ratatui::{
    buffer::Buffer,
    style::{Color, Modifier},
};

use super::theme::COLOR_SELECTION_BG;

/// Output palette, applied after layout so terminal capabilities never change hit targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalPalette {
    TrueColor,
    Ansi256,
    Ansi16,
    Monochrome,
}

impl TerminalPalette {
    pub fn detect() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let windows_terminal =
            std::env::var_os("WT_SESSION").is_some_and(|value| !value.is_empty());
        Self::for_terminal(
            &term,
            &colorterm,
            no_color,
            windows_terminal,
            crossterm::style::available_color_count(),
        )
    }

    fn for_terminal(
        term: &str,
        colorterm: &str,
        no_color: bool,
        windows_terminal: bool,
        color_count: u16,
    ) -> Self {
        let term = term.to_ascii_lowercase();
        let colorterm = colorterm.to_ascii_lowercase();
        if no_color || term == "dumb" {
            Self::Monochrome
        } else if matches!(colorterm.as_str(), "truecolor" | "24bit")
            || term.ends_with("-direct")
            || windows_terminal
            || color_count > 256
        {
            Self::TrueColor
        } else if term.contains("256") || color_count >= 256 {
            Self::Ansi256
        } else {
            Self::Ansi16
        }
    }

    pub fn apply(self, buffer: &mut Buffer) {
        if self == Self::TrueColor {
            return;
        }
        let width = usize::from(buffer.area.width.max(1));
        let origin = (usize::from(buffer.area.x), usize::from(buffer.area.y));
        for (index, cell) in buffer.content.iter_mut().enumerate() {
            if self == Self::Monochrome {
                let selected = cell.bg == COLOR_SELECTION_BG;
                let halves = match cell.symbol() {
                    "▀" => Some((cell.fg, cell.bg)),
                    "▄" => Some((cell.bg, cell.fg)),
                    "█" => Some((cell.fg, cell.fg)),
                    " " if cell.fg == cell.bg && matches!(cell.bg, Color::Rgb(..)) => {
                        Some((cell.bg, cell.bg))
                    }
                    _ => None,
                };
                let emphasized =
                    selected || (halves.is_none() && light(cell.bg) && !light(cell.fg));
                if let Some((upper, lower)) = halves {
                    let x = origin.0 + index % width;
                    let y = (origin.1 + index / width) * 2;
                    cell.set_symbol(match (lcd_ink(upper, x, y), lcd_ink(lower, x, y + 1)) {
                        (true, true) => "█",
                        (true, false) => "▀",
                        (false, true) => "▄",
                        (false, false) => " ",
                    });
                }
                cell.fg = Color::Reset;
                cell.bg = Color::Reset;
                cell.underline_color = Color::Reset;
                if emphasized {
                    cell.modifier.insert(Modifier::REVERSED);
                }
            } else {
                cell.fg = self.color(cell.fg);
                cell.underline_color = self.color(cell.underline_color);
                cell.bg = if self == Self::Ansi16 && cell.bg == COLOR_SELECTION_BG {
                    Color::Blue
                } else {
                    self.color(cell.bg)
                };
            }
        }
    }

    fn color(self, color: Color) -> Color {
        let rgb = match color {
            Color::Rgb(r, g, b) => [r, g, b],
            Color::Indexed(index) if self == Self::Ansi16 => indexed_rgb(index),
            _ => return color,
        };
        if self == Self::Ansi256 {
            Color::Indexed(nearest_256(rgb))
        } else {
            let index = ANSI_RGB
                .iter()
                .enumerate()
                .min_by_key(|(_, candidate)| distance(rgb, **candidate))
                .unwrap()
                .0;
            ANSI_NAMES[index]
        }
    }
}

const ANSI_NAMES: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];
const ANSI_RGB: [[u8; 3]; 16] = [
    [0, 0, 0],
    [128, 0, 0],
    [0, 128, 0],
    [128, 128, 0],
    [0, 0, 128],
    [128, 0, 128],
    [0, 128, 128],
    [192, 192, 192],
    [128, 128, 128],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [0, 0, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| i32::from(a).abs_diff(i32::from(b)).pow(2))
        .sum()
}

fn indexed_rgb(index: u8) -> [u8; 3] {
    match index {
        0..=15 => ANSI_RGB[index as usize],
        16..=231 => {
            let index = index - 16;
            [
                CUBE[(index / 36) as usize],
                CUBE[(index / 6 % 6) as usize],
                CUBE[(index % 6) as usize],
            ]
        }
        _ => [8 + (index - 232) * 10; 3],
    }
}

fn nearest_256(rgb: [u8; 3]) -> u8 {
    let steps = rgb.map(|value| {
        CUBE.iter()
            .enumerate()
            .min_by_key(|(_, candidate)| value.abs_diff(**candidate))
            .unwrap()
            .0 as u8
    });
    let cube = 16 + steps[0] * 36 + steps[1] * 6 + steps[2];
    let mean = rgb.into_iter().map(u16::from).sum::<u16>() / 3;
    let gray = 232 + ((mean.saturating_sub(8) + 5) / 10).min(23) as u8;
    if distance(rgb, indexed_rgb(gray)) < distance(rgb, indexed_rgb(cube)) {
        gray
    } else {
        cube
    }
}

fn light(color: Color) -> bool {
    luminance(color) >= 128_000
}

fn luminance(color: Color) -> u32 {
    let rgb = match color {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(index) => indexed_rgb(index),
        color => ANSI_NAMES
            .iter()
            .position(|candidate| *candidate == color)
            .map(|index| ANSI_RGB[index])
            .unwrap_or([0; 3]),
    };
    u32::from(rgb[0]) * 299 + u32::from(rgb[1]) * 587 + u32::from(rgb[2]) * 114
}

fn lcd_ink(color: Color, x: usize, y: usize) -> bool {
    // Ordered dithering retains dim colored strokes without flattening the LCD background.
    const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    luminance(color) > (BAYER[y % 4][x % 4] * 16 + 8) * 1000
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn terminal_profiles_select_supported_palettes_without_global_environment_mutation() {
        for (term, color, windows, count, expected) in [
            (
                "xterm-256color",
                "truecolor",
                false,
                65535,
                TerminalPalette::TrueColor,
            ),
            (
                "xterm-256color",
                "24BIT",
                false,
                8,
                TerminalPalette::TrueColor,
            ),
            ("xterm-direct", "", false, 8, TerminalPalette::TrueColor),
            ("xterm-256color", "", false, 256, TerminalPalette::Ansi256),
            (
                "screen-256color",
                "color",
                false,
                8,
                TerminalPalette::Ansi256,
            ),
            ("tmux-256color", "", false, 8, TerminalPalette::Ansi256),
            ("xterm", "", true, 8, TerminalPalette::TrueColor),
            ("", "", false, 65535, TerminalPalette::TrueColor),
            ("linux", "", false, 8, TerminalPalette::Ansi16),
            ("vt100", "", false, 8, TerminalPalette::Ansi16),
            ("", "", false, 8, TerminalPalette::Ansi16),
            (
                "dumb",
                "truecolor",
                true,
                65535,
                TerminalPalette::Monochrome,
            ),
        ] {
            assert_eq!(
                TerminalPalette::for_terminal(term, color, false, windows, count),
                expected
            );
            assert_eq!(
                TerminalPalette::for_terminal(term, color, true, windows, count),
                TerminalPalette::Monochrome
            );
        }
    }

    #[test]
    fn indexed_palette_uses_exact_cube_and_gray_colors() {
        for index in 16..=255 {
            assert_eq!(
                indexed_rgb(nearest_256(indexed_rgb(index))),
                indexed_rgb(index)
            );
        }
        for palette in [TerminalPalette::Ansi16, TerminalPalette::Ansi256] {
            for r in (0..=255).step_by(17) {
                for g in (0..=255).step_by(17) {
                    for b in (0..=255).step_by(17) {
                        assert!(!matches!(
                            palette.color(Color::Rgb(r, g, b)),
                            Color::Rgb(..)
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn monochrome_retains_lcd_ink_selection_and_label_text() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 1));
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(248, 252, 248);
        buffer[(0, 0)].set_symbol("▀").set_fg(white).set_bg(black);
        buffer[(1, 0)].set_symbol("▀").set_fg(black).set_bg(white);
        buffer[(2, 0)].set_symbol(" ").set_fg(white).set_bg(white);
        buffer[(3, 0)].set_symbol(" ").set_fg(black).set_bg(black);
        buffer[(4, 0)]
            .set_symbol("M")
            .set_fg(white)
            .set_bg(COLOR_SELECTION_BG);
        buffer[(4, 0)].underline_color = white;
        TerminalPalette::Monochrome.apply(&mut buffer);
        assert_eq!(
            buffer
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>(),
            "▀▄█ M"
        );
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
        );
        assert!(buffer[(4, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buffer[(4, 0)].underline_color, Color::Reset);
    }

    #[test]
    fn palette_does_not_change_layout_or_truecolor_pixels() {
        let mut buffer = Buffer::empty(Rect::new(7, 3, 4, 2));
        buffer[(8, 4)]
            .set_symbol("▀")
            .set_fg(Color::Rgb(255, 200, 0))
            .set_bg(Color::Rgb(0, 0, 128));
        let original = buffer.clone();
        TerminalPalette::TrueColor.apply(&mut buffer);
        assert_eq!(buffer, original);
        for palette in [TerminalPalette::Ansi16, TerminalPalette::Ansi256] {
            let mut mapped = original.clone();
            palette.apply(&mut mapped);
            assert_eq!(mapped.area, original.area);
            assert_eq!(
                mapped
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>(),
                original
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            );
        }
    }

    #[test]
    fn monochrome_keeps_held_controls_distinct_without_colored_backgrounds() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        buffer[(0, 0)]
            .set_symbol("P")
            .set_fg(Color::Rgb(13, 19, 28))
            .set_bg(Color::Rgb(245, 182, 72));
        buffer[(1, 0)]
            .set_symbol("P")
            .set_fg(Color::Rgb(225, 231, 238))
            .set_bg(Color::Rgb(18, 26, 36));
        TerminalPalette::Monochrome.apply(&mut buffer);
        assert!(buffer[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buffer[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buffer[(0, 0)].symbol(), "P");
    }

    #[test]
    fn monochrome_dithering_does_not_erase_dim_red_or_blue_lcd_ink() {
        for color in [Color::Rgb(248, 0, 0), Color::Rgb(0, 0, 248)] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 4));
            for cell in &mut buffer.content {
                cell.set_symbol(" ").set_fg(color).set_bg(color);
            }
            TerminalPalette::Monochrome.apply(&mut buffer);
            assert!(buffer.content.iter().any(|cell| cell.symbol() != " "));
            assert!(buffer.content.iter().any(|cell| cell.symbol() != "█"));
        }
    }
}
