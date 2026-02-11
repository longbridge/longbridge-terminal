use bitflags::bitflags;
use std::time::Instant;

bitflags! {
    /// Flags to track which UI components need re-rendering
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DirtyFlags: u32 {
        /// Nothing needs rendering
        const NONE = 0;
        /// Watchlist view needs update (quote changes, subscription updates)
        const WATCHLIST = 0b0000_0001;
        /// Stock detail view needs update (individual stock data)
        const STOCK_DETAIL = 0b0000_0010;
        /// Portfolio view needs update (holdings, account balance)
        const PORTFOLIO = 0b0000_0100;
        /// Index carousel needs update (top market indexes)
        const INDEXES = 0b0000_1000;
        /// Help popup needs update
        const POPUP_HELP = 0b0001_0000;
        /// Search popup needs update
        const POPUP_SEARCH = 0b0010_0000;
        /// Account selector popup needs update
        const POPUP_ACCOUNT = 0b0100_0000;
        /// Currency selector popup needs update
        const POPUP_CURRENCY = 0b1000_0000;
        /// Watchlist group selector popup needs update
        const POPUP_WATCHLIST = 0b0001_0000_0000;
        /// Loading screen needs update
        const LOADING = 0b0010_0000_0000;
        /// Error screen needs update
        const ERROR = 0b0100_0000_0000;
        /// Status bar needs update (WebSocket connection status)
        const STATUS_BAR = 0b1000_0000_0000;
        /// Depth (order book) needs update
        const DEPTH = 0b0001_0000_0000_0000;
        /// All components need rendering (full redraw)
        const ALL = 0xFFFF_FFFF;
    }
}

impl DirtyFlags {
    /// Check if any component needs rendering
    #[inline]
    pub fn needs_render(self) -> bool {
        !self.is_empty()
    }

    /// Mark components for a quote update
    #[inline]
    #[must_use]
    pub fn mark_quote_update(mut self) -> Self {
        self.insert(Self::WATCHLIST | Self::STOCK_DETAIL | Self::INDEXES | Self::STATUS_BAR);
        self
    }

    /// Mark components for a depth (order book) update
    #[inline]
    #[must_use]
    pub fn mark_depth_update(mut self) -> Self {
        self.insert(Self::STOCK_DETAIL | Self::DEPTH);
        self
    }

    /// Mark components for a portfolio update
    #[inline]
    #[must_use]
    pub fn mark_portfolio_update(mut self) -> Self {
        self.insert(Self::PORTFOLIO);
        self
    }

    /// Mark components for a state change
    #[inline]
    #[must_use]
    pub fn mark_state_change(mut self) -> Self {
        self.insert(Self::ALL);
        self
    }

    /// Mark components for a popup change
    #[inline]
    #[must_use]
    pub fn mark_popup_change(mut self, popup: u8) -> Self {
        if popup & crate::app::POPUP_HELP != 0 {
            self.insert(Self::POPUP_HELP);
        }
        if popup & crate::app::POPUP_SEARCH != 0 {
            self.insert(Self::POPUP_SEARCH);
        }
        if popup & crate::app::POPUP_ACCOUNT != 0 {
            self.insert(Self::POPUP_ACCOUNT);
        }
        if popup & crate::app::POPUP_CURRENCY != 0 {
            self.insert(Self::POPUP_CURRENCY);
        }
        if popup & crate::app::POPUP_WATCHLIST != 0 {
            self.insert(Self::POPUP_WATCHLIST);
        }
        self
    }
}

/// Manages rendering state and tracks which components need updates
#[derive(Debug)]
pub struct RenderState {
    /// Dirty flags tracking which components need rendering
    dirty: DirtyFlags,
    /// Timestamp of the last successful render
    last_render: Instant,
    /// Total number of renders performed
    render_count: u64,
    /// Number of skipped renders (when nothing was dirty)
    skip_count: u64,
}

impl Default for RenderState {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderState {
    /// Create a new render state, initially clean
    pub fn new() -> Self {
        Self {
            dirty: DirtyFlags::NONE,
            last_render: Instant::now(),
            render_count: 0,
            skip_count: 0,
        }
    }

    /// Check if any component needs rendering
    #[inline]
    pub fn needs_render(&self) -> bool {
        self.dirty.needs_render()
    }

    /// Mark specific components as dirty
    #[inline]
    pub fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty.insert(flags);
    }

    /// Mark all components as dirty (full redraw)
    #[inline]
    pub fn mark_all_dirty(&mut self) {
        self.dirty = DirtyFlags::ALL;
    }

    /// Clear all dirty flags after successful render
    #[inline]
    pub fn clear(&mut self) {
        self.dirty = DirtyFlags::NONE;
        self.last_render = Instant::now();
        self.render_count += 1;
    }

    /// Increment skip counter when render is skipped
    #[inline]
    pub fn skip(&mut self) {
        self.skip_count += 1;
    }

    /// Get the current dirty flags
    #[inline]
    pub fn dirty(&self) -> DirtyFlags {
        self.dirty
    }

    /// Get time since last render
    #[inline]
    pub fn time_since_last_render(&self) -> std::time::Duration {
        self.last_render.elapsed()
    }

    /// Get rendering efficiency (percentage of renders that were skipped)
    #[allow(clippy::cast_precision_loss)]
    pub fn efficiency(&self) -> f64 {
        let total = self.render_count + self.skip_count;
        if total == 0 {
            0.0
        } else {
            (self.skip_count as f64 / total as f64) * 100.0
        }
    }

    /// Get statistics for logging/debugging
    pub fn stats(&self) -> String {
        format!(
            "renders: {}, skips: {}, efficiency: {:.1}%",
            self.render_count,
            self.skip_count,
            self.efficiency()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DirtyFlags, RenderState};

    #[test]
    fn test_dirty_flags() {
        let flags = DirtyFlags::NONE;
        assert!(!flags.needs_render());

        let flags = DirtyFlags::WATCHLIST | DirtyFlags::STOCK_DETAIL;
        assert!(flags.needs_render());
        assert!(flags.contains(DirtyFlags::WATCHLIST));
        assert!(flags.contains(DirtyFlags::STOCK_DETAIL));
        assert!(!flags.contains(DirtyFlags::PORTFOLIO));
    }

    #[test]
    fn test_mark_quote_update() {
        let flags = DirtyFlags::NONE.mark_quote_update();
        assert!(flags.contains(DirtyFlags::WATCHLIST));
        assert!(flags.contains(DirtyFlags::STOCK_DETAIL));
        assert!(flags.contains(DirtyFlags::INDEXES));
        assert!(!flags.contains(DirtyFlags::PORTFOLIO));
    }

    #[test]
    fn test_render_state() {
        let mut state = RenderState::new();
        assert!(!state.needs_render());

        state.mark_dirty(DirtyFlags::WATCHLIST);
        assert!(state.needs_render());

        state.clear();
        assert!(!state.needs_render());
        assert_eq!(state.render_count, 1);
    }

    #[test]
    fn test_efficiency_calculation() {
        let mut state = RenderState::new();

        // Simulate 3 renders and 7 skips
        for _ in 0..3 {
            state.mark_dirty(DirtyFlags::WATCHLIST);
            state.clear();
        }
        for _ in 0..7 {
            state.skip();
        }

        assert_eq!(state.render_count, 3);
        assert_eq!(state.skip_count, 7);
        assert!((state.efficiency() - 70.0).abs() < f64::EPSILON);
    }
}
