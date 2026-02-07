// OpenAPI SDK automatically manages device connections, no need to manually call online API
// Keep this file for compatibility with existing code references

use anyhow::Result;
use crate::openapi::context::QUOTE_CTX;

/// Fetch stock static information
pub async fn fetch_static_info(symbols: &[String]) -> Result<Vec<longport::quote::SecurityStaticInfo>> {
    let ctx = QUOTE_CTX.get().ok_or_else(|| anyhow::anyhow!("QuoteContext not initialized"))?;
    let info = ctx.static_info(symbols.iter().map(|s| s.as_str())).await?;
    Ok(info)
}
