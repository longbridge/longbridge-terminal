use std::sync::{atomic::Ordering, Mutex};

use atomic::Atomic;
use bevy_ecs::system::CommandQueue;
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
    Frame,
};
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tui_markdown::{Options, StyleSheet};
use unicode_width::UnicodeWidthStr;

use crate::{data::Counter, tui::app::RT, tui::ui::styles};

/// The "open on the web" affordance in the article pane's title bar.
const OPEN_ICON: &str = " ↗ ";
const OPEN_ICON_WIDTH: u16 = 3;

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

#[derive(Clone, Copy, PartialEq, Eq, Debug, bytemuck::NoUninit, Default)]
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
    pub published_at: OffsetDateTime,
}

pub static NEWS_ITEMS: std::sync::LazyLock<Mutex<Vec<NewsItem>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

pub static NEWS_LIST_STATE: std::sync::LazyLock<Mutex<ListState>> =
    std::sync::LazyLock::new(|| Mutex::new(ListState::default()));

pub static NEWS_DETAIL_CONTENT: std::sync::LazyLock<Mutex<String>> =
    std::sync::LazyLock::new(|| Mutex::new(String::new()));

/// The security the loaded list belongs to. The dock and the full list view
/// share one fetch, so toggling between them does not re-request.
pub static NEWS_SYMBOL: std::sync::LazyLock<Mutex<String>> =
    std::sync::LazyLock::new(|| Mutex::new(String::new()));

/// Bumped by every news load; a task whose number is no longer the latest is
/// answering for a stock, or an article, the reader has already left.
///
/// Without it the panes are last-writer-wins across the network: walking a
/// watchlist with the dock open starts a fetch per row, and the one that
/// happens to answer last fills the pane — which is not the one being shown.
static NEWS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether this task still speaks for what is on screen.
fn news_is_current(generation: u64) -> bool {
    NEWS_GENERATION.load(Ordering::Relaxed) == generation
}

pub static NEWS_LOADING: Atomic<bool> = Atomic::new(false);
pub static NEWS_DETAIL_LOADING: Atomic<bool> = Atomic::new(false);
pub static NEWS_DETAIL_SCROLL: Atomic<u16> = Atomic::new(0);

/// The furthest the article can scroll, as of the last render. Kept so the
/// scroll keys and the wheel stop at the end of the text instead of running on
/// into blank space.
pub static NEWS_DETAIL_MAX_SCROLL: Atomic<u16> = Atomic::new(0);

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

/// A headline's published time, as a reader scans it: local `MM-DD HH:MM`.
/// The RFC 3339 stamp the API returns is precise but unreadable at a glance.
fn fmt_published(dt: OffsetDateTime) -> String {
    let local =
        dt.to_offset(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC));
    format!(
        "{:02}-{:02} {:02}:{:02}",
        local.month() as u8,
        local.day(),
        local.hour(),
        local.minute()
    )
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

/// Load `counter`'s news unless the list already holds it. The news dock shows
/// without the user asking, so the fetch has to be idempotent per security.
pub fn ensure_news(counter: Counter, tx: mpsc::UnboundedSender<CommandQueue>) {
    let symbol = counter.to_string();
    let loaded = NEWS_SYMBOL.lock().expect("poison").as_str() == symbol;
    if loaded && !NEWS_ITEMS.lock().expect("poison").is_empty() {
        return;
    }
    fetch_news(counter, tx);
}

pub fn fetch_news(counter: Counter, tx: mpsc::UnboundedSender<CommandQueue>) {
    let generation = NEWS_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    *NEWS_SYMBOL.lock().expect("poison") = counter.to_string();
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
                            published_at: item.published_at,
                        }
                    })
                    .collect();

                if !news_is_current(generation) {
                    return;
                }
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
        if !news_is_current(generation) {
            return;
        }
        NEWS_LOADING.store(false, Ordering::Relaxed);
        let _ = tx.send(CommandQueue::default());
    });
}

