use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{App, AppMode};

#[derive(Default)]
pub struct HelpState {
    pub open: bool,
    pub scroll: u16,
}

impl App {
    /// Help is an overlay: never replace the underlying mode or editor draft.
    pub fn handle_help_key(&mut self, key: KeyEvent, page_rows: usize) -> bool {
        if self.dialog_open {
            return false;
        }
        let plain = (key.modifiers & !KeyModifiers::SHIFT).is_empty();
        let toggle = key.code == KeyCode::F(1)
            || (key.code == KeyCode::Char('?')
                && matches!(self.mode, AppMode::Main(_) | AppMode::PortSelection));
        if plain && toggle && key.kind == KeyEventKind::Press {
            if !self.help.open {
                // Release is safe even when the session is no longer ready for input.
                self.release_remote_key();
                self.help.scroll = 0;
            }
            self.help.open = !self.help.open;
            return true;
        }
        if !self.help.open {
            return false;
        }
        if plain && key.kind != KeyEventKind::Release {
            let page = page_rows.clamp(1, usize::from(u16::MAX)) as u16;
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') if key.kind == KeyEventKind::Press => {
                    self.help.open = false;
                }
                KeyCode::Up => self.help.scroll = self.help.scroll.saturating_sub(1),
                KeyCode::Down => self.help.scroll = self.help.scroll.saturating_add(1),
                KeyCode::PageUp => self.help.scroll = self.help.scroll.saturating_sub(page),
                KeyCode::PageDown => self.help.scroll = self.help.scroll.saturating_add(page),
                KeyCode::Home => self.help.scroll = 0,
                KeyCode::End => self.help.scroll = u16::MAX,
                _ => {}
            }
        }
        // Consume everything else, including modified shortcuts and editor input.
        true
    }
}
