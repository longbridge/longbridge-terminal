//! `longbridge ai` — interactive Longbridge AI chat as a full-screen TUI.
//!
//! Structured after grok-build's layering, at a scale proportionate to a hosted
//! chat agent:
//! - [`answer`]  — what an answer is made of (segments, widgets, markers)
//! - [`chart`]   — `vis-chart` specs drawn as braille plots
//! - [`quotes`]  — live quotes for the securities an answer references
//! - [`state`]   — the chat state snapshot + event model (grok's `xai-chat-state`)
//! - [`runtime`] — the agent-runtime seam that streams a turn (grok's `xai-grok-shell`)
//! - [`tui`]     — the full-screen pager/view (grok's `xai-grok-pager`)
//!
//! The Longbridge AI model runs server-side and orchestrates its own tools, so
//! this reuses the shared streaming in [`crate::cli::agent::client`] and has no
//! local tool/workspace layer (unlike grok-build, which edits and runs code).

pub mod answer;
pub mod chart;
pub mod editor;
pub mod markdown;
pub mod quotes;
pub mod runtime;
pub mod session_store;
pub mod state;
pub mod tui;

pub use tui::run;
