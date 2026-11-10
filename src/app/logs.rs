use std::collections::VecDeque;

use crossterm::event::KeyCode;
use ratatui::{style::Style, text::Span};

use super::{App, AppMode, MainTab};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFilter {
    #[default]
    All,
    Rx,
    Tx,
    Remote,
    Errors,
}

impl LogFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Rx => "RX",
            Self::Tx => "TX",
            Self::Remote => "Remote",
            Self::Errors => "Errors",
        }
    }

    pub fn matches(self, line: &str) -> bool {
        let class = classify_log(line);
        match self {
            Self::All => true,
            Self::Rx => class.rx,
            Self::Tx => class.tx,
            Self::Remote => class.remote,
            Self::Errors => class.error,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::All => Self::Rx,
            Self::Rx => Self::Tx,
            Self::Tx => Self::Remote,
            Self::Remote => Self::Errors,
            Self::Errors => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogClass {
    pub rx: bool,
    pub tx: bool,
    pub remote: bool,
    pub error: bool,
}

/// Classify the message, not protocol-like text inside a payload.
pub fn classify_log(line: &str) -> LogClass {
    let message = line.trim_start();
    let message = message
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .map_or(message, |(_, message)| message);
    let lower = message.to_ascii_lowercase();
    let remote = lower.strip_prefix("remote").is_some_and(|rest| {
        rest.is_empty() || rest.starts_with(':') || rest.starts_with(char::is_whitespace)
    });
    let error = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| {
            matches!(
                word,
                "error" | "errors" | "failed" | "failure" | "fatal" | "timeout"
            )
        })
        || lower.contains("timed out");
    LogClass {
        rx: lower.starts_with("rx:"),
        tx: lower.starts_with("tx:"),
        remote,
        error,
    }
}

#[derive(Debug)]
pub struct LogView {
    /// Display rows from the newest matching entry, including wrapped continuations.
    pub scroll: usize,
    pub follow: bool,
    pub filter: LogFilter,
    width: u16,
}

impl Default for LogView {
    fn default() -> Self {
        Self {
            scroll: 0,
            follow: true,
            filter: LogFilter::All,
            width: 80,
        }
    }
}

impl LogView {
    /// Called before insertion, using the exact timestamped line that will be stored.
    pub(crate) fn on_append(&mut self, line: &str) {
        if !self.follow && self.filter.matches(line) {
            self.scroll = self.scroll.saturating_add(wrap_log(line, self.width).len());
        }
    }

    pub fn rows(&self, logs: &VecDeque<String>) -> Vec<(String, LogClass)> {
        logs.iter()
            .rev()
            .filter(|line| self.filter.matches(line))
            .flat_map(|line| {
                let class = classify_log(line);
                wrap_log(line, self.width)
                    .into_iter()
                    .map(move |row| (row, class))
            })
            .collect()
    }

    pub fn set_width(&mut self, width: u16) {
        self.width = width.max(1);
    }

    pub fn clamp(&mut self, row_count: usize) {
        // Keep the anchor even when eviction leaves a partially filled last page.
        self.scroll = if self.follow {
            0
        } else {
            self.scroll.min(row_count.saturating_sub(1))
        };
    }
}

// Hard-wrap at grapheme boundaries so navigation and rendering use identical row counts.
fn wrap_log(line: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut rows = Vec::new();
    for line in line.split('\n') {
        let span = Span::raw(line);
        let mut row = String::new();
        let mut used = 0;
        for grapheme in span.styled_graphemes(Style::default()) {
            let size = Span::raw(grapheme.symbol).width();
            if used + size > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push_str(grapheme.symbol);
            used += size;
        }
        rows.push(row);
    }
    rows
}

impl App {
    pub fn handle_log_key(&mut self, key: KeyCode, page_rows: usize) -> bool {
        if self.mode != AppMode::Main(MainTab::Debug) || self.dialog_open || self.help.open {
            return false;
        }
        match key {
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home => {
                self.log_view.follow = false;
                let page = page_rows.max(1);
                self.log_view.scroll = match key {
                    KeyCode::Up => self.log_view.scroll.saturating_sub(1),
                    KeyCode::Down => self.log_view.scroll.saturating_add(1),
                    KeyCode::PageUp => self.log_view.scroll.saturating_sub(page),
                    KeyCode::PageDown => self.log_view.scroll.saturating_add(page),
                    _ => 0,
                };
            }
            KeyCode::End => {
                self.log_view.follow = true;
                self.log_view.scroll = 0;
            }
            KeyCode::Char('f') => {
                self.log_view.filter = self.log_view.filter.next();
                self.log_view.scroll = 0;
            }
            KeyCode::Char('c') => {
                self.logs.clear();
                self.log_view.scroll = 0;
                self.log_view.follow = true;
                self.status_message = "Local log history cleared; radio unchanged".into();
            }
            _ => return false,
        }
        let row_count = self.log_view.rows(&self.logs).len();
        self.log_view.clamp(row_count);
        true
    }
}
