use std::{
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
    sync::RwLock,
    time::{Duration, Instant},
};

use bevy_ecs::prelude::*;

#[derive(Debug, Resource, Component)]
pub struct Carousel<T> {
    inner: Vec<T>,
    duration: Duration,
    index: AtomicUsize,
    last_time: RwLock<Instant>,
}

impl<T> Deref for Carousel<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Carousel<T> {
    pub fn new(inner: Vec<T>, duration: Duration) -> Self {
        assert!(!inner.is_empty(), "initial value must not be empty");
        Self {
            inner,
            duration,
            index: AtomicUsize::new(0),
            last_time: RwLock::new(Instant::now()),
        }
    }

    pub fn tick(&self) -> &T {
        // todo: use global time instead of local time?
        let now = std::time::Instant::now();
        if now.duration_since(*self.last_time.read().expect("poison")) >= self.duration {
            let mut last_time = self.last_time.write().expect("poison");
            // a double check
            if now.duration_since(*last_time) >= self.duration {
                *last_time = now;
                self.index.fetch_add(1, Ordering::Acquire);
            }
        }
        let idx = self.index.load(Ordering::Relaxed);
        &self.inner[idx % self.inner.len()]
    }

    pub fn current(&self) -> &T {
        let idx = self.index.load(Ordering::Relaxed) % self.inner.len();
        &self.inner[idx]
    }
}

// ============

#[derive(Debug, Resource, Component)]
pub struct Select<T> {
    inner: Vec<T>,
    selected: AtomicUsize,
}

impl<T> Deref for Select<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Select<T> {
    pub fn new(inner: Vec<T>) -> Self {
        assert!(!inner.is_empty(), "initial value must not be empty");
        Self {
            inner,
            selected: AtomicUsize::new(0),
        }
    }

    pub fn select(&self, idx: usize) -> usize {
        let idx = idx % self.inner.len();
        self.selected.swap(idx, Ordering::Relaxed)
    }

    #[inline]
    pub fn index(&self) -> usize {
        self.selected.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> &T {
        &self.inner[self.index()]
    }
}
