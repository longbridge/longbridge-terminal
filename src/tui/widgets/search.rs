use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use bevy_ecs::{prelude::*, system::CommandQueue};
use crossterm::event::KeyEvent;
use futures::future::BoxFuture;
use ratatui::widgets::TableState;
use tokio::sync::{mpsc, watch};
use tui_input::backend::crossterm::EventHandler;

use crate::utils::cycle;

#[derive(Resource, Component)]
pub struct LocalSearch<T> {
    pub(crate) input: tui_input::Input,
    pub(crate) table: TableState,
    visible: bool,
    items: Vec<T>,
    options: Vec<T>,
    func: fn(&str, &T) -> bool,
    error_msg: Option<String>,
}

impl<T> std::fmt::Debug for LocalSearch<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSearch")
            .field("visible", &self.visible)
            .field("input", &self.input.value())
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

impl<T> LocalSearch<T>
where
    T: Clone + Send + 'static,
{
    pub fn new(items: Vec<T>, func: fn(&str, &T) -> bool) -> Self {
        Self {
            input: tui_input::Input::default(),
            table: TableState::default(),
            visible: false,
            options: items.clone(),
            items,
            func,
            error_msg: None,
        }
    }

    pub fn visible(&mut self) {
        self.visible = true;
    }

    pub fn query(&self) -> &str {
        self.input.value()
    }

    pub fn handle_key(&mut self, event: KeyEvent) -> (bool, Option<T>) {
        match event {
            key!(Esc) => {
                self.visible = false;
                self.error_msg = None;
                return (true, None);
            }
            key!(Enter) => {
                if let Some(idx) = self.table.selected() {
                    self.table.select(None);
                    let selected = self.option(idx);
                    self.input.reset();
                    self.options = self.items.clone();
                    self.visible = false;
                    self.error_msg = None;
                    return (true, Some(selected));
                }
            }
            key!(Up) => {
                let idx = cycle::prev_opt(self.table.selected(), self.options.len());
                self.table.select(idx);
            }
            key!(Down) => {
                let idx = cycle::next_opt(self.table.selected(), self.options.len());
                self.table.select(idx);
            }
            _ => {
                self.error_msg = None;
                let evt = crossterm::event::Event::Key(event);
                if self.input.handle_event(&evt).is_some() {
                    let keyword = self.input.value();
                    self.options = self
                        .items
                        .iter()
                        .filter(|v| keyword.is_empty() || (self.func)(keyword, v))
                        .cloned()
                        .collect();
                    // Auto-select first match so Enter works immediately
                    self.table.select(if self.options.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                }
            }
        }
        (false, None)
    }

    pub fn set_error(&mut self, msg: String) {
        self.error_msg = Some(msg);
    }

    pub fn error(&self) -> Option<&str> {
        self.error_msg.as_deref()
    }

    /// When Enter is pressed with a non-empty input and no dropdown item selected,
    /// returns the raw typed query for direct navigation. Leaves the popup open so
    /// the caller can decide whether to close it or show an error.
    pub fn consume_direct_enter(&mut self, event: KeyEvent) -> Option<String> {
        if matches!(event, key!(Enter))
            && self.table.selected().is_none()
            && !self.input.value().is_empty()
        {
            Some(self.input.value().to_string())
        } else {
            None
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.error_msg = None;
        self.input.reset();
        self.options = self.items.clone();
        self.table.select(None);
    }

    pub fn reset_items(&mut self, items: Vec<T>) {
        self.items = items.clone();
        self.options = items;
        self.input.reset();
        self.table.select(None);
        self.error_msg = None;
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn options(&self) -> &[T] {
        &self.options
    }

    fn option(&self, index: usize) -> T {
        self.options.get(index).cloned().expect("index in range")
    }
}

// ------------

#[derive(Resource, Component)]
pub struct Search<T> {
    pub(crate) input: tui_input::Input,
    pub(crate) table: TableState,
    visible: bool,
    options: Arc<Mutex<Vec<T>>>,
    history: Vec<T>,
    tx: watch::Sender<String>,
}

impl<T> std::fmt::Debug for Search<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Search")
            .field("visible", &self.visible)
            .field("input", &self.input.value())
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl<T> Search<T>
where
    T: Clone + PartialEq + Send + 'static,
{
    pub fn new(
        update: mpsc::UnboundedSender<CommandQueue>,
        task: impl Fn(String) -> BoxFuture<'static, Vec<T>> + Send + Sync + 'static,
    ) -> Self {
        let (tx, mut rx) = watch::channel(String::new());
        let options = Arc::new(Mutex::new(vec![]));
        tokio::spawn({
            let options = options.clone();
            async move {
                loop {
                    _ = rx.changed().await;
                    // debounce input
                    loop {
                        match tokio::time::timeout(Duration::from_millis(10), rx.changed()).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(_)) => return,
                            Err(_) => break,
                        }
                    }
                    let input = rx.borrow_and_update().to_string();
                    if input.is_empty() {
                        options.lock().unwrap().clear();
                    } else {
                        let result = (task)(input).await;
                        *options.lock().unwrap() = result;
                    }
                    let _ = update.send(CommandQueue::default());
                }
            }
        });
        Self {
            input: tui_input::Input::default(),
            table: TableState::default(),
            visible: false,
            options,
            history: vec![],
            tx,
        }
    }

    pub fn visible(&mut self) {
        self.visible = true;
    }

    pub fn query(&self) -> &str {
        self.input.value()
    }

    pub fn handle_key(&mut self, event: KeyEvent) -> (bool, Option<T>) {
        match event {
            key!(Esc) => {
                self.visible = false;
                return (true, None);
            }
            key!(Enter) => {
                if let Some(idx) = self.table.selected() {
                    self.table.select(None);
                    let selected = self.option(idx);
                    if self.history.len() >= 20 {
                        self.history.pop();
                    }
                    self.history.retain(|v| v != &selected);
                    self.history.insert(0, selected.clone());

                    self.input.reset();
                    self.options.lock().unwrap().clear();
                    self.visible = false;
                    return (true, Some(selected));
                }
            }
            key!(Up) => {
                let idx = cycle::prev_opt(self.table.selected(), self.options_length());
                self.table.select(idx);
            }
            key!(Down) => {
                let idx = cycle::next_opt(self.table.selected(), self.options_length());
                self.table.select(idx);
            }
            _ => {
                let evt = crossterm::event::Event::Key(event);
                if self.input.handle_event(&evt).is_some() {
                    let _ = self.tx.send(self.input.to_string());
                }
            }
        }
        (false, None)
    }

    pub fn options(&self) -> Vec<T> {
        let opts = self.options.lock().unwrap();
        if !opts.is_empty() {
            return opts.clone();
        }
        if self.input.value().is_empty() {
            self.history.clone()
        } else {
            vec![]
        }
    }

    fn options_length(&self) -> usize {
        let opts = self.options.lock().unwrap();
        if !opts.is_empty() {
            return opts.len();
        }
        if self.input.value().is_empty() {
            self.history.len()
        } else {
            0
        }
    }

    fn option(&self, index: usize) -> T {
        self.options
            .lock()
            .unwrap()
            .get(index)
            .or_else(|| self.history.get(index))
            .cloned()
            .expect("index in range")
    }

    /// If Enter is pressed with non-empty input and no dropdown selection, closes the popup
    /// and returns the raw typed query so the caller can navigate directly by symbol.
    pub fn consume_direct_enter(&mut self, event: KeyEvent) -> Option<String> {
        if matches!(event, key!(Enter))
            && self.table.selected().is_none()
            && !self.input.value().is_empty()
        {
            let query = self.input.value().to_string();
            self.input.reset();
            let _ = self.tx.send(String::new());
            self.visible = false;
            Some(query)
        } else {
            None
        }
    }
}
