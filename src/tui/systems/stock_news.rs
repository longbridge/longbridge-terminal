use std::sync::{atomic::Ordering, Mutex};

use atomic::Atomic;
use bevy_ecs::system::CommandQueue;
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
    Frame,
};
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tui_markdown::{Options, StyleSheet};

use crate::{data::Counter, tui::app::RT, tui::ui::styles, utils::datetime::format_datetime};

// ─── Markdown StyleSheet ─────────────────────────────────────────────────────

/// Custom dark-terminal-friendly stylesheet for news article rendering.
#[derive(Clone, Copy)]
struct NewsStyleSheet;

impl StyleSheet for NewsStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::new()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
            2 => Style::new()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
            3 => Style::new()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
            _ => Style::new().fg(Color::Green).add_modifier(Modifier::ITALIC),
        }
    }

    fn code(&self) -> Style {
        Style::new().fg(Color::LightCyan).bg(Color::DarkGray)
    }

    fn link(&self) -> Style {
        Style::new()
            .fg(Color::Cyan)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::new().fg(Color::LightBlue)
    }

    fn heading_meta(&self) -> Style {
        Style::new().fg(Color::DarkGray)
    }

    fn metadata_block(&self) -> Style {
        Style::new().fg(Color::DarkGray)
    }
}

// ─── State ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, bytemuck::NoUninit, Default)]
#[repr(u8)]
pub enum NewsView {
    #[default]
    Quote = 0,
    List = 1,
    Detail = 2,
}

pub static NEWS_VIEW: Atomic<NewsView> = Atomic::new(NewsView::Quote);

// ─── Data ────────────────────────────────────────────────────────────────────

pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub url: String,
    pub published_at: OffsetDateTime,
}

pub static NEWS_ITEMS: std::sync::LazyLock<Mutex<Vec<NewsItem>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

pub static NEWS_LIST_STATE: std::sync::LazyLock<Mutex<ListState>> =
    std::sync::LazyLock::new(|| Mutex::new(ListState::default()));

pub static NEWS_DETAIL_CONTENT: std::sync::LazyLock<Mutex<String>> =
    std::sync::LazyLock::new(|| Mutex::new(String::new()));

pub static NEWS_LOADING: Atomic<bool> = Atomic::new(false);
pub static NEWS_DETAIL_LOADING: Atomic<bool> = Atomic::new(false);
pub static NEWS_DETAIL_SCROLL: Atomic<u16> = Atomic::new(0);

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Strip YAML frontmatter, returning only the article body.
fn prepare_article(text: &str) -> String {
    let s = text.trim_start();
    if !s.starts_with("---") {
        return text.to_owned();
    }
    let after_open = &s[3..];
    if let Some(end) = after_open.find("\n---") {
        after_open[end + 4..].trim_start_matches('\n').to_owned()
    } else {
        text.to_owned()
    }
}

