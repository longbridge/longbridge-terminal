use ratatui::{
    prelude::{Alignment, Rect},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    tui::app::{AppState, ACCOUNT_CHANNEL},
    tui::keymap::{ActionDef, ActionId},
    tui::ui::{styles, tabs::Tab},
};

/// Blank columns kept between the tab strip and the shortcut hints.
const STRIP_GAP: u16 = 2;

pub fn render(frame: &mut Frame, rect: Rect, state: AppState) {
    let tabs_width = render_tabs(frame, rect, state);

    // Everything the tabs did not claim belongs to the hint row.
    let hints_x = rect.x + tabs_width + STRIP_GAP;
    let hints_rect = Rect {
        x: hints_x,
        width: (rect.x + rect.width).saturating_sub(hints_x),
        ..rect
    };
    render_hints(frame, hints_rect, state);
}

/// Draw the primary tabs as pills — ` 1 WATCHLIST ` — with the active one
/// filled in the brand accent. Returns the total width consumed.
fn render_tabs(frame: &mut Frame, rect: Rect, state: AppState) -> u16 {
    let selected = match state {
        AppState::Portfolio => 1usize,
        AppState::Orders => 2,
        _ => 0,
    };
    let tabs = [
        Tab::new(t!("tabs.Watchlist"), "1"),
        Tab::new(t!("tabs.Portfolio"), "2"),
        Tab::new(t!("tabs.Orders"), "3"),
    ];

    let (rects, width) = crate::tui::ui::tabs::pills(frame, rect, &tabs, selected);
    let mut stored = [Rect::default(); 3];
    for (slot, r) in stored.iter_mut().zip(rects) {
        *slot = r;
    }
    *crate::tui::mouse::NAVBAR_TAB_RECTS.lock().expect("poison") = stored;

    width
}

/// One right-aligned shortcut hint: the spans to draw plus the action a click
/// on it triggers. `drop_rank` orders shortcuts for eviction when the row is
/// too narrow — the highest rank goes first, so `Help` and `Settings` survive.
struct Hint {
    spans: Vec<Span<'static>>,
    width: u16,
    action: ActionId,
    drop_rank: u8,
}

fn hint(action: &ActionDef, drop_rank: u8) -> Hint {
    let dim = styles::dark_gray();
    let label = t!(action.label).to_string();
    // Split "Name [key]" into a dim label + a brighter key.
    let spans = match label.rfind('[') {
        Some(open) => {
            let (name, key) = label.split_at(open);
            vec![
                Span::styled(name.to_string(), dim),
                Span::styled(key.to_string(), styles::hint_key()),
            ]
        }
        None => vec![Span::styled(label.clone(), dim)],
    };
    Hint {
        width: label.width() as u16,
        spans,
        action: action.id,
        drop_rank,
    }
}

/// Rank a shortcut for eviction. Screen-specific actions outrank the global
/// chrome they share the row with, and `Help` / `Settings` are never dropped —
/// `Help` documents whatever had to go.
fn drop_rank(action: ActionId, position: usize) -> u8 {
    match action {
        ActionId::ToggleLog => 200,
        ActionId::ForceQuit | ActionId::Quit => 190,
        ActionId::Search => 100,
        ActionId::Help => 1,
        ActionId::OpenSettings => 0,
        // Context hints: later ones are less central to the screen.
        _ => 120u8.saturating_add(u8::try_from(position).unwrap_or(u8::MAX)),
    }
}

