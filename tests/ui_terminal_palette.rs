use nictui::{
    app::{App, AppMode, MainTab},
    ui::{self, TerminalPalette},
};
use ratatui::{Terminal, backend::TestBackend, style::Color};

const TABS: [MainTab; 11] = [
    MainTab::Channels,
    MainTab::Settings,
    MainTab::Scanning,
    MainTab::MemoryGroups,
    MainTab::BandPlan,
    MainTab::DTMF,
    MainTab::Remote,
    MainTab::Codeplug,
    MainTab::BinFlash,
    MainTab::Debug,
    MainTab::Activity,
];

#[test]
fn every_view_keeps_its_text_and_geometry_in_each_terminal_palette() {
    let mut app = App::demo();
    app.simulator_mode = true;
    for tab in TABS {
        app.mode = AppMode::Main(tab);
        app.last_main_tab = tab;
        for (width, height) in [
            (60, 20),
            (80, 24),
            (100, 30),
            (120, 36),
            (160, 48),
            (190, 80),
        ] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| ui::ui(frame, &mut app)).unwrap();
            let original = terminal.backend().buffer().clone();
            for palette in [
                TerminalPalette::TrueColor,
                TerminalPalette::Ansi256,
                TerminalPalette::Ansi16,
                TerminalPalette::Monochrome,
            ] {
                let mut mapped = original.clone();
                palette.apply(&mut mapped);
                assert_eq!(mapped.area, original.area);
                assert_eq!(mapped.content.len(), original.content.len());
                for (before, after) in original.content.iter().zip(&mapped.content) {
                    if palette != TerminalPalette::Monochrome
                        || !matches!(before.symbol(), " " | "▀" | "▄" | "█")
                    {
                        assert_eq!(
                            before.symbol(),
                            after.symbol(),
                            "{tab:?}/{width}/{palette:?}"
                        );
                    }
                    if palette != TerminalPalette::TrueColor {
                        assert!(!matches!(after.fg, Color::Rgb(..)));
                        assert!(!matches!(after.bg, Color::Rgb(..)));
                    }
                    if palette == TerminalPalette::Ansi16 {
                        assert!(!matches!(after.fg, Color::Indexed(_)));
                        assert!(!matches!(after.bg, Color::Indexed(_)));
                    }
                    if palette == TerminalPalette::Monochrome {
                        assert_eq!((after.fg, after.bg), (Color::Reset, Color::Reset));
                    }
                }
                let text: String = mapped.content.iter().map(|cell| cell.symbol()).collect();
                assert!(text.contains("NicTUI"));
            }
        }
    }
}

#[test]
fn colorless_native_lcd_retains_both_halves_of_a_pixel_pattern() {
    let pixels: Vec<u16> = (0..128 * 128)
        .map(|index| {
            if (index / 128 + index % 128) % 2 == 0 {
                0xffff
            } else {
                0
            }
        })
        .collect();
    let mut display = nictui::remote_display::RadioDisplay::default();
    display.set_native_frame(&pixels).unwrap();
    let area = ratatui::layout::Rect::new(0, 0, 128, 64);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Widget::render(&display, area, &mut buffer);
    TerminalPalette::Monochrome.apply(&mut buffer);
    for y in 0..64 {
        for x in 0..128 {
            assert_eq!(buffer[(x, y)].symbol(), if x % 2 == 0 { "▀" } else { "▄" });
        }
    }
}
