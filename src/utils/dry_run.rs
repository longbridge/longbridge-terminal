//! The order-execution gate.
//!
//! Every command that can move real money — `order buy/sell/cancel/replace`
//! and `grid submit/replace/cancel/suspend/restart` — previews by default and
//! acts only when the caller passes `--execute <CODE>`, quoting the three-digit
//! code the preview printed.
//!
//! # What it is for
//!
//! Stopping the ordinary mistake: acting on an order nobody previewed, or on a
//! *different* order than the one previewed. Change the price after reading the
//! code and it stops matching — the user approved one order and a different one
//! was about to be sent, which is the case worth catching.
//!
//! It is not a secret, and is not meant to be. The arguments are the caller's
//! own and this file is public, so anyone set on skipping the preview can
//! compute a code instead of asking for one. Nor does it prove a *human* saw
//! the preview: a caller can dry-run, read the code and execute in one breath.
//! Enforcing human review needs a control outside this process, such as a
//! harness approval hook or account-level trade confirmation.
//!
//! # Derived, not stored
//!
//! The code is three digits off a digest of the canonicalised arguments, and
//! nothing else — no clock, no file, no per-run value. Storing a pending code
//! instead would buy single use, and cost a list of ways to fail that have
//! nothing to do with the order: previewing twice would strand the first code,
//! an unwritable home directory would break the gate, and a code would die
//! while the user was still deciding.
//!
//! # Tolerant on purpose
//!
//! Inputs are canonicalised before hashing, so a code survives the harmless
//! rewordings between the two runs: `400` and `400.00` are the same price,
//! `buy` and `Buy` the same side, `700.hk` and `700.HK` the same symbol. A
//! confirmation that fails on a difference the user cannot see is worse than no
//! confirmation at all — it teaches people to distrust the gate.

use anyhow::{bail, Result};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::str::FromStr;

/// Canonical form of one argument.
///
/// `Decimal` first: it collapses `400`, `400.0`, `400.00` and `+400`, which is
/// where two runs differ most often. Anything that is not a number is trimmed
/// and upper-cased — symbols, sides, order types and tenors are all matched
/// case-insensitively downstream, so treating them as distinct here would
/// reject an order the exchange considers identical. Free text (a remark)
/// survives unchanged apart from case, because different text really is a
/// different order.
fn canonical(field: &str) -> String {
    let trimmed = field.trim();
    Decimal::from_str(trimmed)
        .map_or_else(|_| trimmed.to_uppercase(), |n| n.normalize().to_string())
}

