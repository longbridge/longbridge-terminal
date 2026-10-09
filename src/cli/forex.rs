//! Forex (currency exchange) channel CLI.
//!
//! Binds `longbridge::forex::ForexContext`. Mirrors the fund channel's shape:
//! one `ForexCommands` enum, an async `run()` dispatcher, and a
//! `schema_for_path` provider. Output flows through the shared `OutputFormat`.
//!
//! The conversion flow is three steps and asynchronous:
//! 1. `quote` — lock a rate and get a `quote_id`.
//! 2. `submit-order` — place the conversion (asks to confirm unless `--yes`);
//!    acceptance only, settlement is async.
//! 3. `order` — poll by `client_order_id` until a terminal state.

use std::io::Write as _;

use anyhow::Result;
use clap::Subcommand;
use longbridge::forex::{GetForexQuoteOptions, SubmitForexOrderOptions};
use rust_decimal::Decimal;
use serde::Serialize;

use super::output::{print_table, strip_private_fields};
use super::OutputFormat;

#[derive(Subcommand)]
pub enum ForexCommands {
    /// Lock a currency-exchange rate and get a quote id
    ///
    /// Give either `--amount` (convert-out side) or `--target-amount`
    /// (convert-in side).
    ///
    /// Example: longbridge forex quote USD HKD --amount 1000
    Quote {
        /// Convert-out currency (ISO 4217, e.g. USD)
        from: String,
        /// Convert-in currency (e.g. HKD)
        to: String,
        /// Convert-out amount (mutually exclusive with --target-amount)
        #[arg(long)]
        amount: Option<Decimal>,
        /// Convert-in amount (mutually exclusive with --amount)
        #[arg(long = "target-amount")]
        target_amount: Option<Decimal>,
    },

    /// Submit a forex order against a quote — asks to confirm unless --yes
    ///
    /// Acceptance only; conversion is asynchronous. The resulting order state is
    /// re-queried and printed. Poll `forex order <client_order_id>` for the
    /// final state.
    ///
    /// Example: longbridge forex submit-order <`quote_id`> <`client_order_id`> --yes
    #[command(name = "submit-order")]
    SubmitOrder {
        /// Quote id returned by `forex quote`
        quote_id: String,
        /// Your own order id (unique across your accounts; the idempotency key)
        client_order_id: String,
        /// Skip the interactive confirmation
        #[arg(long)]
        yes: bool,
    },

    /// A single forex order detail (by client order id)
    ///
    /// Example: longbridge forex order <`client_order_id`>
    Order {
        /// Your order id
        client_order_id: String,
    },
}

fn output<T: Serialize>(value: &T, format: &OutputFormat) -> Result<()> {
    let mut json = serde_json::to_value(value)?;
    strip_private_fields(&mut json);
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&json)?),
        OutputFormat::Pretty => match &json {
            serde_json::Value::Object(map) => {
                let rows: Vec<Vec<String>> = map
                    .iter()
                    .map(|(k, v)| vec![k.clone(), value_cell(v)])
                    .collect();
                print_table(&["Field", "Value"], rows, format);
            }
            other => println!("{other}"),
        },
    }
    Ok(())
}

/// Render a scalar JSON value without the surrounding quotes a raw `to_string`
/// would add.
fn value_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(false);
    }
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

pub async fn run(cmd: ForexCommands, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::forex();
    match cmd {
        ForexCommands::Quote {
            from,
            to,
            amount,
            target_amount,
        } => {
            let opts = if amount.is_some() || target_amount.is_some() {
                let mut o = GetForexQuoteOptions::new();
                if let Some(a) = amount {
                    o = o.amount(a);
                }
                if let Some(t) = target_amount {
                    o = o.target_amount(t);
                }
                Some(o)
            } else {
                None
            };
            output(&ctx.quote(from, to, opts).await?, format)
        }
        ForexCommands::SubmitOrder {
            quote_id,
            client_order_id,
            yes,
        } => {
            if !yes
                && !confirm(&format!(
                    "Submit forex order for quote {quote_id} (client id {client_order_id})?"
                ))?
            {
                println!("Aborted.");
                return Ok(());
            }
            let opts = SubmitForexOrderOptions::new(quote_id, client_order_id.clone());
            ctx.submit_order(opts).await?;
            // submit_order returns (); echo the resulting order state, mirroring
            // the fund channel's cancel-order re-query.
            if matches!(format, OutputFormat::Pretty) {
                println!("Forex order submitted (client id {client_order_id}). Current state:");
            }
            output(&ctx.order(client_order_id).await?, format)
        }
        ForexCommands::Order { client_order_id } => {
            output(&ctx.order(client_order_id).await?, format)
        }
    }
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::object;

    let sub = path.get(1)?.as_str();
    let schema = match sub {
        "quote" => object(
            "Forex quote (a locked rate for a currency pair)",
            &["quote_id", "rate", "expire_at", "ccy_pair"],
        ),
        "submit-order" => object(
            "Forex order detail (re-queried after submission)",
            &["state", "rate", "from_amount", "to_amount", "fail_reason"],
        ),
        "order" => object(
            "A single forex order detail",
            &["state", "rate", "from_amount", "to_amount", "fail_reason"],
        ),
        _ => return None,
    };
    Some(schema)
}
