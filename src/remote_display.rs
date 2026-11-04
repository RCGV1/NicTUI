//! Persistent, ordered radio display rendering. Wire coordinates are native 128x128 pixels.
//! Glyphs are independent approximations, not the programmer's font assets.
//! The programmer's 16x16 protocol icons are reduced to native 8x8 cells.

use crate::protocol::RemotePacket;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use std::collections::BTreeSet;

pub const DISPLAY_WIDTH: usize = 128;
pub const DISPLAY_HEIGHT: usize = 128;
const BLACK: Color = Color::Rgb(0, 0, 0);

#[derive(Clone, Debug)]
pub struct RadioDisplay {
    pixels: Vec<Color>,
    text_pixels: Vec<bool>,
    smooth_pixels: Vec<bool>,
    decimal_pixels: Vec<bool>,
    signal_bar_y: usize,
    text: Vec<TextGlyph>,
}

#[derive(Clone, Debug)]
struct TextGlyph {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    ch: char,
    fg: Color,
    bg: Color,
    intact: bool,
    decimal: bool,
    remaining: [u16; 16],
    region_left: usize,
    region_right: usize,
}

struct TextRun<'a> {
    glyphs: Vec<&'a TextGlyph>,
    region: usize,
    x: usize,
    y: usize,
    text: String,
    raster: Option<TextRaster>,
}

struct TextRaster {
    width: usize,
    height: usize,
    ink: Vec<bool>,
}

impl TextRaster {
    fn new(glyphs: &[&TextGlyph], columns: usize) -> Option<Self> {
        let first = glyphs[0];
        if first.height != 16 || !(63..96).contains(&columns) {
            return None;
        }
        let left = first.x * columns / DISPLAY_WIDTH;
        let last = glyphs.last()?;
        let width = (last.x + last.width) * columns / DISPLAY_WIDTH - left;
        let pixel_top = first.y * columns / DISPLAY_HEIGHT;
        let baseline = (first.y + 14) * columns / DISPLAY_HEIGHT - pixel_top;
        let phase = pixel_top % 2;
        let height = (baseline + 1 + phase).div_ceil(2) * 2;
        let mut raster = Self {
            width,
            height,
            ink: vec![false; width * height],
        };
        for glyph in glyphs {
            let x = glyph.x * columns / DISPLAY_WIDTH - left;
            let cell_width = (glyph.x + glyph.width) * columns / DISPLAY_WIDTH - left - x;
            {
                // Clip/advance and ink are separate, as in the programmer's
                // point-sized font drawing. All tall glyphs share one scale.
                let native_scale = if glyph.width == 16 { 3 } else { 2 };
                let budget = cell_width.checked_sub(if glyph.width == 16 { 3 } else { 1 })?;
                let mut ink_width =
                    (3 * native_scale * columns + DISPLAY_WIDTH / 2) / DISPLAY_WIDTH;
                let mut ink_height =
                    (5 * native_scale * columns + DISPLAY_HEIGHT / 2) / DISPLAY_HEIGHT;
                if ink_width > budget {
                    ink_width = budget;
                    ink_height = (5 * budget + 1) / 3;
                }
                if ink_width < 3 || ink_height > baseline + 1 {
                    return None;
                }
                let ink_top = phase + baseline + 1 - ink_height;
                let bits = tall_rows(glyph.ch);
                for y in 0..ink_height {
                    for dx in 0..ink_width {
                        raster.ink[(ink_top + y) * width + x + dx] =
                            bits[y * 5 / ink_height] & (1 << (2 - dx * 3 / ink_width)) != 0;
                    }
                }
                if glyph.decimal {
                    raster.ink[(phase + baseline) * width + x + cell_width - 2] = true;
                }
            }
        }
        Some(raster)
    }
}

impl Default for RadioDisplay {
    fn default() -> Self {
        Self {
            pixels: vec![BLACK; DISPLAY_WIDTH * DISPLAY_HEIGHT],
            text_pixels: vec![false; DISPLAY_WIDTH * DISPLAY_HEIGHT],
            smooth_pixels: vec![false; DISPLAY_WIDTH * DISPLAY_HEIGHT],
            decimal_pixels: vec![false; DISPLAY_WIDTH * DISPLAY_HEIGHT],
            signal_bar_y: 0,
            text: Vec::new(),
        }
    }
}

impl RadioDisplay {
    pub fn clear(&mut self) {
        self.pixels.fill(BLACK);
        self.text_pixels.fill(false);
        self.smooth_pixels.fill(false);
        self.decimal_pixels.fill(false);
        self.signal_bar_y = 0;
        self.text.clear();
    }

    /// Returns the native pixel color, or black outside the display.
    pub fn pixel(&self, x: usize, y: usize) -> Color {
        if x < DISPLAY_WIDTH && y < DISPLAY_HEIGHT {
            self.pixels[y * DISPLAY_WIDTH + x]
        } else {
            BLACK
        }
    }