/// Stable identity of the action being previewed. Any argument that changes
/// what would reach the exchange belongs in `parts`.
pub fn fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    // Domain separator: this digest must never coincide with any other use of
    // the same inputs elsewhere in the CLI.
    hasher.update(b"longbridge/execute-confirmation/v1");
    for part in parts {
        let part = canonical(part);
        // Length-prefixed so ["ab", "c"] and ["a", "bc"] cannot collide.
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// The code for one request.
pub fn code_for(fingerprint: &str) -> String {
    let digest = Sha256::digest(fingerprint.as_bytes());
    let n = (u32::from(digest[0]) << 8) | u32::from(digest[1]);
    format!("{:03}", n % 1000)
}

/// Check the code the caller quoted back.
///
/// The only way to fail is to quote a code for a *different* order. Leading
/// zeros and stray whitespace are forgiven — a code retyped as `7` is still the
/// code this order was given.
pub fn verify(fingerprint: &str, code: &str) -> Result<()> {
    let trimmed = code.trim();
    let normalized = trimmed
        .parse::<u32>()
        .map_or_else(|_| trimmed.to_string(), |n| format!("{:03}", n % 1000));
    if normalized == code_for(fingerprint) {
        return Ok(());
    }
    bail!(
        "Confirmation code {trimmed} belongs to a different order. Re-run this command \
         without --execute, check the preview, and use the code it prints."
    );
}

/// Re-render this invocation with `--execute <CODE>` appended, so the operator
/// has a line to copy rather than a code to splice in by hand.
///
/// Built from the real `argv`, not from the parsed values, so what is offered is
/// exactly what was previewed. `argv[0]` is replaced with the plain binary name:
/// the actual path may be a `target/debug/...` build nobody would retype.
fn command_with_code(code: &str) -> String {
    let mut out = String::from("longbridge");
    for arg in std::env::args().skip(1) {
        out.push(' ');
        out.push_str(&shell_quote(&arg));
    }
    out.push_str(" --execute ");
    out.push_str(code);
    out
}

/// Quote only when a shell would otherwise mangle the argument, so the common
/// case stays readable.
fn shell_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-=/:@,+".contains(c));
    if safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// The notice as a single sentence, for the `message` field of JSON output.
pub fn message(code: &str, action: &str) -> String {
    format!(
        "Nothing has been sent to the exchange. To {action}, re-run the identical command \
         with --execute {code}. The code only works for this exact order. AI agents: show \
         this preview to the user and only re-run once they have explicitly confirmed it."
    )
}

/// The call to action for human-readable output.
///
/// Deliberately not another `Field    value` row: the code is the one thing the
/// reader has to act on, and as a row it reads like more order detail. Leading
/// with the ready-to-run command makes the next step obvious without the reader
/// having to work out where the code goes.
pub fn print_notice(code: &str, action: &str) {
    println!();
    println!("Nothing has been sent to the exchange. To {action}, run:");
    println!();
    println!("    {}", command_with_code(code));
    println!();
    println!(
        "AI agents: show this preview to the user and only run the command above once they \
         have explicitly confirmed it."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_three_digits() {
        let code = code_for(&fingerprint(&["buy", "700.HK"]));
        assert_eq!(code.len(), 3);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
    }

    #[test]
    fn the_printed_code_verifies() {
        let fp = fingerprint(&["buy", "700.HK", "400"]);
        assert!(verify(&fp, &code_for(&fp)).is_ok());
    }

    #[test]
    fn harmless_rewordings_keep_the_same_code() {
        // Each pair is the same order said differently. Rejecting any of them
        // would be a confirmation failing on a difference the user cannot see.
        for (previewed, executed) in [
            (["buy", "700.HK", "400"], ["buy", "700.HK", "400.00"]),
            (["buy", "700.HK", "400"], ["buy", "700.hk", "400"]),
            (["buy", "700.HK", "400"], ["BUY", "700.HK", "400"]),
            (["buy", "700.HK", "400"], ["buy", " 700.HK ", " 400 "]),
            (["buy", "700.HK", "400"], ["buy", "700.HK", "+400"]),
        ] {
            let code = code_for(&fingerprint(&previewed));
            assert!(
                verify(&fingerprint(&executed), &code).is_ok(),
                "{previewed:?} and {executed:?} must share a code"
            );
        }
    }

    #[test]
    fn a_mangled_code_is_still_recognised() {
        let fp = fingerprint(&["buy", "700.HK"]);
        let code = code_for(&fp);
        assert!(verify(&fp, &format!("  {code} ")).is_ok());
        let unpadded = code.trim_start_matches('0');
        let unpadded = if unpadded.is_empty() { "0" } else { unpadded };
        assert!(verify(&fp, unpadded).is_ok(), "code {code} as {unpadded}");
    }

    #[test]
    fn a_real_change_to_the_order_invalidates_the_code() {
        // The case worth catching: the user approved 400 and 410 was sent.
        let code = code_for(&fingerprint(&["buy", "700.HK", "400"]));
        assert!(verify(&fingerprint(&["buy", "700.HK", "410"]), &code).is_err());
    }

    #[test]
    fn fingerprint_is_unambiguous_across_field_boundaries() {
        assert_ne!(fingerprint(&["AB", "C"]), fingerprint(&["A", "BC"]));
    }

    #[test]
    fn an_argument_needing_quotes_gets_them() {
        assert_eq!(shell_quote("400"), "400");
        assert_eq!(shell_quote("700.HK"), "700.HK");
        assert_eq!(shell_quote("my note"), "'my note'");
    }
}
