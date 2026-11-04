use ratatui::widgets::TableState;

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