fn truncate_title(title: &str, max: usize) -> String {
    if title.chars().count() > max {
        format!(
            "{}…",
            title
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        title.to_owned()
    }
}

// ─── Actions ─────────────────────────────────────────────────────────────────

pub fn fetch_news(counter: Counter, tx: mpsc::UnboundedSender<CommandQueue>) {
    NEWS_LOADING.store(true, Ordering::Relaxed);
    if let Ok(mut items) = NEWS_ITEMS.lock() {
        items.clear();
    }
    if let Ok(mut state) = NEWS_LIST_STATE.lock() {
        *state = ListState::default();
    }

    RT.get().unwrap().spawn(async move {
        match crate::openapi::content().news(&counter.to_string()).await {
            Ok(raw_items) => {
                let news_items: Vec<NewsItem> = raw_items
                    .into_iter()
                    .take(50)
                    .map(|item| {
                        let title = if item.title.is_empty() {
                            truncate_title(&item.description, 80)
                        } else {
                            item.title.clone()
                        };
                        NewsItem {
                            id: item.id.clone(),
                            title,
                            url: item.url.clone(),
                            published_at: item.published_at,
                        }
                    })
                    .collect();

                if let Ok(mut stored) = NEWS_ITEMS.lock() {
                    *stored = news_items;
                }
                if let Ok(mut state) = NEWS_LIST_STATE.lock() {
                    state.select(Some(0));
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch news: {}", e);
            }
        }
        NEWS_LOADING.store(false, Ordering::Relaxed);
        let _ = tx.send(CommandQueue::default());
    });
}

pub fn fetch_news_detail(id: String, tx: mpsc::UnboundedSender<CommandQueue>) {
    NEWS_DETAIL_LOADING.store(true, Ordering::Relaxed);
    NEWS_DETAIL_SCROLL.store(0, Ordering::Relaxed);
    if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
        content.clear();
    }

    RT.get().unwrap().spawn(async move {
        let url = match crate::locale::get() {
            "zh-CN" => format!("https://longbridge.com/zh-CN/news/{id}.md"),
            "zh-HK" => format!("https://longbridge.com/zh-HK/news/{id}.md"),
            _ => format!("https://longbridge.com/news/{id}.md"),
        };
        let client = reqwest::Client::new();
        let result = client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await;
        match result {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(text) => {
                    if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
                        *content = prepare_article(&text);
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to read news detail body: {e}");
                    if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
                        *content = t!("News.ErrorContent", error = e.to_string()).to_string();
                    }
                }
            },
            Ok(resp) => {
                let status = resp.status();
                tracing::error!("Failed to fetch news detail: HTTP {status}");
                if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
                    *content = t!("News.ErrorHttp", status = status.to_string()).to_string();
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch news detail: {e}");
                if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
                    *content = t!("News.ErrorFetch", error = e.to_string()).to_string();
                }
            }
        }
        NEWS_DETAIL_LOADING.store(false, Ordering::Relaxed);
        let _ = tx.send(CommandQueue::default());
    });
}

pub fn news_list_up() {
    let len = NEWS_ITEMS.lock().expect("poison").len();
    if len == 0 {
        return;
    }
    let mut state = NEWS_LIST_STATE.lock().expect("poison");
    let new_idx = state
        .selected()
        .map_or(0, |i| if i == 0 { len - 1 } else { i - 1 });
    state.select(Some(new_idx));
}

pub fn news_list_down() {
    let len = NEWS_ITEMS.lock().expect("poison").len();
    if len == 0 {
        return;
    }
    let mut state = NEWS_LIST_STATE.lock().expect("poison");
    let new_idx = state.selected().map_or(0, |i| (i + 1) % len);
    state.select(Some(new_idx));
}

pub fn news_detail_scroll_up() {
    let s = NEWS_DETAIL_SCROLL.load(Ordering::Relaxed);
    NEWS_DETAIL_SCROLL.store(s.saturating_sub(3), Ordering::Relaxed);
}

pub fn news_detail_scroll_down() {
    let s = NEWS_DETAIL_SCROLL.load(Ordering::Relaxed);
    NEWS_DETAIL_SCROLL.store(s.saturating_add(3), Ordering::Relaxed);
}

/// Returns `(id, url)` for the currently selected news item, if any.
pub fn selected_news_item() -> Option<(String, String)> {
    let state = NEWS_LIST_STATE.lock().expect("poison");
    let idx = state.selected()?;
    drop(state);
    NEWS_ITEMS
        .lock()
        .expect("poison")
        .get(idx)
        .map(|item| (item.id.clone(), item.url.clone()))
}

// Kept for callers that only need the id.
pub fn selected_news_id() -> Option<String> {
    selected_news_item().map(|(id, _)| id)
}

/// Returns the web URL of the currently selected news item, if any.
pub fn selected_news_url() -> Option<String> {
    selected_news_item().map(|(_, url)| url)
}

pub fn reset_news_view() {
    NEWS_VIEW.store(NewsView::Quote, Ordering::Relaxed);
    if let Ok(mut items) = NEWS_ITEMS.lock() {
        items.clear();
    }
    if let Ok(mut state) = NEWS_LIST_STATE.lock() {
        *state = ListState::default();
    }
    if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
        content.clear();
    }
    NEWS_DETAIL_SCROLL.store(0, Ordering::Relaxed);
}

// ─── Rendering ───────────────────────────────────────────────────────────────

