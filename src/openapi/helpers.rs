use crate::openapi::{quote_limited, trade_limited};
/// Helper functions for rate-limited API calls
/// These functions wrap common API patterns with automatic rate limiting
use anyhow::Result;

/// Subscribe to quotes with automatic rate limiting
pub async fn subscribe_quotes<I, T>(symbols: I, sub_types: longport::quote::SubFlags) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let ctx = quote_limited();
    let symbols: Vec<String> = symbols.into_iter().map(Into::into).collect();
    let symbols_str = symbols.join(",");

    ctx.execute(&format!("subscribe({symbols_str})"), || {
        let inner = ctx.inner();
        let symbols = symbols.clone();
        Box::pin(async move {
            inner
                .subscribe(&symbols, sub_types)
                .await
                .map_err(anyhow::Error::from)
        })
    })
    .await
}

/// Unsubscribe from quotes with automatic rate limiting
pub async fn unsubscribe_quotes<I, T>(
    symbols: I,
    sub_types: longport::quote::SubFlags,
) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let ctx = quote_limited();
    let symbols: Vec<String> = symbols.into_iter().map(Into::into).collect();
    let symbols_str = symbols.join(",");

    ctx.execute(&format!("unsubscribe({symbols_str})"), || {
        let inner = ctx.inner();
        let symbols = symbols.clone();
        Box::pin(async move {
            inner
                .unsubscribe(&symbols, sub_types)
                .await
                .map_err(anyhow::Error::from)
        })
    })
    .await
}

/// Get quotes with automatic rate limiting
pub async fn get_quotes<I, T>(symbols: I) -> Result<Vec<longport::quote::SecurityQuote>>
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let ctx = quote_limited();
    let symbols: Vec<String> = symbols.into_iter().map(Into::into).collect();
    let symbols_str = symbols.join(",");

    ctx.execute(&format!("quote({symbols_str})"), || {
        let inner = ctx.inner();
        let symbols = symbols.clone();
        Box::pin(async move { inner.quote(&symbols).await.map_err(anyhow::Error::from) })
    })
    .await
}

/// Get static info with automatic rate limiting
pub async fn get_static_info<I, T>(symbols: I) -> Result<Vec<longport::quote::SecurityStaticInfo>>
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let ctx = quote_limited();
    let symbols: Vec<String> = symbols.into_iter().map(Into::into).collect();
    let symbols_str = symbols.join(",");

    ctx.execute(&format!("static_info({symbols_str})"), || {
        let inner = ctx.inner();
        let symbols = symbols.clone();
        Box::pin(async move {
            inner
                .static_info(&symbols)
                .await
                .map_err(anyhow::Error::from)
        })
    })
    .await
}

/// Get trades with automatic rate limiting
pub async fn get_trades(symbol: &str, count: usize) -> Result<Vec<longport::quote::Trade>> {
    let ctx = quote_limited();
    let symbol = symbol.to_string();

    ctx.execute(&format!("trades({symbol}, {count})"), || {
        let inner = ctx.inner();
        let symbol = symbol.clone();
        Box::pin(async move {
            inner
                .trades(&symbol, count)
                .await
                .map_err(anyhow::Error::from)
        })
    })
    .await
}

/// Get watchlist with automatic rate limiting
pub async fn get_watchlist() -> Result<Vec<longport::quote::WatchlistGroup>> {
    let ctx = quote_limited();

    ctx.execute("watchlist", || {
        let inner = ctx.inner();
        Box::pin(async move { inner.watchlist().await.map_err(anyhow::Error::from) })
    })
    .await
}

/// Get account balance with automatic rate limiting
pub async fn get_account_balance(
    currency: Option<&str>,
) -> Result<Vec<longport::trade::AccountBalance>> {
    let ctx = trade_limited();

    ctx.execute("account_balance", || {
        let inner = ctx.inner();
        let currency = currency.map(ToString::to_string);
        Box::pin(async move {
            inner
                .account_balance(currency.as_deref())
                .await
                .map_err(anyhow::Error::from)
        })
    })
    .await
}

/// Get stock positions with automatic rate limiting
pub async fn get_stock_positions() -> Result<longport::trade::StockPositionsResponse> {
    let ctx = trade_limited();

    ctx.execute("stock_positions", || {
        let inner = ctx.inner();
        Box::pin(async move {
            inner
                .stock_positions(None)
                .await
                .map_err(anyhow::Error::from)
        })
    })
    .await
}
