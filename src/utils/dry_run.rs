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

/// Canonical form of one field.
///
/// `Decimal` first: it collapses `400`, `400.0`, `400.00`, `+400`, `0400` and
/// `4e2`, which is where two runs differ most often. Anything that is not a
/// number is trimmed and upper-cased — symbols, sides and order types are all
/// matched case-insensitively downstream, so treating them as distinct here
/// would reject an order the exchange considers identical.
fn canonical(field: &str) -> String {
    let trimmed = field.trim();
    // `from_str` rejects exponent form, which JSON encoders do emit for very
    // small or very large values (a crypto price as `1e-8`), so try
    // `from_scientific` before concluding it is not a number.
    Decimal::from_str(trimmed)
        .or_else(|_| Decimal::from_scientific(trimmed))
        .map_or_else(|_| trimmed.to_uppercase(), |n| n.normalize().to_string())
}

/// The canonical description of what a confirmation code covers.
///
/// Every gated command funnels through one of the constructors below, so a
/// call site cannot get the field order wrong, and so a mismatch can be
/// explained by showing two strings rather than two opaque digests.
///
/// The fields are deliberately few — the symbol, the side, the size and the
/// price are what a user reads off a preview and what a wrong order would get
/// wrong. Folding in the optional extras would buy very little and risk the
/// failure this gate can least afford: a caller that drops `--remark` on the
/// second run being told its code is wrong, for a difference nobody can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope(String);

impl Scope {
    /// `buy 100 700.HK @ 400` — a new order.
    ///
    /// `side` comes from the subcommand and so cannot drift between two runs of
    /// the same command, while leaving it out would let a code previewed for a
    /// buy execute a sell.
    pub fn order(side: &str, symbol: &str, quantity: &str, price: &str) -> Self {
        let price = canonical(price);
        Self(format!(
            "{} {} {} @ {}",
            canonical(side).to_lowercase(),
            canonical(quantity),
            canonical(symbol),
            if price.is_empty() { "market" } else { &price },
        ))
    }

    /// `cancel order 20240101-1` — an action on an order that already exists.
    pub fn on_order(action: &str, order_id: &str) -> Self {
        Self(format!("{} order {}", action, canonical(order_id)))
    }

    /// `replace order 20240101-1 to 200 @ 255` — a change to an existing order.
    pub fn replace(order_id: &str, quantity: &str, price: &str) -> Self {
        let price = canonical(price);
        Self(format!(
            "replace order {} to {} @ {}",
            canonical(order_id),
            canonical(quantity),
            if price.is_empty() {
                "unchanged"
            } else {
                &price
            },
        ))
    }

    /// `grid submit 100 700.HK @ 449` — a grid strategy.
    pub fn grid(action: &str, symbol: &str, quantity: &str, base_price: &str) -> Self {
        Self(format!(
            "grid {} {} {} @ {}",
            action,
            canonical(quantity),
            canonical(symbol),
            canonical(base_price),
        ))
    }

    /// The three-digit code this scope is confirmed by.
    pub fn code(&self) -> String {
        let mut hasher = Sha256::new();
        // Domain separator: this digest must never coincide with any other use
        // of the same text elsewhere in the CLI.
        hasher.update(b"longbridge/execute-confirmation/v1");
        hasher.update(self.0.as_bytes());
        let digest = hasher.finalize();
        let n = (u32::from(digest[0]) << 8) | u32::from(digest[1]);
        format!("{:03}", n % 1000)
    }

