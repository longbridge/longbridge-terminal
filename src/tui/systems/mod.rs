use std::{collections::HashMap, sync::Mutex};

use atomic::Atomic;
use bevy_ecs::{
    event::Event,
    schedule::State,
    system::{Res, ResMut, Resource},
};
use ratatui::widgets::TableState;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::{
    data::{Account, Counter, KlineType, ReadyState, SubTypes, WatchlistGroup},
    openapi,
    tui::app::AppState,
    tui::widgets::{Carousel, LocalSearch, Search},
};

mod common;
mod orders;
mod portfolio;
mod stock_detail;
mod stock_news;
mod watchlist;
mod watchlist_stock;

// Re-export render functions
pub use common::*;
pub use orders::*;
pub use portfolio::*;
pub use stock_detail::*;
pub use stock_news::*;
pub use watchlist::*;
pub use watchlist_stock::*;

// Compatibility type alias
pub type Component = ();

/// Which symbols each named mount asked the server to stream.
///
/// The split between the two feeds is deliberate. `QUOTE` belongs to the
/// watchlist and to the index carousel: it is subscribed once and never given
/// back, which is what keeps a price on screen without a request per screen.
/// `DEPTH` and `TRADE` belong to whichever single stock the detail view is
/// showing, and they are the expensive half — an order book pushes many times
/// a second, and every push deep-copies the stock it lands on. Leaving them
/// subscribed for every stock ever opened is what made a session get slower
/// the longer it was used.
pub struct WsManager {
    detail: std::sync::Mutex<HashMap<String, Vec<Counter>>>,
}

/// The feeds only the stock the reader is looking at needs.
const DETAIL_FLAGS: longbridge::quote::SubFlags =
    longbridge::quote::SubFlags::DEPTH.union(longbridge::quote::SubFlags::TRADE);

impl WsManager {
    fn new() -> Self {
        Self {
            detail: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Release the per-stock feeds this mount holds. `QUOTE` is never released:
    /// the watchlist behind the detail view is still showing those prices.
    pub async fn unmount(&self, name: &str) -> anyhow::Result<()> {
        let symbols = self
            .detail
            .lock()
            .expect("poison")
            .remove(name)
            .unwrap_or_default();
        if symbols.is_empty() {
            return Ok(());
        }
        let symbols: Vec<String> = symbols.iter().map(ToString::to_string).collect();
        let _ = crate::openapi::quote()
            .unsubscribe(&symbols, DETAIL_FLAGS)
            .await;
        Ok(())
    }

    pub async fn remount(
        &self,
        _name: &str,
        symbols: &[Counter],
        _sub_type: SubTypes,
    ) -> anyhow::Result<()> {
        let ctx = crate::openapi::quote();
        let symbol_strings: Vec<String> = symbols
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let _ = ctx
            .subscribe(&symbol_strings, longbridge::quote::SubFlags::QUOTE)
            .await;
        Ok(())
    }

    /// Stream everything the detail view draws for `symbols`.
    ///
    /// One `subscribe` for all three feeds rather than one per feed: the quote
    /// SDK runs a single command at a time and awaits its whole round trip, so
    /// each extra call is another full network wait in front of the data the
    /// reader is waiting for. Whatever this mount was streaming before is left
    /// running and handed to [`release_stale`], which the caller sends once the
    /// panel is filled — a release is never what the reader is waiting on.
    pub async fn quote_detail(&self, name: &str, symbols: &[Counter]) -> anyhow::Result<()> {
        let symbol_strings: Vec<String> = symbols
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        {
            let mut detail = self.detail.lock().expect("poison");
            let mount = detail.entry(name.to_string()).or_default();
            for counter in symbols {
                if !mount.contains(counter) {
                    mount.push(counter.clone());
                }
            }
        }

        let _ = crate::openapi::quote()
            .subscribe(
                &symbol_strings,
                longbridge::quote::SubFlags::QUOTE | DETAIL_FLAGS,
            )
            .await;
        Ok(())
    }

    /// Stop streaming the order book and ticks of everything this mount holds
    /// except `keep`.
    ///
    /// Called off the critical path, after the panel the reader is waiting for
    /// is filled. Until it runs, a walk down a watchlist leaves a trail of open
    /// order books, and every one of them pushes several times a second into a
    /// panel nobody is looking at.
    pub async fn release_stale(&self, name: &str, keep: &[Counter]) -> anyhow::Result<()> {
        let stale: Vec<String> = {
            let detail = self.detail.lock().expect("poison");
            let Some(mount) = detail.get(name) else {
                return Ok(());
            };
            mount
                .iter()
                .filter(|counter| !keep.contains(counter))
                .map(ToString::to_string)
                .collect()
        };
        if stale.is_empty() {
            return Ok(());
        }
        crate::openapi::quote()
            .unsubscribe(&stale, DETAIL_FLAGS)
            .await?;
        // Forgotten only once the server has been told, so a task cancelled
        // part-way through leaves them on the books for the next release to
        // pick up rather than stranding them subscribed for the session.
        if let Some(mount) = self.detail.lock().expect("poison").get_mut(name) {
            mount.retain(|counter| keep.contains(counter));
        }
        Ok(())
    }
}

pub static WS: std::sync::LazyLock<WsManager> = std::sync::LazyLock::new(WsManager::new);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskLevel {
    Safe,
    Low,
    MiddleLow,
    Middle,
    MiddleHigh,
    Medium,
    High,
    Danger,
    Warning,
}

#[derive(Event)]
pub struct TuiEvent(pub tui_input::InputRequest);

// Shared statics
pub(crate) static KLINE_TYPE: Atomic<KlineType> = Atomic::new(KlineType::PerDay);
pub(crate) static KLINE_INDEX: Atomic<usize> = Atomic::new(0);

pub(crate) static LAST_DONE: std::sync::LazyLock<Mutex<HashMap<Counter, Decimal>>> =
    std::sync::LazyLock::new(Mutex::default);
pub(crate) static WATCHLIST_TABLE: std::sync::LazyLock<Mutex<TableState>> =
    std::sync::LazyLock::new(Mutex::default);

// Shared type aliases
pub(crate) type NavFooter<'w> = (
    Res<'w, State<AppState>>,
    Res<'w, Carousel<[Counter; 3]>>,
    Res<'w, WsState>,
);

pub(crate) type PopUp<'w> = (
    ResMut<'w, LocalSearch<Account>>,
    ResMut<'w, LocalSearch<openapi::account::CurrencyInfo>>,
    ResMut<'w, Search<openapi::search::StockItem>>,
    ResMut<'w, LocalSearch<WatchlistGroup>>,
    ResMut<'w, LocalSearch<Counter>>,
);

// Shared event types
#[derive(Event)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Enter,
    NewsToggle,
    NewsScrollUp,
    NewsScrollDown,
    NewsOpen,
}

// Shared resource types
#[derive(Clone, Resource)]
pub struct Command(pub mpsc::UnboundedSender<bevy_ecs::system::CommandQueue>);

#[derive(Resource)]
pub struct WsState(pub ReadyState);

#[derive(Resource)]
pub struct StockDetail(pub Counter);
