use ratatui::widgets::TableState;

pub fn next_item(list_len: usize, state: &mut TableState) {
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
    let i = match state.selected() {
        Some(i) => {
            if i == 0 {
                list_len - 1
            } else {
                i - 1
            }
        }
        None => 0,
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
