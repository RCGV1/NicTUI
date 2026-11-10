use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;

use super::{App, AppMode, MainTab};

impl App {
    /// Move within the current table without wrapping at page or endpoint boundaries.
    pub fn handle_table_navigation(&mut self, key: KeyCode, page_rows: usize) -> bool {
        if !matches!(
            key,
            KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown
        ) || self.dialog_open
        {
            return false;
        }
        let (len, state) = match self.mode {
            AppMode::Main(MainTab::Channels) => (self.channels.len(), &mut self.channel_state),
            AppMode::Main(MainTab::Settings) => (
                if self.settings.is_some() {
                    crate::protocol::SETTINGS_METADATA.len()
                } else {
                    0
                },
                &mut self.settings_state,
            ),
            AppMode::Main(MainTab::Scanning) => (self.scan_presets.len(), &mut self.preset_state),
            AppMode::Main(MainTab::MemoryGroups) => (
                crate::protocol::GROUP_LABEL_COUNT,
                &mut self.scanning_group_state,
            ),
            AppMode::Main(MainTab::BandPlan) => (self.band_plans.len(), &mut self.bandplan_state),
            AppMode::Main(MainTab::DTMF) => (self.dtmf_presets.len(), &mut self.dtmf_state),
            AppMode::Main(MainTab::Activity) => (
                self.activity.lock().unwrap().log.rows().count(),
                &mut self.activity_state,
            ),
            _ => return false,
        };
        if len == 0 {
            state.select(None);
        } else {
            let current = state.selected().filter(|&index| index < len);
            let index = match key {
                KeyCode::Home => 0,
                KeyCode::End => len - 1,
                KeyCode::PageUp => current.map_or(0, |i| i.saturating_sub(page_rows.max(1))),
                KeyCode::PageDown => {
                    current.map_or(0, |i| i.saturating_add(page_rows.max(1)).min(len - 1))
                }
                _ => unreachable!(),
            };
            state.select(Some(index));
        }
        true
    }

    /// Intercept before per-tab routing, especially Remote's modifier filter.
    pub fn handle_read_all_key(&mut self, key: KeyEvent) -> bool {
        if !matches!(key.code, KeyCode::Char('r' | 'R')) {
            return false;
        }
        let plain_r = key.code == KeyCode::Char('r') && key.modifiers.is_empty();
        let shortcut = (key.code == KeyCode::Char('R') && key.modifiers.is_empty())
            || key.modifiers == KeyModifiers::SHIFT
            || key.modifiers == KeyModifiers::CONTROL;
        if matches!(self.mode, AppMode::Main(_)) {
            if plain_r {
                return false;
            }
            if shortcut && key.kind == KeyEventKind::Press {
                self.start_read_all();
            }
            // Do not let unsupported modified r chords fall through to a section read.
            return true;
        }
        // Preserve uppercase text entry in editors, but never type Ctrl+R as plain r.
        !plain_r
            && (!key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT
                || matches!(
                    self.mode,
                    AppMode::Reading | AppMode::Writing | AppMode::BinFlashing
                ))
    }
}

pub fn next_item(list_len: usize, state: &mut TableState) {
    if list_len == 0 {
        state.select(None);
        return;
    }
    let i = match state.selected() {
        Some(i) => {
            if i >= list_len - 1 {
                0
            } else {
                i + 1
            }
        }
        None => 0,
    };
    state.select(Some(i));
}

pub fn prev_item(list_len: usize, state: &mut TableState) {
    if list_len == 0 {
        state.select(None);
        return;
    }
    let i = match state.selected() {
        Some(i) if i < list_len => {
            if i == 0 {
                list_len - 1
            } else {
                i - 1
            }
        }
        _ => 0,
    };
    state.select(Some(i));
}

pub fn update_selection_after_remove(state: &mut TableState, removed_idx: usize, list_len: usize) {
    state.select(None);
    if removed_idx < list_len {
        state.select(Some(removed_idx));
    } else if list_len > 0 {
        state.select(Some(list_len - 1));
    }
}

pub fn update_selection_after_add(state: &mut TableState, new_idx: usize) {
    state.select(None);
    state.select(Some(new_idx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lists_clear_selection_in_both_directions() {
        for selected in [None, Some(0), Some(1), Some(usize::MAX)] {
            let mut state = TableState::default();
            state.select(selected);
            next_item(0, &mut state);
            assert_eq!(state.selected(), None);

            state.select(selected);
            prev_item(0, &mut state);
            assert_eq!(state.selected(), None);
        }
    }

    #[test]
    fn stale_selections_restart_at_first_item_in_both_directions() {
        for list_len in [1, 3] {
            for selected in [list_len, list_len + 1, usize::MAX] {
                let mut state = TableState::default();
                state.select(Some(selected));
                next_item(list_len, &mut state);
                assert_eq!(state.selected(), Some(0));

                state.select(Some(selected));
                prev_item(list_len, &mut state);
                assert_eq!(state.selected(), Some(0));
            }
        }
    }

    #[test]
    fn unselected_lists_start_at_first_item_in_both_directions() {
        let mut state = TableState::default();
        next_item(3, &mut state);
        assert_eq!(state.selected(), Some(0));

        state.select(None);
        prev_item(3, &mut state);
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn valid_selections_advance_and_wrap_in_both_directions() {
        for (selected, next, previous) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
            let mut state = TableState::default();
            state.select(Some(selected));
            next_item(3, &mut state);
            assert_eq!(state.selected(), Some(next));

            state.select(Some(selected));
            prev_item(3, &mut state);
            assert_eq!(state.selected(), Some(previous));
        }
    }

    #[test]
    fn single_item_lists_keep_selection_in_both_directions() {
        let mut state = TableState::default();
        state.select(Some(0));
        next_item(1, &mut state);
        assert_eq!(state.selected(), Some(0));

        prev_item(1, &mut state);
        assert_eq!(state.selected(), Some(0));
    }
}
