use atomic::Atomic;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, bytemuck::NoUninit)]
#[repr(u8)]
pub enum PopupKind {
    #[default]
    None,
    Help,
    Search,
    Account,
    Currency,
    Watchlist,
    WatchlistSearch,
    OrderEntry,
    CancelOrder,
    ReplaceOrder,
    DateFilter,
}

impl PopupKind {
    #[inline]
    pub fn is_open(self) -> bool {
        self != Self::None
    }
}

pub static POPUP_STATE: Atomic<PopupKind> = Atomic::new(PopupKind::None);

pub fn open(kind: PopupKind) {
    POPUP_STATE.store(kind, Ordering::Relaxed);
}

pub fn close() {
    POPUP_STATE.store(PopupKind::None, Ordering::Relaxed);
}

pub fn current() -> PopupKind {
    POPUP_STATE.load(Ordering::Relaxed)
}
