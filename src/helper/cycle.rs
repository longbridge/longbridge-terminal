pub fn prev(idx: Option<usize>, all: usize) -> Option<usize> {
    if let Some(idx) = idx {
        idx.checked_sub(1).or_else(|| all.checked_sub(1))
    } else {
        all.checked_sub(1)
    }
}

pub fn next(idx: Option<usize>, all: usize) -> Option<usize> {
    if let Some(idx) = idx {
        let next = idx + 1;
        if next < all {
            Some(next)
        } else {
            (all > 0).then_some(0)
        }
    } else {
        (all > 0).then_some(0)
    }
}

/// select previous item, with extra one for input source
pub fn prev_opt(idx: Option<usize>, all: usize) -> Option<usize> {
    if let Some(idx) = idx {
        if idx == 0 {
            None
        } else {
            Some(idx - 1)
        }
    } else {
        all.checked_sub(1)
    }
}

/// select next item, with extra one for input source
pub fn next_opt(idx: Option<usize>, all: usize) -> Option<usize> {
    if let Some(idx) = idx {
        let next = idx + 1;
        if next < all {
            Some(next)
        } else {
            None
        }
    } else {
        (all > 0).then_some(0)
    }
}