fn render_hints(frame: &mut Frame, rect: Rect, state: AppState) {
    if rect.width == 0 {
        return;
    }

    // The paper-trading badge is never dropped: it says which book you trade.
    let account_channel = ACCOUNT_CHANNEL.read().expect("poison").clone();
    let badge: Option<Vec<Span>> =
        (account_channel.as_deref() == Some("lb_papertrading")).then(|| {
            vec![
                Span::styled(t!("account.type.paper").to_string(), styles::bmp()),
                Span::styled(" │ ", styles::dark_gray()),
            ]
        });
    let badge_width = badge
        .as_ref()
        .map_or(0, |spans| spans.iter().map(|s| s.width() as u16).sum());

    // Context-aware shortcut hints derived from the keymap (single source of
    // truth), so the navbar shows the keys actually available on this screen.
    let ctx = crate::tui::keymap::Context::from_state(state);
    let mut hints: Vec<Hint> = crate::tui::keymap::global()
        .navbar_hints(ctx)
        .iter()
        .enumerate()
        .map(|(i, action)| hint(action, drop_rank(action.id, i)))
        .collect();

    // Evict the least important hints until the row fits. Separators are one
    // space wide, so N hints cost sum(widths) + N - 1.
    let budget = rect.width.saturating_sub(badge_width);
    while !hints.is_empty() && row_width(&hints) > budget {
        let victim = hints
            .iter()
            .enumerate()
            .max_by_key(|(_, h)| h.drop_rank)
            .map_or(0, |(i, _)| i);
        hints.remove(victim);
    }

    // Lay the surviving hints out right-aligned, recording a click target for
    // each so the hints double as buttons.
    let row = row_width(&hints);
    let mut x = rect.x + rect.width.saturating_sub(row);
    let mut targets = Vec::with_capacity(hints.len());
    let mut spans: Vec<Span> = badge.unwrap_or_default();
    for (i, h) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
            x += 1;
        }
        spans.extend(h.spans.iter().cloned());
        targets.push((
            Rect {
                x,
                y: rect.y,
                width: h.width,
                height: 1,
            },
            h.action,
        ));
        x += h.width;
    }
    *crate::tui::mouse::NAVBAR_HINT_RECTS.lock().expect("poison") = targets;

    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
        rect,
    );
}

/// Total width of a hint row: every hint plus one space between neighbours.
fn row_width(hints: &[Hint]) -> u16 {
    let sum: u16 = hints.iter().map(|h| h.width).sum();
    sum + (hints.len().saturating_sub(1)) as u16
}

#[cfg(test)]
mod tests {
    use super::{drop_rank, hint, row_width};
    use crate::tui::keymap::{self, ActionId, Context};

    /// Hints that survive a row `width` columns wide, in display order.
    fn fitted(ctx: Context, width: u16) -> Vec<ActionId> {
        let mut hints: Vec<_> = keymap::global()
            .navbar_hints(ctx)
            .iter()
            .enumerate()
            .map(|(i, action)| hint(action, drop_rank(action.id, i)))
            .collect();
        while !hints.is_empty() && row_width(&hints) > width {
            let victim = hints
                .iter()
                .enumerate()
                .max_by_key(|(_, h)| h.drop_rank)
                .map_or(0, |(i, _)| i);
            hints.remove(victim);
        }
        hints.iter().map(|h| h.action).collect()
    }

    #[test]
    fn wide_row_keeps_every_hint() {
        let all = keymap::global().navbar_hints(Context::Orders).len();
        assert_eq!(fitted(Context::Orders, 200).len(), all);
    }

    #[test]
    fn settings_outlives_every_other_hint() {
        // The bug this ordering fixes: Settings used to be the shortcut that
        // fell off a narrow navbar.
        for width in 0..120u16 {
            let kept = fitted(Context::Orders, width);
            if kept.len() == 1 {
                assert_eq!(kept, vec![ActionId::OpenSettings], "at width {width}");
            }
            if kept.len() >= 2 {
                assert!(kept.contains(&ActionId::OpenSettings), "at width {width}");
                assert!(kept.contains(&ActionId::Help), "at width {width}");
            }
        }
    }

    #[test]
    fn narrow_row_drops_hints_instead_of_truncating_one() {
        // A row too narrow for even one hint shows none, never half a label.
        assert!(fitted(Context::Orders, 5).is_empty());
    }
}