    /// Apply every packet in arrival order, before any bounded history is updated.
    pub fn apply(&mut self, packet: &RemotePacket) {
        match packet {
            RemotePacket::DrawRectangle {
                x,
                y,
                width,
                height,
                color,
            } => {
                // The live frequency protocol adds its decimal in the padding of
                // the last double-width MHz cell, before the narrow fraction.
                let decimal = self
                    .text
                    .iter()
                    .find(|glyph| {
                        glyph.intact
                            && glyph.width == 16
                            && glyph.height == 16
                            && glyph.ch.is_ascii_digit()
                            && glyph.x + 14 == usize::from(*x)
                            && glyph.y + 13 == usize::from(*y)
                            && *width == 2
                            && *height == 2
                            && glyph.fg == wire_color(*color)
                            && (0..5).all(|index| {
                                self.text.iter().any(|fraction| {
                                    fraction.intact
                                        && fraction.x == glyph.x + glyph.width + index * 8
                                        && fraction.y == glyph.y
                                        && fraction.width == 8
                                        && fraction.height == 16
                                        && fraction.ch.is_ascii_digit()
                                        && fraction.fg == glyph.fg
                                        && fraction.bg == glyph.bg
                                })
                            })
                    })
                    .map(|glyph| (glyph.x, glyph.y));
                self.fill(
                    *x as usize,
                    *y as usize,
                    *width as usize,
                    *height as usize,
                    wire_color(*color),
                );
                if let Some((glyph_x, glyph_y)) = decimal {
                    if let Some(glyph) = self
                        .text
                        .iter_mut()
                        .find(|glyph| glyph.x == glyph_x && glyph.y == glyph_y && glyph.width == 16)
                    {
                        glyph.intact = true;
                        glyph.decimal = true;
                        glyph.remaining[13] |= 0xc000;
                        glyph.remaining[14] |= 0xc000;
                    }
                    for row in usize::from(*y)..usize::from(*y) + 2 {
                        let start = row * DISPLAY_WIDTH + usize::from(*x);
                        self.text_pixels[start..start + 2].fill(true);
                        self.decimal_pixels[start..start + 2].fill(true);
                    }
                }
            }
            RemotePacket::DisplayText {
                font_size,
                x,
                y,
                fg_color,
                bg_color,
                text,
            } => {
                let slot = if *font_size == 129 { 3 } else { *font_size & 3 };
                let (width, height) = [(6, 8), (8, 8), (8, 16), (16, 16)][slot as usize];
                let mut x = *x as usize;
                for ch in text.chars().take_while(|ch| *ch != '\0') {
                    if x >= DISPLAY_WIDTH {
                        break;
                    }
                    self.glyph(
                        x,
                        *y as usize,
                        width,
                        height,
                        if ch == '@' { ' ' } else { ch },
                        wire_color(*fg_color),
                        wire_color(*bg_color),
                    );
                    x += width;
                }
            }
            RemotePacket::DrawSymbol {
                symbol_id,
                x,
                y,
                fg_color,
                bg_color,
            } => {
                if let Some(rows) = SYMBOL_ROWS.get(*symbol_id as usize) {
                    self.fill(*x as usize, *y as usize, 8, 8, wire_color(*bg_color));
                    for dy in 0..8 {
                        for dx in 0..8 {
                            // Bit zero is leftmost. Pool each 2x2 block so thin icon
                            // strokes survive reduction from the programmer's 2x raster.
                            if (rows[dy * 2] | rows[dy * 2 + 1]) & (3 << (dx * 2)) != 0 {
                                self.paint_pixel(
                                    *x as usize + dx,
                                    *y as usize + dy,
                                    wire_color(*fg_color),
                                );
                            }
                        }
                    }
                }
            }
            RemotePacket::SignalBarPos { y, .. } => self.signal_bar_y = *y as usize,
            RemotePacket::SignalStrength { strength, mode, .. } => {
                let level = ((*strength as usize * 27) >> 4).min(120);
                let active = if *mode == 0 {
                    Color::Rgb(50, 205, 50)
                } else {
                    Color::Rgb(255, 165, 0)
                };
                for i in (0..120).step_by(2) {
                    self.fill(
                        i + 4,
                        self.signal_bar_y,
                        1,
                        6,
                        if i < level {
                            active
                        } else {
                            Color::Rgb(40, 40, 40)
                        },
                    );
                }
            }
            RemotePacket::NoiseLevel { level, mode } => {
                let level = (*level as usize).min(120);
                let color = if *mode == 0 {
                    Color::Rgb(90, 90, 255)
                } else {
                    Color::Rgb(50, 205, 50)
                };
                // Preserve the programmer's overlapping fill/clear order.
                self.fill(4, self.signal_bar_y + 6, level + 4, 1, color);
                self.fill(level + 4, self.signal_bar_y + 6, 120 - level, 1, BLACK);
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(self, area);
    }

    fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        let end_x = x.saturating_add(width).min(DISPLAY_WIDTH);
        let end_y = y.saturating_add(height).min(DISPLAY_HEIGHT);
        if x >= end_x || y >= end_y {
            return;
        }
        self.text.retain_mut(|glyph| {
            if x < glyph.x + glyph.width
                && end_x > glyph.x
                && y < glyph.y + glyph.height
                && end_y > glyph.y
            {
                let full_cell = x <= glyph.x
                    && end_x >= glyph.x + glyph.width
                    && y <= glyph.y
                    && end_y >= glyph.y + glyph.height;
                // Repainting unchanged padding does not damage the character.
                // Full-cell writes still retire metadata, including blank cells.
                if glyph.intact
                    && !full_cell
                    && (y.max(glyph.y)..end_y.min(glyph.y + glyph.height)).all(|row| {
                        self.pixels[row * DISPLAY_WIDTH + x.max(glyph.x)
                            ..row * DISPLAY_WIDTH + end_x.min(glyph.x + glyph.width)]
                            .iter()
                            .all(|pixel| *pixel == color)
                    })
                {
                    return true;
                }
                // Keep an invalid marker until that cell is replaced. Otherwise
                // surviving pieces could become independent semantic overlays.
                glyph.intact = false;
                glyph.decimal = false;
                let left = x.max(glyph.x) - glyph.x;
                let right = end_x.min(glyph.x + glyph.width) - glyph.x;
                let covered = (((1_u32 << (right - left)) - 1) << left) as u16;
                for row in y.max(glyph.y)..end_y.min(glyph.y + glyph.height) {
                    glyph.remaining[row - glyph.y] &= !covered;
                }
            }
            glyph.remaining.iter().any(|row| *row != 0)
        });
        for row in y..end_y {
            for column in x..end_x {
                let index = row * DISPLAY_WIDTH + column;
                if self.pixels[index] != color {
                    self.text_pixels[index] = false;
                    self.smooth_pixels[index] = false;
                }
                self.pixels[index] = color;
            }
            self.decimal_pixels[row * DISPLAY_WIDTH + x..row * DISPLAY_WIDTH + end_x].fill(false);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn glyph(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        ch: char,
        fg: Color,
        bg: Color,
    ) {
        self.fill(x, y, width, height, bg);
        let narrow_grid = height == 16 && (ch.is_ascii_digit() || matches!(ch, '.' | '-'));
        if height == 16 {
            let rows = tall_rows(ch);
            let scale = if width == 16 { 3 } else { 2 };
            let ink_width = 3 * scale;
            let ink_top = height - 1 - 5 * scale;
            for dy in 0..5 * scale {
                for dx in 0..ink_width {
                    if rows[dy / scale] & (1 << (2 - dx / scale)) != 0 {
                        self.paint_pixel(x + dx, y + ink_top + dy, fg);
                    }
                }
            }
        } else {
            let rows = glyph_rows(ch);
            for dy in 0..height - 1 {
                for dx in 0..width - 1 {
                    if rows[dy * 7 / (height - 1)] & (1 << (4 - dx * 5 / (width - 1))) != 0 {
                        self.paint_pixel(x + dx, y + dy, fg);
                    }
                }
            }
        }
        // Mark the completed glyph only after its ink has invalidated older
        // overlapping cells, including strokes drawn into unchanged padding.
        for row in y..y.saturating_add(height).min(DISPLAY_HEIGHT) {
            if x < DISPLAY_WIDTH {
                self.text_pixels
                    [row * DISPLAY_WIDTH + x..row * DISPLAY_WIDTH + (x + width).min(DISPLAY_WIDTH)]
                    .fill(true);
                self.smooth_pixels
                    [row * DISPLAY_WIDTH + x..row * DISPLAY_WIDTH + (x + width).min(DISPLAY_WIDTH)]
                    .fill(!narrow_grid);
            }
        }
        if x + width <= DISPLAY_WIDTH && y + height <= DISPLAY_HEIGHT {
            let mut remaining = [0; 16];
            remaining[..height].fill(u16::MAX >> (16 - width));
            self.text.push(TextGlyph {
                x,
                y,
                width,
                height,
                ch,
                fg,
                bg,
                intact: true,
                decimal: false,
                remaining,
                region_left: x,
                region_right: x + width,
            });
            // Preserve the extent of a connected label after individual cells
            // are erased. Otherwise its surviving prefix/suffix could be
            // incorrectly promoted back to complete terminal text.
            let mut left = x;
            let mut right = x + width;
            loop {
                let previous = (left, right);
                for glyph in &self.text {
                    if glyph.y == y
                        && glyph.height == height
                        && glyph.region_left <= right
                        && glyph.region_right >= left
                    {
                        left = left.min(glyph.region_left);
                        right = right.max(glyph.region_right);
                    }
                }
                if previous == (left, right) {
                    break;
                }
            }
            for glyph in &mut self.text {
                if glyph.y == y
                    && glyph.height == height
                    && glyph.region_left <= right
                    && glyph.region_right >= left
                {
                    glyph.region_left = left;
                    glyph.region_right = right;
                }
            }
        }
    }

    fn paint_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x < DISPLAY_WIDTH && y < DISPLAY_HEIGHT {
            // A no-op background fill may have preserved an older glyph. Its
            // metadata must not survive new symbol/glyph ink in that padding.
            if self.pixels[y * DISPLAY_WIDTH + x] != color {
                self.fill(x, y, 1, 1, color);
            }
        }
    }
}

/// Wire colors are little-endian BGR565, with red in the LOW five bits.
fn wire_color(value: u16) -> Color {
    Color::Rgb(
        ((value & 31) << 3) as u8,
        (((value >> 5) & 63) << 2) as u8,
        ((value >> 11) << 3) as u8,
    )
}

impl Widget for &RadioDisplay {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buffer[(x, y)].reset();
                buffer[(x, y)].set_bg(BLACK);
            }
        }
        let columns = usize::from(area.width).min(usize::from(area.height) * 2);
        if columns == 0 {
            return;
        }
        let rows = columns.div_ceil(2);
        let left = area.x + (area.width - columns as u16) / 2;
        let top = area.y + (area.height - rows as u16) / 2;
        let mut glyphs: Vec<_> = self.text.iter().collect();
        glyphs.sort_by_key(|glyph| (glyph.y, glyph.x));
        // Connected cells form a region even across packet/color/font-width
        // boundaries. A damaged label cannot turn into a semantic suffix, but an
        // unrelated value on the same baseline remains independent.
        let mut regions: Vec<(usize, bool, bool)> = Vec::new();
        let mut right = 0;
        for (index, glyph) in glyphs.iter().enumerate() {
            if index == 0
                || glyph.y != glyphs[index - 1].y
                || glyph.height != glyphs[index - 1].height
                || glyph.region_left != glyphs[index - 1].region_left
                || glyph.region_right != glyphs[index - 1].region_right
            {
                if let Some(previous) = regions.last_mut() {
                    previous.1 &= right == glyphs[index - 1].region_right;
                }
                regions.push((index, true, false));
                right = glyph.region_left;
            }
            let region = regions.last_mut().unwrap();
            region.1 &= glyph.intact && glyph.ch.is_ascii() && glyph.x == right;
            right = glyph.x + glyph.width;
            // The two independent source grids need at least their native ink
            // resolution. Four raster columns cannot represent 5x7 menu letters.
            let narrow_grid =
                glyph.height == 16 && (glyph.ch.is_ascii_digit() || matches!(glyph.ch, '.' | '-'));
            region.2 |= glyph.height < 16
                || if narrow_grid {
                    glyph.width * columns / DISPLAY_WIDTH < 4
                        || glyph.height * columns / DISPLAY_HEIGHT < 5
                } else {
                    (glyph.width - 1) * columns / DISPLAY_WIDTH < 5
                        || (glyph.height - 1) * columns / DISPLAY_HEIGHT < 7
                };
        }
        if let Some(region) = regions.last_mut() {
            region.1 &= right == glyphs.last().unwrap().region_right;
        }
        // A font row has one visual scale even when labels and values are
        // separate regions. Damaged regions still retain their raster pixels.
        let compact_rows: BTreeSet<_> = regions
            .iter()
            .filter(|(_, intact, needs_text)| *intact && *needs_text)
            .map(|(start, _, _)| (glyphs[*start].y, glyphs[*start].height))
            .collect();
        for (start, _, needs_text) in &mut regions {
            *needs_text |= compact_rows.contains(&(glyphs[*start].y, glyphs[*start].height));
        }
        let mut compact = Vec::new();
        let mut index = 0;
        let mut region = 0;
        while index < glyphs.len() {
            while region + 1 < regions.len() && index >= regions[region + 1].0 {
                region += 1;
            }
            let first = glyphs[index];
            let mut end = index + 1;
            while end < glyphs.len() {
                let previous = glyphs[end - 1];
                let next = glyphs[end];
                if next.y != first.y
                    || next.height != first.height
                    || next.fg != first.fg
                    || next.bg != first.bg
                    || next.x != previous.x + previous.width
                {
                    break;
                }
                end += 1;
            }
            let run = &glyphs[index..end];
            let raster = TextRaster::new(run, columns);
            if regions[region].1 && (regions[region].2 || raster.is_some()) {
                let mut text = String::new();
                for glyph in run {
                    text.push(glyph.ch);
                    if glyph.decimal {
                        text.push('.');
                    }
                }
                let x = first.x * columns / DISPLAY_WIDTH;
                let last = run.last().unwrap();
                let right = ((last.x + last.width) * columns).div_ceil(DISPLAY_WIDTH);
                if raster.is_some() || (text.len() <= right - x && x + text.len() <= columns) {
                    let y = if raster.is_some() {
                        first.y * columns / (DISPLAY_HEIGHT * 2)
                    } else {
                        (first.y
                            + if first.height >= 16 {
                                first.height / 2
                            } else {
                                0
                            })
                            * columns
                            / (DISPLAY_HEIGHT * 2)
                    };
                    compact.push(TextRun {
                        glyphs: run.to_vec(),
                        region,
                        x,
                        y,
                        text,
                        raster,
                    });
                } else {
                    regions[region].1 = false;
                }
            }
            index = end;
        }
        compact.retain(|run| regions[run.region].1);
        // A terminal character covers an entire cell, sometimes extending outside
        // its native glyph. Only substitute on a uniform background. Reconsider
        // neighboring runs when one must stay raster, so overlays never hide ink.
        let mut graphics;
        loop {
            graphics = self.pixels.clone();
            for run in &compact {
                for glyph in &run.glyphs {
                    for y in glyph.y..glyph.y + glyph.height {
                        graphics[y * DISPLAY_WIDTH + glyph.x
                            ..y * DISPLAY_WIDTH + glyph.x + glyph.width]
                            .fill(glyph.bg);
                    }
                }
            }
            let count = compact.len();
            let mut rejected = vec![false; regions.len()];
            let mut occupied: Vec<(usize, usize, usize)> = Vec::new();
            compact.retain_mut(|run| {
                let first = run.glyphs[0];
                let width = run
                    .raster
                    .as_ref()
                    .map_or(run.text.len(), |raster| raster.width);
                let height = run.raster.as_ref().map_or(1, |raster| raster.height / 2);
                for row in [run.y, run.y + 1] {
                    if row + height > rows || row * 256 / columns >= first.y + first.height {
                        continue;
                    }
                    let end = run.x + width;
                    if occupied.iter().any(|(x0, x1, y)| {
                        (row..row + height).contains(y) && run.x < *x1 && end > *x0
                    }) {
                        continue;
                    }
                    let x0 = run.x * DISPLAY_WIDTH / columns;
                    let x1 = (end * DISPLAY_WIDTH).div_ceil(columns).min(DISPLAY_WIDTH);
                    let y0 = row * 2 * DISPLAY_HEIGHT / columns;
                    let y1 = ((row + height) * 2 * DISPLAY_HEIGHT)
                        .div_ceil(columns)
                        .min(DISPLAY_HEIGHT);
                    let uniform = if let Some(raster) = &run.raster {
                        // Blank output pixels retain the sampled graphics, so
                        // only actual ink needs a uniform background underneath.
                        (0..raster.height).all(|dy| {
                            (0..raster.width).all(|dx| {
                                if !raster.ink[dy * raster.width + dx] {
                                    return true;
                                }
                                let px0 = (run.x + dx) * DISPLAY_WIDTH / columns;
                                let px1 = ((run.x + dx + 1) * DISPLAY_WIDTH)
                                    .div_ceil(columns)
                                    .min(DISPLAY_WIDTH);
                                let py0 = (row * 2 + dy) * DISPLAY_HEIGHT / columns;
                                let py1 = ((row * 2 + dy + 1) * DISPLAY_HEIGHT)
                                    .div_ceil(columns)
                                    .min(DISPLAY_HEIGHT);
                                (py0..py1).all(|y| {
                                    graphics[y * DISPLAY_WIDTH + px0..y * DISPLAY_WIDTH + px1]
                                        .iter()
                                        .all(|color| *color == first.bg)
                                })
                            })
                        })
                    } else {
                        (y0..y1).all(|y| {
                            graphics[y * DISPLAY_WIDTH + x0..y * DISPLAY_WIDTH + x1]
                                .iter()
                                .all(|color| *color == first.bg)
                        })
                    };
                    if uniform {
                        run.y = row;
                        occupied.extend((row..row + height).map(|y| (run.x, end, y)));
                        return true;
                    }
                }
                rejected[run.region] = true;
                false
            });
            compact.retain(|run| !rejected[run.region]);
            if compact.len() == count {
                break;
            }
        }
        // Removed text must not keep influencing the source sampler: otherwise
        // identical rectangle edges vote differently under a substituted label.
        let mut substituted = vec![false; DISPLAY_WIDTH * DISPLAY_HEIGHT];
        for run in &compact {
            for glyph in &run.glyphs {
                for y in glyph.y..glyph.y + glyph.height {
                    substituted
                        [y * DISPLAY_WIDTH + glyph.x..y * DISPLAY_WIDTH + glyph.x + glyph.width]
                        .fill(true);
                }
            }
        }
        // Sample covered source pixels rather than a single point, preserving thin
        // dividers and meter segments when the display is reduced to terminal cells.
        let mut colors: Vec<(Color, usize)> = Vec::with_capacity(16);
        let mut sample = |column: usize, pixel_row: usize| {
            if pixel_row >= columns {
                return BLACK;
            }
            let x0 = column * DISPLAY_WIDTH / columns;
            let y0 = pixel_row * DISPLAY_HEIGHT / columns;
            let x1 = ((column + 1) * DISPLAY_WIDTH)
                .div_ceil(columns)
                .min(DISPLAY_WIDTH);
            let y1 = ((pixel_row + 1) * DISPLAY_HEIGHT)
                .div_ceil(columns)
                .min(DISPLAY_HEIGHT);
            colors.clear();
            // A verified 2x2 frequency decimal must survive reduction even when
            // most of its sample is background. The mask is overwritten by every
            // later draw, just like the pixels; it never replays an old rectangle.
            for y in y0..y1 {
                for x in x0..x1 {
                    if self.decimal_pixels[y * DISPLAY_WIDTH + x]
                        && !substituted[y * DISPLAY_WIDTH + x]
                    {
                        return graphics[y * DISPLAY_WIDTH + x];
                    }
                }
            }
            let text_sample = (y0..y1).any(|y| {
                (x0..x1).any(|x| {
                    self.text_pixels[y * DISPLAY_WIDTH + x] && !substituted[y * DISPLAY_WIDTH + x]
                })
            });
            let smooth_sample = (y0..y1).all(|y| {
                (x0..x1).all(|x| {
                    self.smooth_pixels[y * DISPLAY_WIDTH + x] && !substituted[y * DISPLAY_WIDTH + x]
                })
            });
            if smooth_sample {
                // Integrate the actual covered area, not the most common color.
                // A thin letter stroke can cover less than half a sample and
                // must still contribute instead of disappearing into its bg.
                let mut rgb = [0_usize; 3];
                let mut total = 0;
                for y in y0..y1 {
                    let dy = ((y + 1) * columns).min((pixel_row + 1) * DISPLAY_HEIGHT)
                        - (y * columns).max(pixel_row * DISPLAY_HEIGHT);
                    for x in x0..x1 {
                        let dx = ((x + 1) * columns).min((column + 1) * DISPLAY_WIDTH)
                            - (x * columns).max(column * DISPLAY_WIDTH);
                        let weight = dx * dy;
                        if let Color::Rgb(r, g, b) = graphics[y * DISPLAY_WIDTH + x] {
                            for (sum, value) in rgb.iter_mut().zip([r, g, b]) {
                                *sum += usize::from(value) * weight;
                            }
                            total += weight;
                        }
                    }
                }
                if let [Some(red), Some(green), Some(blue)] =
                    rgb.map(|value| value.checked_div(total))
                {
                    return Color::Rgb(red as u8, green as u8, blue as u8);
                }
            }
            for y in y0..y1 {
                for x in x0..x1 {
                    let color = graphics[y * DISPLAY_WIDTH + x];
                    // The coarse 3x5 grid needs hard gaps between adjacent cells;
                    // mixed graphic/text samples must not blur unrelated graphics.
                    if text_sample || color != BLACK {
                        if let Some((_, count)) =
                            colors.iter_mut().find(|(value, _)| *value == color)
                        {
                            *count += 1;
                        } else {
                            colors.push((color, 1));
                        }
                    }
                }
            }
            colors
                .iter()
                .max_by_key(|(_, count)| *count)
                .map(|(color, _)| *color)
                .unwrap_or(BLACK)
        };
        for row in 0..rows {
            for col in 0..columns {
                let fg = sample(col, row * 2);
                let bg = sample(col, row * 2 + 1);
                buffer[(left + col as u16, top + row as u16)]
                    .set_symbol("▀")
                    .set_fg(fg)
                    .set_bg(bg);
            }
        }
        for run in compact {
            let first = run.glyphs[0];
            if let Some(raster) = run.raster {
                for row in 0..raster.height / 2 {
                    for x in 0..raster.width {
                        let cell =
                            &mut buffer[(left + (run.x + x) as u16, top + (run.y + row) as u16)];
                        let fg = if raster.ink[row * 2 * raster.width + x] {
                            first.fg
                        } else {
                            cell.fg
                        };
                        let bg = if raster.ink[(row * 2 + 1) * raster.width + x] {
                            first.fg
                        } else {
                            cell.bg
                        };
                        cell.set_symbol("▀").set_fg(fg).set_bg(bg);
                    }
                }
                continue;
            }
            buffer.set_stringn(
                left + run.x as u16,
                top + run.y as u16,
                &run.text,
                run.text.len(),
                Style::default()
                    .fg(first.fg)
                    .bg(first.bg)
                    .add_modifier(if first.height >= 16 {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
        }
    }
}

fn digit_rows(ch: char) -> [u8; 5] {
    match ch {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        '.' => [0, 0, 0, 0, 2],
        '-' => [0, 0, 7, 0, 0],
        _ => [0; 5],
    }
}

// One compact cap-height/baseline for all tall text, not a cell-fit font.
// These independent forms deliberately trade detail for undistorted strokes.
fn tall_rows(ch: char) -> [u8; 5] {
    if ch.is_ascii_digit() || matches!(ch, '.' | '-') {
        return digit_rows(ch);
    }
    match ch {
        'M' => [5, 7, 7, 5, 5],
        'W' => [5, 5, 7, 7, 5],
        _ => {
            let rows = narrow_rows(ch);
            [rows[0], rows[2], rows[3], rows[4], rows[6]]
        }
    }
}

// Independent 3x7 terminal-scale forms. Keep one whole output pixel of
// inter-character padding instead of pooling adjacent source glyph strokes.
fn narrow_rows(ch: char) -> [u8; 7] {
    match ch {
        ' ' => [0; 7],
        '0' => [7, 5, 5, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 2, 2, 7],
        '2' => [7, 1, 1, 7, 4, 4, 7],
        '3' => [7, 1, 1, 7, 1, 1, 7],
        '4' => [5, 5, 5, 7, 1, 1, 1],
        '5' => [7, 4, 4, 7, 1, 1, 7],
        '6' => [7, 4, 4, 7, 5, 5, 7],
        '7' => [7, 1, 1, 2, 2, 2, 2],
        '8' => [7, 5, 5, 7, 5, 5, 7],
        '9' => [7, 5, 5, 7, 1, 1, 7],
        'A' => [2, 5, 5, 7, 5, 5, 5],
        'B' => [6, 5, 5, 6, 5, 5, 6],
        'C' => [3, 4, 4, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 5, 5, 6],
        'E' => [7, 4, 4, 6, 4, 4, 7],
        'F' => [7, 4, 4, 6, 4, 4, 4],
        'G' => [3, 4, 4, 5, 5, 5, 3],
        'H' => [5, 5, 5, 7, 5, 5, 5],
        'I' => [7, 2, 2, 2, 2, 2, 7],
        'J' => [1, 1, 1, 1, 5, 5, 2],
        'K' => [5, 5, 6, 4, 6, 5, 5],
        'L' => [4, 4, 4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5, 5, 5],
        'N' => [5, 7, 7, 7, 5, 5, 5],
        'O' => [2, 5, 5, 5, 5, 5, 2],
        'P' => [6, 5, 5, 6, 4, 4, 4],
        'Q' => [2, 5, 5, 5, 5, 3, 1],
        'R' => [6, 5, 5, 6, 5, 5, 5],
        'S' => [3, 4, 4, 2, 1, 1, 6],
        'T' => [7, 2, 2, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 5, 5, 2],
        'W' => [5, 5, 5, 5, 7, 7, 5],
        'X' => [5, 5, 5, 2, 5, 5, 5],
        'Y' => [5, 5, 5, 2, 2, 2, 2],
        'Z' => [7, 1, 1, 2, 4, 4, 7],
        'a' => [0, 0, 6, 1, 7, 5, 7],
        'b' => [4, 4, 6, 5, 5, 5, 6],
        'c' => [0, 0, 3, 4, 4, 4, 3],
        'd' => [1, 1, 3, 5, 5, 5, 3],
        'e' => [0, 0, 2, 5, 7, 4, 3],
        'f' => [1, 2, 7, 2, 2, 2, 2],
        'g' => [0, 0, 3, 5, 3, 1, 6],
        'h' => [4, 4, 6, 5, 5, 5, 5],
        'i' => [2, 0, 6, 2, 2, 2, 7],
        'j' => [1, 0, 1, 1, 1, 5, 2],
        'k' => [4, 4, 5, 6, 4, 6, 5],
        'l' => [6, 2, 2, 2, 2, 2, 7],
        'm' => [0, 0, 7, 7, 5, 5, 5],
        'n' => [0, 0, 6, 5, 5, 5, 5],
        'o' => [0, 0, 2, 5, 5, 5, 2],
        'p' => [0, 0, 6, 5, 6, 4, 4],
        'q' => [0, 0, 3, 5, 3, 1, 1],
        'r' => [0, 0, 6, 5, 4, 4, 4],
        's' => [0, 0, 3, 4, 2, 1, 6],
        't' => [2, 2, 7, 2, 2, 2, 1],
        'u' => [0, 0, 5, 5, 5, 5, 3],
        'v' => [0, 0, 5, 5, 5, 5, 2],
        'w' => [0, 0, 5, 5, 5, 7, 7],
        'x' => [0, 0, 5, 5, 2, 5, 5],
        'y' => [0, 0, 5, 5, 3, 1, 6],
        'z' => [0, 0, 7, 1, 2, 4, 7],
        '.' => [0, 0, 0, 0, 0, 0, 2],
        ',' => [0, 0, 0, 0, 0, 2, 4],
        ':' => [0, 2, 0, 0, 2, 0, 0],
        '-' => [0, 0, 0, 7, 0, 0, 0],
        '+' => [0, 0, 2, 7, 2, 0, 0],
        '/' => [1, 1, 2, 2, 2, 4, 4],
        '_' => [0, 0, 0, 0, 0, 0, 7],
        _ => [7, 5, 5, 5, 5, 5, 7],
    }
}

// Independently drawn 5x7 glyphs; unsupported characters have an outlined replacement.
fn glyph_rows(ch: char) -> [u8; 7] {
    match ch {
        ' ' => [0; 7],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 25, 21, 19, 19, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 27, 17],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ',' => [0, 0, 0, 0, 4, 4, 8],
        ':' => [0, 4, 4, 0, 4, 4, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '%' => [17, 18, 2, 4, 8, 9, 17],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        '<' => [1, 2, 4, 8, 4, 2, 1],
        '^' => [4, 10, 17, 0, 0, 0, 0],
        '!' => [4, 4, 4, 4, 4, 0, 4],
        '*' => [0, 21, 14, 31, 14, 21, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '=' => [0, 0, 31, 0, 31, 0, 0],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        'a' => [0, 0, 14, 1, 15, 17, 15],
        'b' => [16, 16, 30, 17, 17, 17, 30],
        'c' => [0, 0, 14, 17, 16, 17, 14],
        'd' => [1, 1, 15, 17, 17, 17, 15],
        'e' => [0, 0, 14, 17, 31, 16, 14],
        'f' => [6, 8, 8, 30, 8, 8, 8],
        'g' => [0, 14, 17, 17, 15, 1, 14],
        'h' => [16, 16, 30, 17, 17, 17, 17],
        'i' => [4, 0, 12, 4, 4, 4, 14],
        'j' => [2, 0, 6, 2, 2, 18, 12],
        'k' => [16, 16, 18, 20, 24, 20, 18],
        'l' => [12, 4, 4, 4, 4, 4, 14],
        'm' => [0, 0, 26, 21, 21, 21, 21],
        'n' => [0, 0, 30, 17, 17, 17, 17],
        'o' => [0, 0, 14, 17, 17, 17, 14],
        'p' => [0, 0, 30, 17, 30, 16, 16],
        'q' => [0, 0, 15, 17, 15, 1, 1],
        'r' => [0, 0, 22, 25, 16, 16, 16],
        's' => [0, 0, 15, 16, 14, 1, 30],
        't' => [8, 8, 30, 8, 8, 9, 6],
        'u' => [0, 0, 17, 17, 17, 19, 13],
        'v' => [0, 0, 17, 17, 17, 10, 4],
        'w' => [0, 0, 17, 17, 21, 21, 10],
        'x' => [0, 0, 17, 10, 4, 10, 17],
        'y' => [0, 0, 17, 17, 15, 1, 14],
        'z' => [0, 0, 31, 2, 4, 8, 31],
        ';' => [0, 4, 4, 0, 4, 4, 8],
        '?' => [14, 17, 1, 2, 4, 0, 4],
        '#' => [10, 10, 31, 10, 31, 10, 10],
        '&' => [12, 18, 20, 8, 21, 18, 13],
        '$' => [4, 15, 20, 14, 5, 30, 4],
        '"' => [10, 10, 10, 0, 0, 0, 0],
        '\'' => [4, 4, 8, 0, 0, 0, 0],
        '[' => [14, 8, 8, 8, 8, 8, 14],
        ']' => [14, 2, 2, 2, 2, 2, 14],
        '{' => [2, 4, 4, 8, 4, 4, 2],
        '}' => [8, 4, 4, 2, 4, 4, 8],
        '\\' => [16, 8, 8, 4, 2, 2, 1],
        '|' => [4, 4, 4, 4, 4, 4, 4],
        '~' => [0, 0, 8, 21, 2, 0, 0],
        '`' => [8, 4, 2, 0, 0, 0, 0],
        _ => [31, 17, 17, 17, 17, 17, 31],
    }
}

// Small protocol icon masks recovered from verified static initializers; no font
// tables or application implementation are included. IDs match the inspected
// programmer (Bluetooth=12, blank=13, pause=14), not the shifted public-doc table.
const SYMBOL_ROWS: [[u16; 16]; 17] = [
    [
        0, 124, 254, 198, 198, 16382, 32766, 26310, 26310, 16070, 15872, 26112, 26112, 32512,
        16128, 0,
    ],
    [
        0, 64, 432, 684, 682, 682, 682, 682, 12970, 19082, 9730, 4098, 2050, 1538, 260, 0,
    ],
    [
        0, 32736, 16416, 32736, 16416, 16416, 16440, 16444, 16446, 28734, 30748, 31744, 31744,
        14336, 0, 0,
    ],
    [
        0, 896, 1920, 3840, 7680, 15360, 30720, 61440, 30720, 15360, 7680, 3840, 1920, 896, 0, 0,
    ],
    [
        0, 384, 960, 2016, 4080, 8184, 16380, 32766, 960, 960, 960, 960, 960, 960, 960, 0,
    ],
    [
        0, 960, 960, 960, 960, 960, 960, 960, 32766, 16380, 8184, 4080, 2016, 960, 384, 0,
    ],
    [
        0, 2016, 16380, 32766, 32766, 32766, 32766, 16380, 2016, 960, 480, 112, 24, 4, 0, 0,
    ],
    [
        0, 0, 0, 0, 56, 124, 32750, 32710, 27886, 27772, 56, 0, 0, 0, 0, 0,
    ],
    [
        0, 3136, 7360, 14816, 29680, 25080, 24796, 24654, 29190, 15110, 8070, 4046, 1948, 824, 560,
        0,
    ],
    [
        0, 384, 960, 2016, 3696, 7224, 14364, 28686, 57351, 57351, 65535, 65535, 384, 8184, 16380,
        0,
    ],
    [
        0, 31942, 50886, 50886, 50894, 50894, 50902, 50902, 50902, 50918, 50918, 50886, 50886,
        50886, 31942, 0,
    ],
    [
        0, 31868, 50886, 50886, 50886, 50886, 50886, 65278, 50886, 50886, 50886, 50886, 50886,
        50886, 50886, 0,
    ],
    [
        0, 3840, 6912, 13104, 25456, 13280, 7104, 3968, 1792, 7104, 13280, 25456, 13104, 6912,
        3840, 0,
    ],
    [0; 16],
    [
        0, 0, 31868, 31868, 31868, 31868, 31868, 31868, 31868, 31868, 31868, 31868, 31868, 31868,
        0, 0,
    ],
    [
        0, 7951, 16159, 29494, 25446, 25446, 29542, 16230, 8038, 870, 870, 870, 822, 799, 783, 0,
    ],
    [
        0, 8056, 16252, 29454, 25350, 25356, 29464, 16176, 8032, 864, 864, 864, 880, 830, 798, 0,
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(display: &RadioDisplay, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        Widget::render(display, area, &mut buffer);
        buffer
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer.content.iter().map(|cell| cell.symbol()).collect()
    }

    fn rect(x: u8, y: u8, width: u8, height: u8, color: u16) -> RemotePacket {
        RemotePacket::DrawRectangle {
            x,
            y,
            width,
            height,
            color,
        }
    }

    fn text(font_size: u8, x: u8, y: u8, text: &str) -> RemotePacket {
        RemotePacket::DisplayText {
            font_size,
            x,
            y,
            fg_color: 0x001f,
            bg_color: 0xf800,
            text: text.into(),
        }
    }

    fn symbol(id: u8, x: u8, y: u8) -> RemotePacket {
        RemotePacket::DrawSymbol {
            symbol_id: id,
            x,
            y,
            fg_color: 0x001f,
            bg_color: 0xf800,
        }
    }

    fn frequency_text(font_size: u8, x: u8, y: u8, value: &str, bg: u16) -> RemotePacket {
        RemotePacket::DisplayText {
            font_size,
            x,
            y,
            fg_color: 0xa7df,
            bg_color: bg,
            text: value.into(),
        }
    }

    fn split_frequency(y: u8, bg: u16, fragmented: bool) -> RadioDisplay {
        let mut display = RadioDisplay::default();
        display.apply(&rect(0, 0, 128, 128, bg));
        if fragmented {
            for (index, ch) in " 162".chars().enumerate() {
                display.apply(&frequency_text(
                    129,
                    23 + index as u8 * 16,
                    y,
                    &ch.to_string(),
                    bg,
                ));
            }
            display.apply(&frequency_text(2, 87, y, "55", bg));
            display.apply(&frequency_text(2, 103, y, "000", bg));
        } else {
            display.apply(&frequency_text(129, 23, y, " 162", bg));
            display.apply(&frequency_text(2, 87, y, "55000", bg));
        }
        display.apply(&rect(85, y + 13, 2, 2, 0xa7df));
        display
    }

    fn half_pixel(buffer: &Buffer, x: usize, y: usize) -> Color {
        let cell = &buffer[(x as u16, (y / 2) as u16)];
        assert_eq!(cell.symbol(), "▀");
        if y.is_multiple_of(2) {
            cell.fg
        } else {
            cell.bg
        }
    }

    #[test]
    fn live_split_frequencies_and_decimal_are_readable_at_narrow_sizes() {
        let mut display = split_frequency(41, 0, false);
        display.apply(&rect(0, 67, 128, 45, 0xaa82));
        display.apply(&frequency_text(129, 23, 86, " 453", 0xaa82));
        display.apply(&frequency_text(2, 87, 86, "47500", 0xaa82));
        display.apply(&rect(85, 99, 2, 2, 0xa7df));
        for columns in [24_u16, 32, 33, 34, 57, 58, 59, 62] {
            let buffer = rendered(&display, columns, columns.div_ceil(2));
            let content = buffer_text(&buffer);
            assert!(content.contains("162.55000"), "{columns}: {content}");
            assert!(content.contains("453.47500"), "{columns}: {content}");
        }
    }

    #[test]
    fn fragmented_text_packets_match_whole_runs_in_pixels_and_terminal_geometry() {
        for bg in [0, 0xaa82] {
            let whole = split_frequency(41, bg, false);
            let fragmented = split_frequency(41, bg, true);
            assert_eq!(whole.pixels, fragmented.pixels);
            assert_eq!(whole.text_pixels, fragmented.text_pixels);
            for columns in [24_u16, 33, 63, 64, 65, 74, 128] {
                assert_eq!(
                    rendered(&whole, columns, columns.div_ceil(2)),
                    rendered(&fragmented, columns, columns.div_ceil(2)),
                    "{columns} columns, bg {bg:#x}",
                );
            }
        }
    }

    #[test]
    fn native_digit_shapes_and_separators_do_not_depend_on_background_color() {
        let black = split_frequency(41, 0, false);
        let blue = split_frequency(41, 0xaa82, false);
        let ink = wire_color(0xa7df);
        for columns in [64_u16, 65, 74, 80, 128] {
            let black_buffer = rendered(&black, columns, columns.div_ceil(2));
            let blue_buffer = rendered(&blue, columns, columns.div_ceil(2));
            for (black, blue) in black_buffer.content.iter().zip(&blue_buffer.content) {
                assert_eq!(black.fg == ink, blue.fg == ink, "{columns} columns");
                assert_eq!(black.bg == ink, blue.bg == ink, "{columns} columns");
            }
            let scale = usize::from(columns);
            let top = (41 * scale).div_ceil(128);
            let bottom = 56 * scale / 128;
            for index in 0..5 {
                let x = 87 + index * 8;
                let left = x * scale / 128;
                let right = ((x + 8) * scale).div_ceil(128).min(scale);
                assert!(
                    (left..right).any(|col| (top..bottom)
                        .any(|row| { half_pixel(&black_buffer, col, row) == ink })),
                    "fraction digit {index} lost at {columns} columns",
                );
                assert!(
                    (left..right).any(|col| (top..bottom)
                        .all(|row| { half_pixel(&black_buffer, col, row) == BLACK })),
                    "fraction digit {index} has no separator at {columns} columns",
                );
            }
            let mut native_only = black.clone();
            native_only.text.clear();
            let native = rendered(&native_only, columns, columns.div_ceil(2));
            if columns < 96 {
                assert!(
                    black_buffer != native,
                    "medium size must use snapped glyphs"
                );
            } else {
                assert_eq!(black_buffer, native);
            }
        }
    }

    #[test]
    fn partial_erasure_disables_the_entire_baseline_and_never_resurrects_text() {
        for bg in [0, 0xaa82] {
            let mut display = split_frequency(41, bg, false);
            for _ in 0..8 {
                display.apply(&rect(74, 44, 9, 8, bg));
                let mut native_only = display.clone();
                native_only.text.clear();
                for columns in [24_u16, 33, 63, 64, 74] {
                    assert_eq!(
                        rendered(&display, columns, columns.div_ceil(2)),
                        rendered(&native_only, columns, columns.div_ceil(2)),
                        "partial erasure at {columns} columns, bg {bg:#x}",
                    );
                }
                display.apply(&frequency_text(129, 71, 41, "2", bg));
                display.apply(&rect(85, 54, 2, 2, 0xa7df));
                let fresh = split_frequency(41, bg, false);
                assert_eq!(display.pixels, fresh.pixels);
                assert_eq!(rendered(&display, 33, 17), rendered(&fresh, 33, 17));
            }
            display.apply(&rect(71, 41, 16, 16, bg));
            // A dot alone must not revalidate a cleared MHz digit.
            display.apply(&rect(85, 54, 2, 2, 0xa7df));
            let mut native_only = display.clone();
            native_only.text.clear();
            assert_eq!(rendered(&display, 33, 17), rendered(&native_only, 33, 17));
            display.apply(&rect(0, 0, 128, 128, bg));
            assert!(display.text.is_empty());
            assert!(display.pixels.iter().all(|pixel| *pixel == wire_color(bg)));
        }
    }

    #[test]
    fn shorter_fraction_updates_preserve_the_untouched_suffix() {
        let mut partial = split_frequency(41, 0, false);
        partial.apply(&frequency_text(2, 87, 41, "6", 0));
        let mut whole = split_frequency(41, 0, false);
        whole.apply(&frequency_text(2, 87, 41, "65000", 0));
        assert_eq!(partial.pixels, whole.pixels);
        for columns in [24_u16, 33, 64, 74] {
            assert_eq!(
                rendered(&partial, columns, columns.div_ceil(2)),
                rendered(&whole, columns, columns.div_ceil(2)),
            );
        }
    }

    #[test]
    fn verified_decimal_survives_reduction_and_later_draws_replace_its_coverage() {
        let ink = wire_color(0xa7df);
        for bg in [0, 0xaa82] {
            let display = split_frequency(41, bg, false);
            let mut erased = display.clone();
            erased.apply(&rect(85, 54, 2, 2, bg));
            for columns in [64_u16, 65, 74, 80, 128] {
                let with_dot = rendered(&display, columns, columns.div_ceil(2));
                let without_dot = rendered(&erased, columns, columns.div_ceil(2));
                let scale = usize::from(columns);
                let (x, y) = if columns < 96 {
                    (87 * scale / 128 - 2, (41 + 14) * scale / 128)
                } else {
                    (85 * scale / 128, 54 * scale / 128)
                };
                assert_eq!(half_pixel(&with_dot, x, y), ink, "{columns}, bg {bg:#x}");
                assert_ne!(
                    half_pixel(&without_dot, x, y),
                    ink,
                    "stale decimal at {columns}"
                );
            }
            let mut partial = display.clone();
            partial.apply(&rect(85, 54, 1, 2, bg));
            let buffer = rendered(&partial, 128, 64);
            assert_eq!(half_pixel(&buffer, 85, 54), wire_color(bg));
            assert_eq!(half_pixel(&buffer, 86, 54), ink);
            partial.apply(&rect(86, 54, 1, 2, bg));
            assert!(!partial.decimal_pixels.iter().any(|pixel| *pixel));
            assert_eq!(rendered(&partial, 74, 37), rendered(&erased, 74, 37));
        }
    }

    #[test]
    fn full_rect_clear_prunes_markers_before_different_font_and_position_updates() {
        let mut display = split_frequency(41, 0, false);
        display.apply(&rect(74, 44, 9, 8, 0));
        assert!(display.text.iter().any(|glyph| !glyph.intact));
        display.apply(&rect(23, 41, 104, 16, 0));
        assert!(display.text.is_empty());
        assert!(!display.decimal_pixels.iter().any(|pixel| *pixel));
        display.apply(&frequency_text(2, 11, 44, "146.52000", 0));
        let mut fresh = RadioDisplay::default();
        fresh.apply(&frequency_text(2, 11, 44, "146.52000", 0));
        assert_eq!(display.pixels, fresh.pixels);
        for columns in [24_u16, 33, 64, 74] {
            assert_eq!(
                rendered(&display, columns, columns.div_ceil(2)),
                rendered(&fresh, columns, columns.div_ceil(2)),
            );
        }
        assert!(buffer_text(&rendered(&display, 24, 12)).contains("146.52000"));
    }

    #[test]
    fn adjacent_compact_baselines_cannot_overwrite_the_same_terminal_cell() {
        let mut display = RadioDisplay::default();
        display.apply(&frequency_text(0, 0, 0, "A", 0));
        display.apply(&frequency_text(0, 0, 8, "B", 0));
        let buffer = rendered(&display, 24, 12);
        assert_eq!(buffer[(0, 0)].symbol(), "A");
        assert_eq!(buffer[(0, 1)].symbol(), "B");
        assert_eq!(buffer_text(&buffer).matches('A').count(), 1);
        assert_eq!(buffer_text(&buffer).matches('B').count(), 1);
    }

    #[test]
    fn multiple_narrower_replacements_retire_a_double_width_invalid_marker() {
        let mut display = RadioDisplay::default();
        display.apply(&frequency_text(129, 0, 0, "1", 0));
        display.apply(&frequency_text(2, 0, 0, "1", 0));
        assert!(display.text.iter().any(|glyph| !glyph.intact));
        display.apply(&frequency_text(2, 8, 0, "2", 0));
        assert_eq!(display.text.len(), 2);
        assert!(display.text.iter().all(|glyph| glyph.intact));
        let mut fresh = RadioDisplay::default();
        fresh.apply(&frequency_text(2, 0, 0, "12", 0));
        assert_eq!(display.pixels, fresh.pixels);
        for columns in [24_u16, 32, 33, 63, 64, 74] {
            assert_eq!(
                rendered(&display, columns, columns.div_ceil(2)),
                rendered(&fresh, columns, columns.div_ceil(2)),
            );
        }
        assert!(buffer_text(&rendered(&display, 24, 12)).contains("12"));

        display.apply(&rect(0, 0, 8, 8, 0));
        display.apply(&rect(0, 8, 8, 8, 0));
        assert_eq!(display.text.len(), 1);
        display.apply(&rect(8, 0, 8, 16, 0));
        assert!(display.text.is_empty());
    }

    #[test]
    fn odd_width_text_uses_the_raster_scale_and_does_not_cover_unrelated_graphics() {
        let mut display = RadioDisplay::default();
        display.apply(&frequency_text(0, 0, 61, "A", 0));
        let buffer = rendered(&display, 33, 17);
        assert_eq!(buffer[(0, 7)].symbol(), "A");
        assert_ne!(buffer[(0, 8)].symbol(), "A");

        display.clear();
        display.apply(&rect(0, 24, 6, 7, 0x07e0));
        display.apply(&frequency_text(0, 0, 31, "A", 0));
        let buffer = rendered(&display, 32, 16);
        // Compare against the untouched graphic, not a raster letter whose
        // text sampling mask itself changes the adjacent graphic's edge vote.
        let mut graphic_only = RadioDisplay::default();
        graphic_only.apply(&rect(0, 24, 6, 7, 0x07e0));
        let native = rendered(&graphic_only, 32, 16);
        for x in 0..32 {
            assert_eq!(buffer[(x, 3)], native[(x, 3)], "green graphic at {x},3");
        }
    }

    #[test]
    fn terminal_text_remains_readable_and_later_draws_erase_it() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut display = RadioDisplay::default();
        display.apply(&rect(0, 0, 128, 128, 0xf800));
        display.apply(&text(0, 0, 0, "NORTH RIDGE"));
        display.apply(&text(0, 0, 32, "145.31000"));
        for (width, height) in [(24, 12), (64, 32)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| display.render(frame, frame.area()))
                .unwrap();
            let content: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(content.contains("NORTH RIDGE"), "{width}x{height}");
            assert!(content.contains("145.31000"), "{width}x{height}");
            assert!(
                !content
                    .chars()
                    .any(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
            );
        }
        display.apply(&rect(0, 0, 66, 8, 0));
        let mut terminal = Terminal::new(TestBackend::new(64, 32)).unwrap();
        terminal
            .draw(|frame| display.render(frame, frame.area()))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!content.contains('N'));
        assert!(content.contains("145.31000"));
    }

    #[test]
    fn large_frequencies_use_native_raster_and_narrow_views_remain_readable() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut display = RadioDisplay::default();
        display.apply(&rect(0, 0, 128, 128, 0xf800));
        display.apply(&text(2, 4, 24, "145.33500"));
        let mut large = Terminal::new(TestBackend::new(128, 64)).unwrap();
        large
            .draw(|frame| display.render(frame, frame.area()))
            .unwrap();
        assert!(
            large
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.symbol() == "▀")
        );
        let mut raster_only = display.clone();
        raster_only.text.clear();
        assert_eq!(large.backend().buffer(), &rendered(&raster_only, 128, 64));
        let mut small = Terminal::new(TestBackend::new(24, 12)).unwrap();
        small
            .draw(|frame| display.render(frame, frame.area()))
            .unwrap();
        let content: String = small
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("145.33500"));
        display.clear();
        small
            .draw(|frame| display.render(frame, frame.area()))
            .unwrap();
        let content: String = small
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!content.contains("145.33500"));
    }

    #[test]
    fn terminal_render_preserves_single_pixel_strokes_and_color() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut display = RadioDisplay::default();
        display.apply(&rect(1, 3, 1, 1, 0x001f));
        let mut terminal = Terminal::new(TestBackend::new(64, 32)).unwrap();
        terminal
            .draw(|frame| display.render(frame, frame.area()))
            .unwrap();
        let cell = &terminal.backend().buffer()[(0, 0)];
        assert_eq!(cell.symbol(), "▀");
        assert_eq!(cell.fg, BLACK);
        assert_eq!(cell.bg, wire_color(0x001f));
        let mut tiny = Terminal::new(TestBackend::new(1, 1)).unwrap();
        tiny.draw(|frame| display.render(frame, frame.area()))
            .unwrap();
    }

    #[test]
    fn rendering_clips_to_the_requested_buffer_region() {
        let mut display = RadioDisplay::default();
        display.apply(&rect(0, 0, 128, 128, 0x001f));
        display.apply(&text(2, 4, 24, "145.31000"));
        display.apply(&symbol(0, 120, 120));
        for area in [
            Rect::new(4, 3, 24, 12),
            Rect::new(29, 19, 8, 8),
            Rect::new(8, 5, 0, 2),
            Rect::new(8, 5, 2, 0),
            Rect::new(40, 30, 2, 2),
        ] {
            let mut buffer =
                Buffer::filled(Rect::new(0, 0, 32, 20), ratatui::buffer::Cell::new("X"));
            Widget::render(&display, area, &mut buffer);
            let clipped = area.intersection(buffer.area);
            for y in 0..20 {
                for x in 0..32 {
                    if !clipped.contains((x, y).into()) {
                        assert_eq!(buffer[(x, y)].symbol(), "X", "{area:?} at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn color_order_and_truncation_match_programmer() {
        assert_eq!(wire_color(0x001f), Color::Rgb(248, 0, 0));
        assert_eq!(wire_color(0x07e0), Color::Rgb(0, 252, 0));
        assert_eq!(wire_color(0xf800), Color::Rgb(0, 0, 248));
        assert_eq!(wire_color(0xffff), Color::Rgb(248, 252, 248));
    }

    #[test]
    fn rectangles_clip_without_wrapping_or_clamping_origins() {
        let mut display = RadioDisplay::default();
        display.apply(&rect(127, 127, 255, 255, 0x001f));
        assert_eq!(display.pixel(127, 127), wire_color(0x001f));
        assert_eq!(display.pixel(126, 127), BLACK);
        display.apply(&rect(255, 255, 255, 255, 0xffff));
        display.apply(&rect(0, 0, 0, 128, 0xffff));
        display.apply(&rect(0, 0, 128, 0, 0xffff));
        assert_eq!(display.pixel(0, 0), BLACK);
        assert_eq!(display.pixel(usize::MAX, usize::MAX), BLACK);
    }

    #[test]
    fn overlapping_draws_and_clear_are_ordered() {
        let mut display = RadioDisplay::default();
        display.apply(&rect(0, 0, 128, 128, 0x001f));
        display.apply(&text(0, 4, 5, "A"));
        display.apply(&rect(4, 5, 6, 8, 0));
        assert_eq!(display.pixel(4, 5), BLACK);
        assert_eq!(display.pixel(9, 12), BLACK);
        assert_eq!(display.pixel(10, 12), wire_color(0x001f));
        display.clear();
        assert!(display.pixels.iter().all(|pixel| *pixel == BLACK));
        assert_eq!(display.signal_bar_y, 0);
    }

    #[test]
    fn font_slots_clear_exact_cell_extents() {
        for (slot, width, height) in [
            (0, 6, 8),
            (1, 8, 8),
            (2, 8, 16),
            (3, 16, 16),
            (129, 16, 16),
            (130, 8, 16),
        ] {
            let mut display = RadioDisplay::default();
            display.apply(&text(slot, 3, 4, "  "));
            assert_eq!(
                display.pixel(3 + width * 2 - 1, 4 + height - 1),
                wire_color(0xf800)
            );
            assert_eq!(display.pixel(3 + width * 2, 4), BLACK);
            assert_eq!(display.pixel(3, 4 + height), BLACK);
        }
    }

    #[test]
    fn text_spaces_erase_and_shorter_text_preserves_trailing_cells() {
        let mut display = RadioDisplay::default();
        display.apply(&text(0, 0, 0, "AA"));
        let second = display.pixel(8, 0);
        assert_eq!(second, wire_color(0x001f));
        display.apply(&text(0, 0, 0, "@"));
        for y in 0..8 {
            for x in 0..6 {
                assert_eq!(display.pixel(x, y), wire_color(0xf800));
            }
        }
        assert_eq!(display.pixel(8, 0), second);
        display.apply(&text(0, 0, 0, "\0A"));
        assert_eq!(display.pixel(2, 0), wire_color(0xf800));
    }

    #[test]
    fn lower_case_is_not_forced_to_upper_case() {
        assert_ne!(glyph_rows('a'), glyph_rows('A'));
        assert_ne!(glyph_rows('m'), glyph_rows('M'));
    }

    #[test]
    fn framebuffer_survives_more_than_fifty_events_and_clone_is_independent() {
        let mut display = RadioDisplay::default();
        display.apply(&rect(2, 3, 1, 1, 0x001f));
        for id in 0..200 {
            display.apply(&RemotePacket::SmallStatus {
                id,
                value1: 0,
                value2: 0,
            });
            display.apply(&RemotePacket::NoiseLevel { level: id, mode: 0 });
        }
        assert_eq!(display.pixel(2, 3), wire_color(0x001f));
        let mut clone = display.clone();
        clone.clear();
        assert_eq!(display.pixel(2, 3), wire_color(0x001f));
        assert_eq!(clone.pixel(2, 3), BLACK);
    }

    #[test]
    fn symbols_use_programmer_ids_and_blank_erases() {
        let mut display = RadioDisplay::default();
        for id in 0..17 {
            display.apply(&symbol(id, 10, 20));
            let has_ink =
                (20..28).any(|y| (10..18).any(|x| display.pixel(x, y) == wire_color(0x001f)));
            assert_eq!(has_ink, id != 13, "symbol {id}");
        }
        display.apply(&symbol(13, 10, 20));
        for y in 20..28 {
            for x in 10..18 {
                assert_eq!(display.pixel(x, y), wire_color(0xf800));
            }
        }
        let before = display.pixels.clone();
        display.apply(&symbol(255, 10, 20));
        assert_eq!(display.pixels, before);
    }

    #[test]
    fn signal_bar_position_scaling_gaps_and_mode_match_programmer() {
        let mut display = RadioDisplay::default();
        display.apply(&RemotePacket::SignalBarPos { y: 40, aux: 255 });
        display.apply(&RemotePacket::SignalStrength {
            strength: 16,
            mode: 0,
            battery: 0,
        });
        assert_eq!(display.pixel(30, 40), Color::Rgb(50, 205, 50));
        assert_eq!(display.pixel(32, 40), Color::Rgb(40, 40, 40));
        assert_eq!(display.pixel(5, 40), BLACK);
        assert_eq!(display.pixel(4, 45), Color::Rgb(50, 205, 50));
        assert_eq!(display.pixel(4, 46), BLACK);
        display.apply(&RemotePacket::SignalStrength {
            strength: 255,
            mode: 1,
            battery: 0,
        });
        assert_eq!(display.pixel(122, 40), Color::Rgb(255, 165, 0));
        display.apply(&RemotePacket::SignalBarPos { y: 127, aux: 0 });
        display.apply(&RemotePacket::SignalStrength {
            strength: 1,
            mode: 0,
            battery: 0,
        });
        assert_eq!(display.pixel(4, 127), Color::Rgb(50, 205, 50));
    }

    #[test]
    fn noise_meter_preserves_overlapping_fills_and_maximum_edge() {
        let mut display = RadioDisplay::default();
        display.apply(&RemotePacket::SignalBarPos { y: 10, aux: 0 });
        display.apply(&RemotePacket::NoiseLevel { level: 1, mode: 0 });
        assert_eq!(display.pixel(4, 16), Color::Rgb(90, 90, 255));
        assert_eq!(display.pixel(5, 16), BLACK);
        display.apply(&RemotePacket::NoiseLevel {
            level: 255,
            mode: 1,
        });
        assert_eq!(display.pixel(127, 16), Color::Rgb(50, 205, 50));
        display.apply(&RemotePacket::NoiseLevel { level: 0, mode: 0 });
        assert_eq!(display.pixel(4, 16), BLACK);
        assert_eq!(display.pixel(123, 16), BLACK);
        assert_eq!(display.pixel(127, 16), Color::Rgb(50, 205, 50));
    }
}
