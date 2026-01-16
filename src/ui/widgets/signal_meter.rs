use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Gauge, Widget},
};

pub struct SignalMeter {
    pub strength: u8,
}

impl Widget for SignalMeter {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let sig_val = self.strength as f64 / 120.0;
        let sig_color = if sig_val < 0.4 {
            Color::Green
        } else if sig_val < 0.7 {
            Color::Yellow
        } else {
            Color::Red
        };

        let label = format!("Signal: {}/120", self.strength);

        Gauge::default()
            .block(
                Block::default()
                    .title(" SIGNAL STRENGTH ")
                    .borders(Borders::ALL),
            )
            .gauge_style(Style::default().fg(sig_color))
            .percent((sig_val * 100.0).min(100.0) as u16)
            .label(label)
            .render(area, buf);
    }
}
