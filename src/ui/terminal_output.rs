use std::io::{self, Write};

use super::TerminalPalette;

const LIMIT: usize = 128;

#[derive(Clone, Copy)]
enum State {
    Text,
    Escape,
    EscapePassthrough,
    Csi,
    CsiPassthrough,
    String { osc: bool, escaped: bool },
}

/// Convert indexed SGR colors to classic ANSI syntax only for the 16-color profile.
pub struct TerminalWriter<W: Write> {
    inner: W,
    enabled: bool,
    state: State,
    escape: [u8; LIMIT],
    escape_len: usize,
    output: [u8; LIMIT],
    output_len: usize,
    output_pos: usize,
}

impl<W: Write> TerminalWriter<W> {
    pub fn new(inner: W, palette: TerminalPalette) -> Self {
        Self {
            inner,
            enabled: palette == TerminalPalette::Ansi16,
            state: State::Text,
            escape: [0; LIMIT],
            escape_len: 0,
            output: [0; LIMIT],
            output_len: 0,
            output_pos: 0,
        }
    }

    fn drain_output(&mut self) -> io::Result<()> {
        while self.output_pos < self.output_len {
            match self
                .inner
                .write(&self.output[self.output_pos..self.output_len])
            {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => self.output_pos += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        self.output_pos = 0;
        self.output_len = 0;
        Ok(())
    }

    fn emit_escape(&mut self) {
        self.output[..self.escape_len].copy_from_slice(&self.escape[..self.escape_len]);
        self.output_len = self.escape_len;
        self.escape_len = 0;
    }
}

impl<W: Write> Write for TerminalWriter<W> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if !self.enabled {
            return self.inner.write(input);
        }
        self.drain_output()?;
        let mut consumed = 0;
        while consumed < input.len() {
            let byte = input[consumed];
            match self.state {
                State::Text => {
                    if byte != 0x1b {
                        let end = input[consumed..]
                            .iter()
                            .position(|&byte| byte == 0x1b)
                            .map_or(input.len(), |index| consumed + index);
                        // Text has no buffered prefix, so preserve the inner writer's
                        // short-write and error semantics on this common path.
                        return self.inner.write(&input[consumed..end]);
                    }
                    self.escape[0] = byte;
                    self.escape_len = 1;
                    self.state = State::Escape;
                    consumed += 1;
                }
                State::Escape | State::EscapePassthrough => {
                    let buffered = matches!(self.state, State::Escape);
                    if buffered && byte == b'[' {
                        self.escape[1] = byte;
                        self.escape_len = 2;
                        self.state = State::Csi;
                        consumed += 1;
                        continue;
                    }
                    if buffered {
                        self.emit_escape();
                    }
                    if byte == 0x1b {
                        self.escape[0] = byte;
                        self.escape_len = 1;
                        self.state = State::Escape;
                        return Ok(consumed + 1);
                    }
                    self.state = match byte {
                        b'[' => State::CsiPassthrough,
                        b']' | b'P' | b'X' | b'^' | b'_' => State::String {
                            osc: byte == b']',
                            escaped: false,
                        },
                        _ => State::Text,
                    };
                    self.output[self.output_len] = byte;
                    self.output_len += 1;
                    return Ok(consumed + 1);
                }
                State::Csi => {
                    if byte == 0x1b {
                        self.emit_escape();
                        self.escape[0] = byte;
                        self.escape_len = 1;
                        self.state = State::Escape;
                        return Ok(consumed + 1);
                    }
                    if self.escape_len == LIMIT {
                        self.emit_escape();
                        self.state = State::CsiPassthrough;
                        if consumed != 0 {
                            return Ok(consumed);
                        }
                        self.drain_output()?;
                        continue;
                    }
                    self.escape[self.escape_len] = byte;
                    self.escape_len += 1;
                    consumed += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        if byte == b'm' {
                            self.output_len =
                                rewrite_sgr(&self.escape[..self.escape_len], &mut self.output);
                            self.escape_len = 0;
                        } else {
                            self.emit_escape();
                        }
                        self.state = State::Text;
                        return Ok(consumed);
                    }
                    if !(0x20..=0x3f).contains(&byte) {
                        self.emit_escape();
                        self.state = State::Text;
                        return Ok(consumed);
                    }
                }
                State::CsiPassthrough => {
                    if byte == 0x1b {
                        if self.output_len != 0 {
                            return Ok(consumed);
                        }
                        self.state = State::Text;
                        continue;
                    }
                    self.output[self.output_len] = byte;
                    self.output_len += 1;
                    consumed += 1;
                    if (0x40..=0x7e).contains(&byte) || !(0x20..=0x3f).contains(&byte) {
                        self.state = State::Text;
                        return Ok(consumed);
                    }
                    if self.output_len == LIMIT || consumed == input.len() {
                        return Ok(consumed);
                    }
                }
                State::String { osc, escaped } => {
                    self.output[self.output_len] = byte;
                    self.output_len += 1;
                    consumed += 1;
                    if (osc && byte == 7) || (escaped && byte == b'\\') {
                        self.state = State::Text;
                        return Ok(consumed);
                    }
                    self.state = State::String {
                        osc,
                        escaped: byte == 0x1b,
                    };
                    if self.output_len == LIMIT || consumed == input.len() {
                        return Ok(consumed);
                    }
                }
            }
        }
        Ok(consumed)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.enabled {
            self.drain_output()?;
            if self.escape_len != 0 {
                self.emit_escape();
                // Flush must emit incomplete input too. Its remaining bytes can
                // no longer be rewritten, but still need escape/string tracking.
                self.state = match self.state {
                    State::Escape => State::EscapePassthrough,
                    State::Csi => State::CsiPassthrough,
                    state => state,
                };
                self.drain_output()?;
            }
        }
        self.inner.flush()
    }
}

