use crate::ui::theme::{COLOR_BORDER, COLOR_ERROR, COLOR_SUCCESS, COLOR_SURFACE_1, COLOR_WARNING};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Gauge, Widget},
};

#[allow(dead_code)]
pub struct SignalMeter {
    pub strength: u8,
}

impl Widget for SignalMeter {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let sig_val = self.strength as f64 / 120.0;
        let sig_color = if sig_val < 0.4 {
            COLOR_SUCCESS
        } else if sig_val < 0.7 {
            COLOR_WARNING
        } else {
            COLOR_ERROR
        };

        let label = format!("Signal: {}/120", self.strength);

        Gauge::default()
            .block(
                Block::default()
                    .title(" SIGNAL STRENGTH ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_BORDER)),
            )
            .gauge_style(Style::default().fg(sig_color).bg(COLOR_SURFACE_1))
            .percent((sig_val * 100.0).min(100.0) as u16)
            .label(label)
            .render(area, buf);
    }
}