    /// Check the code the caller quoted back.
    ///
    /// The only way to fail is to quote a code for a *different* order. Leading
    /// zeros and stray whitespace are forgiven — a code retyped as `7` is still
    /// the code this order was given. The canonical text goes into the error so
    /// a mismatch can be read rather than guessed at.
    pub fn verify(&self, code: &str) -> Result<()> {
        let trimmed = code.trim();
        let normalized = trimmed
            .parse::<u32>()
            .map_or_else(|_| trimmed.to_string(), |n| format!("{:03}", n % 1000));
        if normalized == self.code() {
            return Ok(());
        }
        bail!(
            "Confirmation code {trimmed} does not match this request ({}). Re-run this \
             command without --execute, check the preview, and use the code it prints.",
            self.0
        );
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
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

    /// One order, written every way a caller might reasonably write it.
    ///
    /// A false rejection here is the failure this gate can least afford: the
    /// user sees the order they approved, the CLI insists the code is wrong,
    /// and the next thing they learn is to distrust the confirmation. So the
    /// bar is that every spelling below produces one scope, and therefore one
    /// code.
    fn same_order() -> Vec<Scope> {
        [
            ("Buy", "700.HK", "100", "400"),
            // Trailing zeros, which a preview, a spreadsheet or a human adds.
            ("Buy", "700.HK", "100", "400.0"),
            ("Buy", "700.HK", "100", "400.00"),
            ("Buy", "700.HK", "100", "400.0000000000"),
            ("Buy", "700.HK", "100.00", "400"),
            // Signs and leading zeros that survive a round trip through a number.
            ("Buy", "700.HK", "100", "+400"),
            ("Buy", "700.HK", "0100", "0400"),
            ("Buy", "700.HK", "100", "0400.000"),
            // Exponent form, which JSON encoders emit for some values.
            ("Buy", "700.HK", "100", "4e2"),
            ("Buy", "700.HK", "1e2", "400"),
            ("Buy", "700.HK", "100", "4E2"),
            // Case, on the symbol and on the side.
            ("buy", "700.HK", "100", "400"),
            ("BUY", "700.HK", "100", "400"),
            ("Buy", "700.hk", "100", "400"),
            ("Buy", "700.Hk", "100", "400"),
            // Whitespace from a copy-paste or a wrapped line.
            (" Buy ", " 700.HK ", " 100 ", " 400 "),
            ("Buy", "700.HK", "100", "\t400\n"),
        ]
        .into_iter()
        .map(|(side, symbol, qty, price)| Scope::order(side, symbol, qty, price))
        .collect()
    }

    /// Prices whose decimal places must survive, because dropping or adding one
    /// is a hundredfold error. Each pair is the same number written twice.
    fn same_fractional_price() -> Vec<(Scope, Scope)> {
        [
            ("0.5", "0.50"),
            ("0.5", ".5"),
            ("500.2", "500.200"),
            ("500.2", "500.2000000000"),
            ("0.00000001", "1e-8"),
            ("1234.5678", "1234.56780"),
        ]
        .into_iter()
        .map(|(a, b)| {
            (
                Scope::order("buy", "700.HK", "100", a),
                Scope::order("buy", "700.HK", "100", b),
            )
        })
        .collect()
    }

    /// Orders that really are different, and so must not share a code.
    fn different_orders() -> Vec<Scope> {
        vec![
            Scope::order("Buy", "700.HK", "100", "410"),   // price
            Scope::order("Buy", "700.HK", "100", "400.1"), // price, one decimal place out
            Scope::order("Buy", "700.HK", "100", "40"),    // price, decimal point moved
            Scope::order("Buy", "700.HK", "100", "4000"),
            Scope::order("Buy", "700.HK", "200", "400"), // quantity
            Scope::order("Buy", "9988.HK", "100", "400"), // symbol
            Scope::order("Sell", "700.HK", "100", "400"), // side
            Scope::order("Buy", "700.HK", "100", ""),    // limit dropped for a market order
            Scope::on_order("cancel", "700"),            // a different kind of action entirely
            Scope::replace("700", "100", "400"),
            Scope::grid("submit", "700.HK", "100", "400"),
        ]
    }

    #[test]
    fn a_code_is_always_three_digits() {
        for scope in same_order().into_iter().chain(different_orders()) {
            let code = scope.code();
            assert_eq!(code.len(), 3, "{scope} gave {code}");
            assert!(
                code.chars().all(|c| c.is_ascii_digit()),
                "{scope} gave {code}"
            );
        }
    }

    #[test]
    fn every_spelling_of_one_order_normalises_to_one_scope() {
        let all = same_order();
        let baseline = &all[0];
        assert_eq!(baseline.to_string(), "buy 100 700.HK @ 400");
        for scope in &all {
            assert_eq!(
                scope, baseline,
                "{scope} must normalise to the same scope as {baseline}"
            );
            assert!(
                scope.verify(&baseline.code()).is_ok(),
                "{scope} must verify"
            );
        }
    }

    #[test]
    fn fractional_prices_survive_normalisation() {
        // Trailing-zero stripping must not become decimal-point moving.
        for (a, b) in same_fractional_price() {
            assert_eq!(a, b, "{a} and {b} are the same price");
            assert!(b.verify(&a.code()).is_ok());
        }
    }

    #[test]
    fn the_code_a_preview_prints_always_verifies() {
        for scope in same_order().into_iter().chain(different_orders()) {
            assert!(
                scope.verify(&scope.code()).is_ok(),
                "{scope} must accept its own code"
            );
        }
    }

    #[test]
    fn a_different_order_does_not_inherit_the_code() {
        let approved = same_order()[0].code();
        for scope in different_orders() {
            assert!(
                scope.verify(&approved).is_err(),
                "{scope} must not accept the baseline code"
            );
        }
    }

    #[test]
    fn a_mangled_code_is_still_recognised() {
        // How a code comes back after a human retypes it, or a client sends it
        // through a JSON number and loses the leading zeros.
        let scope = same_order().remove(0);
        let code = scope.code();
        let unpadded = code.trim_start_matches('0');
        let unpadded = if unpadded.is_empty() { "0" } else { unpadded };
        for quoted in [
            code.clone(),
            format!(" {code}"),
            format!("{code} "),
            format!("  {code}  "),
            format!("\t{code}\n"),
            unpadded.to_string(),
            format!(" {unpadded} "),
        ] {
            assert!(
                scope.verify(&quoted).is_ok(),
                "code {code} quoted as {quoted:?}"
            );
        }
    }

    #[test]
    fn nonsense_in_the_code_slot_is_rejected_without_panicking() {
        // The one thing that must never happen is a crash on the path that
        // decides whether real money moves.
        let scope = same_order().remove(0);
        for quoted in [
            "",
            " ",
            "abc",
            "-1",
            "1000",
            "99999999999999999999",
            "4.7",
            "٤٧٣",
        ] {
            let _ = scope.verify(quoted);
        }
    }

    #[test]
    fn a_mismatch_explains_itself() {
        // A code that fails must say what this request actually is, or the user
        // has no way to see which of the two orders is the odd one.
        let err = Scope::order("buy", "700.HK", "100", "410")
            .verify(&Scope::order("buy", "700.HK", "100", "400").code())
            .expect_err("must reject");
        assert!(err.to_string().contains("buy 100 700.HK @ 410"), "{err}");
    }

    #[test]
    fn every_distinct_order_gets_a_distinct_scope() {
        // Codes are three digits and will collide by design; the scopes behind
        // them must not, or a collision would be systematic.
        let mut seen = std::collections::HashSet::new();
        for scope in different_orders() {
            assert!(seen.insert(scope.to_string()), "{scope} collided");
        }
        assert!(seen.insert(same_order()[0].to_string()));
    }

    #[test]
    fn a_missing_price_reads_as_market_not_as_nothing() {
        assert_eq!(
            Scope::order("buy", "700.HK", "100", "").to_string(),
            "buy 100 700.HK @ market"
        );
    }

    #[test]
    fn free_text_is_content_not_a_value_to_normalise_away() {
        assert_ne!(canonical("buy the dip"), canonical("sell the rip"));
    }

    #[test]
    fn an_argument_needing_quotes_gets_them() {
        assert_eq!(shell_quote("400"), "400");
        assert_eq!(shell_quote("700.HK"), "700.HK");
        assert_eq!(shell_quote("--price"), "--price");
        assert_eq!(shell_quote("my note"), "'my note'");
        assert_eq!(shell_quote(""), "''");
    }
}