fn number(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0_u16, |value, &byte| {
        if byte.is_ascii_digit() {
            value.checked_mul(10)?.checked_add(u16::from(byte - b'0'))
        } else {
            None
        }
    })
}

fn push_parameter(output: &mut [u8; LIMIT], len: &mut usize, first: &mut bool, value: &[u8]) {
    if !*first {
        output[*len] = b';';
        *len += 1;
    }
    output[*len..*len + value.len()].copy_from_slice(value);
    *len += value.len();
    *first = false;
}

fn rewrite_sgr(input: &[u8], output: &mut [u8; LIMIT]) -> usize {
    if !input[2..input.len() - 1]
        .iter()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b';' | b':'))
    {
        output[..input.len()].copy_from_slice(input);
        return input.len();
    }
    output[..2].copy_from_slice(b"\x1b[");
    let mut len = 2;
    let mut first = true;
    let mut parameters = input[2..input.len() - 1].split(|&byte| byte == b';');
    while let Some(parameter) = parameters.next() {
        let code = number(parameter);
        if matches!(code, Some(38 | 48 | 58)) {
            let mut following = parameters.clone();
            let mode = following.next().and_then(number);
            let index = following.next().and_then(number);
            if mode == Some(5) && index.is_some_and(|index| index <= 255) {
                let index = index.unwrap();
                if code == Some(58) {
                    parameters = following;
                    continue;
                }
                if index < 16 {
                    let mapped = if code == Some(38) {
                        if index < 8 {
                            30 + index
                        } else {
                            90 + index - 8
                        }
                    } else if index < 8 {
                        40 + index
                    } else {
                        100 + index - 8
                    };
                    let digits = [
                        b'0' + (mapped / 100) as u8,
                        b'0' + (mapped / 10 % 10) as u8,
                        b'0' + (mapped % 10) as u8,
                    ];
                    push_parameter(
                        output,
                        &mut len,
                        &mut first,
                        &digits[if mapped >= 100 { 0 } else { 1 }..],
                    );
                    parameters = following;
                    continue;
                }
            }
            // Extended color payload values are not independent SGR attributes.
            // In particular, RGB components may themselves be 38, 48, or 58.
            push_parameter(output, &mut len, &mut first, parameter);
            let payload = match mode {
                Some(2) => 4,
                Some(5) => 2,
                _ => 0,
            };
            for value in parameters.by_ref().take(payload) {
                push_parameter(output, &mut len, &mut first, value);
            }
        } else {
            push_parameter(output, &mut len, &mut first, parameter);
        }
    }
    if first {
        // Removing only an underline color must not emit ESC[m (a style reset).
        0
    } else {
        output[len] = b'm';
        len + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use ratatui::{
        backend::{Backend, CrosstermBackend},
        buffer::Cell,
        style::Color,
    };

    fn convert(input: &[u8], palette: TerminalPalette, chunk: usize) -> Vec<u8> {
        let mut output = Vec::new();
        let mut writer = TerminalWriter::new(&mut output, palette);
        for part in input.chunks(chunk) {
            writer.write_all(part).unwrap();
        }
        writer.flush().unwrap();
        output
    }

    #[test]
    fn compound_sgr_converts_all_sixteen_colors_at_every_split() {
        for index in 0..16 {
            let input = format!("a\x1b[1;38;5;{index};4;48;5;{index};22;39;49mb");
            let foreground = if index < 8 {
                30 + index
            } else {
                90 + index - 8
            };
            let background = if index < 8 {
                40 + index
            } else {
                100 + index - 8
            };
            let expected = format!("a\x1b[1;{foreground};4;{background};22;39;49mb");
            for split in 0..=input.len() {
                let mut output = Vec::new();
                let mut writer = TerminalWriter::new(&mut output, TerminalPalette::Ansi16);
                writer.write_all(&input.as_bytes()[..split]).unwrap();
                writer.write_all(&input.as_bytes()[split..]).unwrap();
                writer.flush().unwrap();
                assert_eq!(output, expected.as_bytes(), "index {index}, split {split}");
            }
            assert_eq!(
                convert(input.as_bytes(), TerminalPalette::Ansi16, 1),
                expected.as_bytes()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn backend_output_uses_classic_colors_only_in_ansi16() {
        if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
            // Crossterm memoizes NO_COLOR globally. Isolate this wire-format
            // assertion rather than changing shared environment or color state.
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ui::terminal_output::tests::backend_output_uses_classic_colors_only_in_ansi16",
                ])
                .env_remove("NO_COLOR")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        for palette in [TerminalPalette::Ansi16, TerminalPalette::Ansi256] {
            let mut output = Vec::new();
            let mut cell = Cell::default();
            cell.set_symbol("X")
                .set_fg(Color::Red)
                .set_bg(Color::LightBlue);
            let mut backend = CrosstermBackend::new(TerminalWriter::new(&mut output, palette));
            backend.draw([(0, 0, &cell)].into_iter()).unwrap();
            Backend::flush(&mut backend).unwrap();
            let output = String::from_utf8(output).unwrap();
            if palette == TerminalPalette::Ansi16 {
                assert!(output.contains("\x1b[31;104m"), "{output:?}");
                assert!(!output.contains("38;5;") && !output.contains("48;5;"));
            } else {
                assert!(output.contains("\x1b[38;5;1;48;5;12m"), "{output:?}");
            }
        }
    }

    #[test]
    fn other_palettes_are_exact_passthrough() {
        let input = "utf8: \u{00e9}\u{2580}\x1b[38;5;1;48;5;15m\x1b]0;title\x07".as_bytes();
        for palette in [
            TerminalPalette::TrueColor,
            TerminalPalette::Ansi256,
            TerminalPalette::Monochrome,
        ] {
            for chunk in 1..=input.len() {
                assert_eq!(convert(input, palette, chunk), input);
            }
        }
    }

    #[test]
    fn unrelated_sequences_payloads_and_utf8_are_preserved() {
        let input = concat!(
            "\u{00e9}\u{2580}\x1b[2J\x1b[12;38H\x1b[?25l",
            "\x1b]0;title \x1b[38;5;1m\x07",
            "\x1b]8;;https://example.test/\x1b[48;5;2m\x1b\\",
            "\x1bPpayload\x1b[38;5;3m\x1b\\",
            "\x1b[38;2;38;5;1;48;2;48;5;2m",
            "\x1b[38;5;200;48;5;255;4:3m",
            "\x1b[38;5;1;1 m",
            "\x1b[0;;1m\x1b[31m\x1b[39;49m"
        )
        .as_bytes();
        for chunk in 1..=input.len() {
            assert_eq!(
                convert(input, TerminalPalette::Ansi16, chunk),
                input,
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn underline_color_is_removed_without_resetting_other_styles() {
        for chunk in 1..=32 {
            assert_eq!(
                convert(
                    b"\x1b[58;5;9mX\x1b[1;58;5;4;4mY\x1b[;58;5;1m",
                    TerminalPalette::Ansi16,
                    chunk
                ),
                b"X\x1b[1;4mY\x1b[m"
            );
        }
    }

    #[test]
    fn oversized_and_flushed_incomplete_sequences_are_preserved() {
        let input = format!("\x1b[{}38;5;1m\x1b[38;5;2m", "1;".repeat(LIMIT));
        let expected = format!("\x1b[{}38;5;1m\x1b[32m", "1;".repeat(LIMIT));
        for chunk in 1..=input.len() {
            assert_eq!(
                convert(input.as_bytes(), TerminalPalette::Ansi16, chunk),
                expected.as_bytes()
            );
        }
        for prefix in [b"\x1b".as_slice(), b"\x1b[38;5;"] {
            let mut output = Vec::new();
            let mut writer = TerminalWriter::new(&mut output, TerminalPalette::Ansi16);
            writer.write_all(prefix).unwrap();
            writer.flush().unwrap();
            writer.write_all(b"1m\x1b[48;5;3m").unwrap();
            writer.flush().unwrap();
            assert_eq!(output, [prefix, b"1m\x1b[43m"].concat());
        }
        let input = format!("\x1b[{}\x1b[38;5;1m", "1;".repeat(LIMIT));
        let expected = format!("\x1b[{}\x1b[31m", "1;".repeat(LIMIT));
        for chunk in 1..=input.len() {
            assert_eq!(
                convert(input.as_bytes(), TerminalPalette::Ansi16, chunk),
                expected.as_bytes()
            );
        }
        assert_eq!(
            convert(b"\x1b\x1b[38;5;1m", TerminalPalette::Ansi16, 1),
            b"\x1b\x1b[31m"
        );
    }

    #[derive(Default)]
    struct ShortWriter {
        bytes: Vec<u8>,
        fail_at: Option<usize>,
        fail_flush: bool,
    }

    impl Write for ShortWriter {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            if self.fail_at == Some(self.bytes.len()) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let count = input.len().min(1);
            self.bytes.extend_from_slice(&input[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn short_writes_and_errors_preserve_pending_output_for_retry() {
        let mut writer = TerminalWriter::new(ShortWriter::default(), TerminalPalette::Ansi16);
        writer.inner.fail_at = Some(3);
        writer.write_all(b"\x1b[38;5;1m").unwrap();
        assert_eq!(
            writer.flush().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(writer.inner.bytes, b"\x1b[3");
        writer.inner.fail_at = None;
        writer.flush().unwrap();
        assert_eq!(writer.inner.bytes, b"\x1b[31m");
        writer.inner.fail_at = Some(writer.inner.bytes.len());
        assert_eq!(
            writer.write(b"text").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        writer.inner.fail_at = None;
        writer.inner.fail_flush = true;
        assert_eq!(
            writer.flush().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
