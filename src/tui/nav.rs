use bevy_ecs::prelude::*;

use crate::data::Counter;
use crate::tui::app::AppState;
use crate::tui::systems;
use crate::tui::widgets::Carousel;

/// Normalizes user input into a full `CODE.MARKET` symbol string.
/// - Input with a dot (e.g. `AAPL.US`, `700.hk`) → validates market, returns uppercased.
/// - All-letter input (e.g. `AAPL`, `tsla`) → appends `.US`.
/// - All-digit input (e.g. `700`, `09988`) → appends `.HK`.
/// - Anything else → `None` (invalid).
pub fn normalize_counter(query: &str) -> Option<String> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    if q.contains('.') {
        let mut parts = q.splitn(2, '.');
        let code = parts.next().unwrap_or("").trim();
        let market = parts.next().unwrap_or("").trim().to_uppercase();
        if code.is_empty()
            || !matches!(
                market.as_str(),
                "HK" | "US" | "SH" | "SZ" | "SG" | "HAS"
            )
        {
            return None;
        }
        Some(format!("{}.{}", code.to_uppercase(), market))
    } else if q.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(format!("{}.US", q.to_uppercase()))
    } else if q.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{q}.HK"))
    } else {
        None
    }
}

pub fn navigate_to_counter(app: &mut bevy_app::App, counter: Counter) {
    app.world.insert_resource(systems::StockDetail(counter));
    let state = *app.world.resource::<State<AppState>>().get();
    let next_state = if state == AppState::Stock {
        AppState::Stock
    } else {
        AppState::WatchlistStock
    };
    app.world.insert_resource(NextState(Some(next_state)));
}

pub fn get_active_symbol(app: &bevy_app::App, state: AppState) -> Option<String> {
    match state {
        AppState::Stock | AppState::WatchlistStock => app
            .world
            .get_resource::<systems::StockDetail>()
            .map(|sd| sd.0.to_string()),
        AppState::Portfolio => {
            let idx = systems::PORTFOLIO_TABLE
                .lock()
                .expect("poison")
                .selected()?;
            let view = systems::PORTFOLIO_VIEW.read().expect("poison");
            view.as_ref()?.holdings.get(idx).map(|h| h.symbol.clone())
        }
        AppState::Watchlist => {
            let idx = systems::WATCHLIST_TABLE
                .lock()
                .expect("poison")
                .selected()?;
            let watchlist = crate::tui::app::WATCHLIST.read().expect("poison");
            let counters = watchlist.counters();
            counters.get(idx).map(std::string::ToString::to_string)
        }
        _ => None,
    }
}

pub fn show_index(world: &mut World, index: usize) {
    let indexes = world.resource::<Carousel<[Counter; 3]>>().current();
    world.insert_resource(systems::StockDetail(indexes[index].clone()));
    world.insert_resource(NextState(Some(AppState::WatchlistStock)));
}
