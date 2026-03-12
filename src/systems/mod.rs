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
    app::AppState,
    data::{Account, Counter, KlineType, ReadyState, SubTypes, WatchlistGroup},
    openapi,
    widgets::{Carousel, LocalSearch, Search},
};

mod common;
mod portfolio;
mod stock_detail;
mod watchlist;
mod watchlist_stock;

// Re-export render functions
pub use common::*;
pub use portfolio::*;
pub use stock_detail::*;
pub use watchlist::*;
pub use watchlist_stock::*;

// Compatibility type alias
pub type Component = ();

// WebSocket subscription management (simplified implementation)
pub struct WsManager;

impl WsManager {
    #[allow(clippy::unused_async)]
    pub async fn unmount(&self, _name: &str) -> anyhow::Result<()> {
        // TODO: Use Longbridge SDK to unsubscribe
        Ok(())
    }

    pub async fn remount(
        &self,
        _name: &str,
        symbols: &[Counter],
        _sub_type: SubTypes,
    ) -> anyhow::Result<()> {
        // TODO: Use Longbridge SDK to resubscribe
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

    pub async fn quote_detail(&self, _name: &str, symbols: &[Counter]) -> anyhow::Result<()> {
        let ctx = crate::openapi::quote();
        let symbol_strings: Vec<String> = symbols
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let _ = ctx
            .subscribe(
                &symbol_strings,
                longbridge::quote::SubFlags::QUOTE | longbridge::quote::SubFlags::DEPTH,
            )
            .await;
        Ok(())
    }

    pub async fn quote_trade(&self, _name: &str, symbols: &[Counter]) -> anyhow::Result<()> {
        let ctx = crate::openapi::quote();
        let symbol_strings: Vec<String> = symbols
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let _ = ctx
            .subscribe(&symbol_strings, longbridge::quote::SubFlags::TRADE)
            .await;
        Ok(())
    }
}

pub static WS: std::sync::LazyLock<WsManager> = std::sync::LazyLock::new(|| WsManager);

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
}

// Shared resource types
#[derive(Clone, Resource)]
pub struct Command(pub mpsc::UnboundedSender<bevy_ecs::system::CommandQueue>);

#[derive(Resource)]
pub struct WsState(pub ReadyState);

#[derive(Resource)]
pub struct StockDetail(pub Counter);
