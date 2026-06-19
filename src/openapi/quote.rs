// OpenAPI SDK automatically manages device connections, no need to manually call online API
// Keep this file for compatibility with existing code references

use crate::openapi::context::QUOTE_CTX;
use anyhow::Result;

/// Fetch stock static information
pub async fn fetch_static_info(
    symbols: &[String],
) -> Result<Vec<longport::quote::SecurityStaticInfo>> {
    let ctx = QUOTE_CTX
        .get()
        .ok_or_else(|| anyhow::anyhow!("QuoteContext not initialized"))?;
    let info = ctx
        .static_info(symbols.iter().map(std::string::String::as_str))
        .await?;
    Ok(info)
}

/// Fetch recent trades for a symbol
pub async fn fetch_trades(symbol: &str, count: usize) -> Result<Vec<longport::quote::Trade>> {
    let ctx = QUOTE_CTX
        .get()
        .ok_or_else(|| anyhow::anyhow!("QuoteContext not initialized"))?;
    let trades = ctx.trades(symbol, count).await?;
    Ok(trades)
}
