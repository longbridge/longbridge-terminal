use dashmap::DashMap;
use std::sync::Arc;

use super::{Counter, Stock};

/// Global stock cache
pub static STOCKS: std::sync::LazyLock<StockStore> = std::sync::LazyLock::new(StockStore::new);

/// Stock storage (simplified)
pub struct StockStore {
    inner: DashMap<Counter, Arc<Stock>>,
}

impl StockStore {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    /// Get single stock
    pub fn get(&self, counter: &Counter) -> Option<Arc<Stock>> {
        self.inner.get(counter).map(|r| Arc::clone(r.value()))
    }

    /// Batch get stocks
    pub fn mget(&self, counters: &[Counter]) -> Vec<Option<Arc<Stock>>> {
        counters.iter().map(|c| self.get(c)).collect()
    }

    /// Insert or update stock
    pub fn insert(&self, stock: Stock) {
        let counter = stock.counter.clone();
        self.inner.insert(counter, Arc::new(stock));
    }

    /// Modify stock data (atomic operation)
    pub fn modify<F>(&self, counter: Counter, f: F)
    where
        F: FnOnce(&mut Stock),
    {
        let mut stock = self
            .get(&counter)
            .map_or_else(|| Stock::new(counter.clone()), |s| (*s).clone());
        f(&mut stock);
        self.insert(stock);
    }

    /// Remove stock
    pub fn remove(&self, counter: &Counter) {
        self.inner.remove(counter);
    }

    /// Clear all stocks
    pub fn clear(&self) {
        self.inner.clear();
    }

    /// Get stock count
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for StockStore {
    fn default() -> Self {
        Self::new()
    }
}