pub fn fetch_news_detail(id: String, tx: mpsc::UnboundedSender<CommandQueue>) {
    let generation = NEWS_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    NEWS_DETAIL_LOADING.store(true, Ordering::Relaxed);
    NEWS_DETAIL_SCROLL.store(0, Ordering::Relaxed);
    if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
        content.clear();
    }

    RT.get().unwrap().spawn(async move {
        // Same source as `longbridge news detail`: GET /v1/content/news/{id}
        // via the signed OpenAPI client (the old longbridge.com/news/{id}.md
        // scrape is gone).
        let result = match id.parse::<i64>() {
            Ok(id) => crate::openapi::news::news_detail(id).await,
            Err(_) => Err(anyhow::anyhow!("invalid news id: {id}")),
        };
        match result {
            Ok(item) => {
                let mut meta = vec![crate::cli::output::fmt_unix_ts(item.published_at)];
                if !item.author.name.is_empty() {
                    meta.push(item.author.name.clone());
                }
                if !item.tickers.is_empty() {
                    meta.push(item.tickers.join(" "));
                }
                let body = if item.body.is_empty() {
                    &item.description
                } else {
                    &item.body
                };
                let title = if item.title.is_empty() {
                    crate::cli::news::truncate_display(&item.description, 70)
                } else {
                    item.title.clone()
                };
                let text = format!("# {title}\n\n{}\n\n{body}", meta.join(" · "));
                if !news_is_current(generation) {
                    return;
                }
                if let Ok(mut content) = NEWS_DETAIL_CONTENT.lock() {
                    *content = prepare_article(&text);
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch news detail: {e}");
                if !news_is_current(generation) {
                    return;
                }
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

/// Scroll the article by `lines` (negative scrolls up), clamped to its length.
pub fn news_detail_scroll_by(lines: i32) {
    let current = NEWS_DETAIL_SCROLL.load(Ordering::Relaxed);
    let max = NEWS_DETAIL_MAX_SCROLL.load(Ordering::Relaxed);
    let delta = lines.unsigned_abs().min(u32::from(u16::MAX)) as u16;
    let next = if lines < 0 {
        current.saturating_sub(delta)
    } else {
        current.saturating_add(delta).min(max)
    };
    NEWS_DETAIL_SCROLL.store(next, Ordering::Relaxed);
}

pub fn news_detail_scroll_up() {
    news_detail_scroll_by(-3);
}

pub fn news_detail_scroll_down() {
    news_detail_scroll_by(3);
}

/// The id of the currently selected news item, if any.
pub fn selected_news_id() -> Option<String> {
    let state = NEWS_LIST_STATE.lock().expect("poison");
    let idx = state.selected()?;
    drop(state);
    NEWS_ITEMS
        .lock()
        .expect("poison")
        .get(idx)
        .map(|item| item.id.clone())
}

/// The article's page on longbridge.com. The list API also carries a source
/// URL, but the Longbridge page is the canonical destination and always exists.
#[must_use]
pub fn news_web_url(id: &str) -> String {
    format!("https://longbridge.com/news/{id}")
}

/// The web URL of the currently selected news item, if any.
pub fn selected_news_url() -> Option<String> {
    selected_news_id().as_deref().map(news_web_url)
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
    if let Ok(mut symbol) = NEWS_SYMBOL.lock() {
        symbol.clear();
    }
    NEWS_DETAIL_SCROLL.store(0, Ordering::Relaxed);
    NEWS_DETAIL_MAX_SCROLL.store(0, Ordering::Relaxed);
}

// ─── Rendering ───────────────────────────────────────────────────────────────

/// Renders the news list into `rect`.
///
/// Every item is two rows — headline, then a dim published time. Titles alone
/// pack too tightly to tell one story from the next, so the second row is what
/// separates them, not decoration.
fn render_news_list(frame: &mut Frame, rect: Rect) {
    let title_str = t!("News.Title");
    // The way out rides the list's border, mirroring `News [n]` on the quote
    // panel — the pair replaces the tab strip that used to sit above both.
    let back_label = format!(" {} ", t!("News.BackKey"));
    let back_width = back_label.width() as u16;
    let block = Block::default()
        .title(format!(" {title_str} "))
        .title_top(Line::from(Span::styled(back_label, styles::hint_key())).right_aligned())
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(styles::border());
    frame.render_widget(block, rect);
    *crate::tui::mouse::NEWS_BACK_RECT.lock().expect("poison") = Rect {
        x: rect.x + rect.width.saturating_sub(1 + back_width),
        y: rect.y,
        width: back_width.min(rect.width),
        height: 1,
    };

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
            ListItem::new(vec![
                Line::from(item.title.clone()),
                Line::from(Span::styled(
                    fmt_published(item.published_at),
                    styles::dark_gray(),
                )),
            ])
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

/// The news list, as its own pane.
pub fn render_news_list_view(frame: &mut Frame, rect: Rect) {
    render_news_list(frame, rect);
}

/// Rows a list item occupies: its headline and its published time.
pub const NEWS_ITEM_ROWS: u16 = 2;

/// Split view: 3/10 mini list on the left, 7/10 article detail on the right.
pub fn render_news_detail_view(frame: &mut Frame, rect: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(3, 10), Constraint::Ratio(7, 10)])
        .split(rect);

    render_news_list(frame, chunks[0]);

    // ── Detail pane ──────────────────────────────────────────────────────────
    render_news_detail_pane(frame, chunks[1]);
}

/// Draw the article pane: the markdown body, wrapped and scrollable with a
/// scrollbar, under a title bar carrying a clickable "open on the web" icon.
fn render_news_detail_pane(frame: &mut Frame, rect: Rect) {
    let detail_title = t!("News.Detail");
    let article_url = selected_news_url();

    let mut block = Block::default()
        .title(format!(" {detail_title} "))
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(styles::border());
    if article_url.is_some() {
        block =
            block.title_top(Line::from(Span::styled(OPEN_ICON, styles::link())).right_aligned());
    }
    frame.render_widget(block, rect);

    // The icon opens the article on longbridge.com. It sits on the top border,
    // ending one column before the corner.
    if let Some(url) = article_url {
        crate::tui::mouse::register_link(
            Rect {
                x: rect.x + rect.width.saturating_sub(1 + OPEN_ICON_WIDTH),
                y: rect.y,
                width: OPEN_ICON_WIDTH,
                height: 1,
            },
            url,
        );
    }

    let inner = rect.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    *crate::tui::mouse::NEWS_DETAIL_RECT.lock().expect("poison") = inner;

    // Content area + a 1-line key hint bar at the bottom.
    let [content_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let hint = Paragraph::new(t!("News.DetailHint").to_string()).style(styles::dark_gray());

    if NEWS_DETAIL_LOADING.load(Ordering::Relaxed) {
        frame.render_widget(Paragraph::new("Loading..."), content_area);
        frame.render_widget(hint, hint_area);
        return;
    }

    let content = NEWS_DETAIL_CONTENT.lock().expect("poison").clone();
    if content.is_empty() {
        NEWS_DETAIL_MAX_SCROLL.store(0, Ordering::Relaxed);
        frame.render_widget(Paragraph::new(t!("News.DetailEmpty")), content_area);
        frame.render_widget(hint, hint_area);
        return;
    }

    // Reserve the right-hand column for the scrollbar so wrapped text never
    // runs under it.
    let text_area = Rect {
        width: content_area.width.saturating_sub(1),
        ..content_area
    };
    let md_text = tui_markdown::from_str_with_options(&content, &Options::new(NewsStyleSheet));
    let article = Paragraph::new(md_text).wrap(Wrap { trim: false });

    // Clamp the stored offset to what the wrapped text actually needs, so the
    // scroll keys and the wheel stop at the last line.
    let total = article.line_count(text_area.width) as u16;
    let max_scroll = total.saturating_sub(text_area.height);
    NEWS_DETAIL_MAX_SCROLL.store(max_scroll, Ordering::Relaxed);
    let scroll = NEWS_DETAIL_SCROLL.load(Ordering::Relaxed).min(max_scroll);
    NEWS_DETAIL_SCROLL.store(scroll, Ordering::Relaxed);

    frame.render_widget(article.scroll((scroll, 0)), text_area);

    if max_scroll > 0 {
        let mut state = ScrollbarState::new(max_scroll as usize).position(scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("▐")
                .thumb_style(styles::dark_gray()),
            Rect {
                x: content_area.x + content_area.width.saturating_sub(1),
                width: 1,
                ..content_area
            },
            &mut state,
        );
    }

    frame.render_widget(hint, hint_area);
}