/// Renders the news list into `rect`. In compact mode only titles are shown
/// (no timestamp, no spacing), intended for the narrow 3/10 column.
fn render_news_list(frame: &mut Frame, rect: Rect, compact: bool) {
    let title_str = t!("News.Title");
    let block = Block::default()
        .title(format!(" {title_str} "))
        .borders(Borders::ALL)
        .border_style(styles::border());
    frame.render_widget(block, rect);

    let inner = rect.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });

    let loading = NEWS_LOADING.load(Ordering::Relaxed);
    if loading {
        frame.render_widget(Paragraph::new("Loading..."), inner);
        return;
    }

    let items = NEWS_ITEMS.lock().expect("poison");
    if items.is_empty() {
        frame.render_widget(Paragraph::new(t!("News.Empty")), inner);
        return;
    }

    let total = items.len();

    // Reserve one column on the right for the scrollbar.
    let list_width = inner.width.saturating_sub(1);
    let list_area = Rect {
        width: list_width,
        ..inner
    };
    *crate::tui::mouse::NEWS_LIST_RECT.lock().expect("poison") = list_area;
    let scrollbar_area = Rect {
        x: inner.x + list_width,
        y: inner.y,
        width: 1,
        height: inner.height,
    };

    let list_items: Vec<ListItem> = items
        .iter()
        .map(|item| {
            if compact {
                ListItem::new(truncate_title(&item.title, list_width as usize))
            } else {
                ListItem::new(vec![
                    Line::from(item.title.clone()),
                    Line::from(Span::styled(
                        format_datetime(item.published_at),
                        styles::dark_gray(),
                    )),
                ])
            }
        })
        .collect();

    drop(items); // release lock before acquiring list state

    let list =
        List::new(list_items).highlight_style(styles::text().add_modifier(Modifier::REVERSED));

    let mut list_state = NEWS_LIST_STATE.lock().expect("poison");
    let selected = list_state.selected().unwrap_or(0);
    frame.render_stateful_widget(list, list_area, &mut *list_state);
    drop(list_state);

    let mut scrollbar_state = ScrollbarState::new(total).position(selected);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None);
    frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
}

/// Full-width news list view (news mode without a selected article).
pub fn render_news_list_view(frame: &mut Frame, rect: Rect) {
    render_news_list(frame, rect, false);
}

/// Split view: 3/10 mini list on the left, 7/10 article detail on the right.
pub fn render_news_detail_view(frame: &mut Frame, rect: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(3, 10), Constraint::Ratio(7, 10)])
        .split(rect);

    render_news_list(frame, chunks[0], false);

    // ── Detail pane ──────────────────────────────────────────────────────────
    let detail_title = t!("News.Detail");
    let block = Block::default()
        .title(format!(" {detail_title} "))
        .borders(Borders::ALL)
        .border_style(styles::border());
    frame.render_widget(block, chunks[1]);

    let inner = chunks[1].inner(Margin {
        vertical: 1,
        horizontal: 1,
    });

    // Split inner: content area + 1-line key hint bar at the bottom
    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);
    let content_area = inner_chunks[0];
    let hint_area = inner_chunks[1];

    let loading = NEWS_DETAIL_LOADING.load(Ordering::Relaxed);
    if loading {
        frame.render_widget(Paragraph::new("Loading..."), content_area);
        frame.render_widget(
            Paragraph::new(t!("News.DetailHint").to_string()).style(styles::dark_gray()),
            hint_area,
        );
        return;
    }

    let content = NEWS_DETAIL_CONTENT.lock().expect("poison").clone();
    let scroll = NEWS_DETAIL_SCROLL.load(Ordering::Relaxed);

    if content.is_empty() {
        frame.render_widget(Paragraph::new(t!("News.DetailEmpty")), content_area);
    } else {
        let md_text = tui_markdown::from_str_with_options(&content, &Options::new(NewsStyleSheet));
        frame.render_widget(Paragraph::new(md_text).scroll((scroll, 0)), content_area);
    }

    frame.render_widget(
        Paragraph::new(t!("News.DetailHint").to_string()).style(styles::dark_gray()),
        hint_area,
    );
}
