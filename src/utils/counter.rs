//! Symbol <-> `counter_id` conversion helpers.
//!
//! Re-exported from the `longport` SDK's public `counter` module so the CLI
//! and the SDK share a single implementation (including the embedded ETF / IX /
//! WT special-counter set).
pub use longport::counter::{counter_id_to_symbol, is_etf, symbol_to_counter_id};
