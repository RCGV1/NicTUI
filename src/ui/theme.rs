use ratatui::style::Color;

pub const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

pub const COLOR_SURFACE_0: Color = Color::Rgb(13, 19, 28);
pub const COLOR_SURFACE_1: Color = Color::Rgb(18, 26, 36);
pub const COLOR_SURFACE_2: Color = Color::Rgb(24, 34, 46);
pub const COLOR_SURFACE_3: Color = Color::Rgb(32, 45, 61);
pub const COLOR_PRIMARY: Color = Color::Rgb(102, 214, 193);
pub const COLOR_SECONDARY: Color = Color::Rgb(134, 162, 255);
pub const COLOR_ACCENT: Color = Color::Rgb(245, 194, 98);
pub const COLOR_SIDEBAR: Color = COLOR_SURFACE_0;
pub const COLOR_HEADER: Color = COLOR_SURFACE_1;
pub const COLOR_SUCCESS: Color = Color::Rgb(113, 208, 153);
pub const COLOR_ERROR: Color = Color::Rgb(255, 110, 110);
pub const COLOR_WARNING: Color = Color::Rgb(245, 182, 72);
pub const COLOR_DIM: Color = Color::Rgb(124, 137, 153);
pub const COLOR_BORDER: Color = COLOR_PRIMARY;
pub const COLOR_TEXT: Color = Color::Rgb(225, 231, 238);
pub const COLOR_SELECTION_BG: Color = Color::Rgb(41, 60, 82);
pub const COLOR_SELECTION_FG: Color = COLOR_TEXT;
