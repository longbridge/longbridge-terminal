//! Shared wording for the order-execution gate.
//!
//! Every command that can move real money — `order buy/sell/cancel/replace`
//! and `grid submit/replace/cancel/suspend/restart` — previews by default and
//! acts only when the caller passes `--execute`. They share this notice so the
//! instruction an AI agent reads is identical no matter which one it ran.

/// Kept as separate lines so the pretty output wraps sensibly while the JSON
/// payload carries one flat sentence.
const NOTICE: &[&str] = &[
    "Nothing has been sent to the exchange.",
    "Review the details above, then re-run the identical command with --execute to go live.",
    "AI agents: show this preview to the user and only add --execute after they explicitly confirm it.",
];

/// The notice as a single sentence, for the `message` field of JSON output.
pub fn message() -> String {
    NOTICE.join(" ")
}

/// The notice as its own block, for human-readable output.
pub fn print_notice() {
    println!();
    for line in NOTICE {
        println!("{line}");
    }
}
