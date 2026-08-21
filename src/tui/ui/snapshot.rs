//! Headless UI snapshot exporter (dev-only, test-gated).
//!
//! Renders view components into an in-memory `TestBackend` buffer (no terminal,
//! no auth, no network) and exports the buffer to a self-contained HTML file
//! that approximates a dark terminal. This lets visuals be inspected/iterated
//! without a live TUI: run `cargo test ui_snapshot -- --nocapture`, then open
//! (or screenshot) the generated files under `target/ui-snapshots/`.

#![cfg(test)]

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

const PAGE_BG: &str = "#0d0d0d";
const PAGE_FG: &str = "#c8ccd4";

/// Map a ratatui color to a CSS color for a typical dark terminal.
/// Returns `None` for `Reset` (caller substitutes the default fg/bg).
fn css_color(c: Color) -> Option<String> {
    let s = match c {
        Color::Reset => return None,
        Color::Black => "#0c0c0c",
        Color::Red => "#c05a5a",
        Color::Green => "#5fae7f",
        Color::Yellow => "#c8a35a",
        Color::Blue => "#5a8ac8",
        Color::Magenta => "#a05aa0",
        Color::Cyan => "#5aa0a0",
        Color::Gray => "#a6acb8",
        Color::DarkGray => "#585c66",
        Color::LightRed => "#f06c7a",
        Color::LightGreen => "#7bd88f",
        Color::LightYellow => "#e6c07b",
        Color::LightBlue => "#7aa2f7",
        Color::LightMagenta => "#c792ea",
        Color::LightCyan => "#7fdbca",
        Color::White => "#e6e6e6",
        Color::Rgb(r, g, b) => return Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Indexed(i) => {
            let (r, g, b) = xterm256_rgb(i);
            return Some(format!("#{r:02x}{g:02x}{b:02x}"));
        }
    };
    Some(s.to_string())
}

/// The xterm-256 palette entry for `i`: 0–15 map to the ANSI colors this page
/// already themes, 16–231 to the 6x6x6 color cube, 232–255 to the gray ramp.
fn xterm256_rgb(i: u8) -> (u8, u8, u8) {
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0..=15 => {
            let base = if i < 8 { 0x80 } else { 0xff };
            let bit = |n: u8| u8::from(i & (1 << n) != 0) * base;
            (bit(0), bit(1), bit(2))
        }
        16..=231 => {
            let n = i - 16;
            (
                CUBE[(n / 36) as usize],
                CUBE[(n / 6 % 6) as usize],
                CUBE[(n % 6) as usize],
            )
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

fn esc(sym: &str) -> String {
    sym.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Serialize a rendered buffer to a standalone HTML page.
fn buffer_to_html(buf: &Buffer, title: &str) -> String {
    let area = buf.area();
    let mut body = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = buf.cell((area.x + x, area.y + y)).expect("cell in bounds");
            let sym = cell.symbol();
            // Skip wide-glyph continuation cells (empty symbol) so CJK keeps
            // its natural 2-column width in a monospace font.
            if sym.is_empty() {
                continue;
            }
            let st = cell.style();
            let mut fg = st.fg.and_then(css_color);
            let mut bg = st.bg.and_then(css_color);
            if st.add_modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
                fg = Some(fg.unwrap_or_else(|| PAGE_BG.to_string()));
                bg = Some(bg.unwrap_or_else(|| PAGE_FG.to_string()));
            }
            let mut style = String::new();
            if let Some(fg) = fg {
                style.push_str(&format!("color:{fg};"));
            }
            if let Some(bg) = bg {
                style.push_str(&format!("background:{bg};"));
            }
            if st.add_modifier.contains(Modifier::BOLD) {
                style.push_str("font-weight:700;");
            }
            if st.add_modifier.contains(Modifier::DIM) {
                style.push_str("opacity:.6;");
            }
            if st.add_modifier.contains(Modifier::UNDERLINED) {
                style.push_str("text-decoration:underline;");
            }
            body.push_str(&format!("<span style=\"{style}\">{}</span>", esc(sym)));
        }
        body.push('\n');
    }
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
<style>body{{margin:0;background:{PAGE_BG};}}\
pre{{margin:0;padding:16px;color:{PAGE_FG};background:{PAGE_BG};\
font:15px/1.2 'Menlo','DejaVu Sans Mono','Consolas',monospace;\
white-space:pre;display:inline-block;letter-spacing:0;}}</style>\
<pre>{body}</pre>"
    )
}

fn out_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-snapshots");
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    dir
}

/// Render a view via a `TestBackend` of the given size and write it to
/// `target/ui-snapshots/<name>.html`.
fn snapshot<F>(name: &str, width: u16, height: u16, draw: F)
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    terminal.draw(|f| draw(f)).expect("draw");
    let buf = terminal.backend().buffer().clone();
    print_text(name, &buf);
    let html = buffer_to_html(&buf, name);
    let path = out_dir().join(format!("{name}.html"));
    std::fs::write(&path, html).expect("write snapshot");
    println!("ui-snapshot: {}", path.display());
}

/// Print a buffer as plain text, so a snapshot can also be eyeballed straight
/// from `cargo test -- --nocapture` without opening the HTML.
fn print_text(name: &str, buf: &Buffer) {
    let area = buf.area();
    println!("--- {name} ({}x{}) ---", area.width, area.height);
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(buf.cell((area.x + x, area.y + y)).expect("cell").symbol());
        }
        println!("|{}|", line.trim_end());
    }
}

/// Serialized against [`footer_packs_quotes_and_drops_whole_ones`]: both render
/// the footer, and the rects it asserts on are published through a single
/// process-global (`mouse::FOOTER_INDEX_RECTS`), so run in parallel each
/// overwrites the other's.
#[test]
#[serial_test::serial(footer_rects)]
fn ui_snapshot_export() {
    rust_i18n::set_locale("en");

    // Navbar (top bar) on the Orders screen — proves the pipeline end-to-end
    // with no fixtures (reads only global keymap + default account channel).
    // Repeated across widths because the hint row sheds shortcuts as it
    // narrows, and the point of the layout is that Settings never falls off.
    for width in [150u16, 110, 90, 70, 50] {
        let name = format!("navbar-{width}");
        snapshot(&name, width, 1, |f| {
            crate::tui::views::navbar::render(f, f.area(), crate::tui::app::AppState::Orders);
        });
    }

    // Footer index row, at widths where it has to shed quotes. Quotes are
    // packed left to right, so a wide bar must not read as gaps with numbers
    // in it, and a narrow one drops a whole quote rather than clipping one.
    for width in [150u16, 110, 70] {
        let name = format!("footer-{width}");
        snapshot(&name, width, 1, |f| {
            let indexes = [
                crate::data::Counter::new("HSI.HK"),
                crate::data::Counter::new("HSCEI.HK"),
                crate::data::Counter::new("HSTECH.HK"),
            ];
            let state = crate::tui::systems::WsState(crate::data::ReadyState::Open);
            crate::tui::views::footer::render(f, f.area(), &indexes, &state);
        });
    }

    // The shared Portfolio/Orders detail panel.
    snapshot("detail-panel", 44, 20, |f| {
        use crate::tui::views::detail::Row;
        let rows = [
            Row::Text(ratatui::text::Line::from("US AAPL")),
            Row::Blank,
            Row::Section("Position".into()),
            Row::field("Qty", "300"),
            Row::field("Cost", "150.00 USD"),
            Row::Blank,
            Row::Section("Performance".into()),
            Row::styled(
                "P/L",
                "▲ +1,230.00",
                crate::tui::ui::styles::up(std::cmp::Ordering::Greater),
            ),
        ];
        crate::tui::views::detail::render(f, f.area(), "Apple", &rows, vec![]);
    });
}

/// The home-page URL must be clickable wherever it is drawn: the mouse handler
/// resolves clicks through `mouse::link_at`, so a rendered banner that fails to
/// register its link is a silently dead link.
#[test]
fn banner_url_is_clickable() {
    use ratatui::widgets::Widget;

    crate::tui::mouse::clear_links();
    let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 30));
    let area = *buf.area();
    crate::tui::ui::assets::banner(ratatui::style::Style::default()).render(area, &mut buf);

    let hit = (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| (x, y)))
        .find_map(|(x, y)| crate::tui::mouse::link_at(x, y));
    assert_eq!(hit.as_deref(), Some(crate::tui::ui::assets::HOME_URL));
}

#[test]
fn help_popup_url_is_clickable() {
    rust_i18n::set_locale("en");
    crate::tui::mouse::clear_links();

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    terminal
        .draw(|f| crate::tui::views::help::render(f, f.area()))
        .expect("draw");

    // The registered rect must sit on the row that actually shows the URL.
    let buf = terminal.backend().buffer();
    let area = *buf.area();
    let hit = (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| (x, y)))
        .find(|&(x, y)| crate::tui::mouse::link_at(x, y).is_some())
        .expect("help popup registers its URL as a link");
    let row: String = (0..area.width)
        .map(|x| buf.cell((x, hit.1)).expect("cell").symbol())
        .collect();
    assert!(
        row.contains(crate::tui::ui::assets::HOME_URL),
        "link rect is on row {:?}",
        row.trim()
    );
}

/// The footer packs its quotes left to right and drops a whole quote rather
/// than clipping one, so a wide bar is not mostly gaps and a narrow one is not
/// a half-written number.
///
/// Deliberately locale-agnostic. `rust_i18n`'s locale is process-global and
/// other tests change it, so anything asserted about an exact column here would
/// depend on which test ran last — the layout rules below hold for any label.
#[test]
#[serial_test::serial(footer_rects)]
fn footer_packs_quotes_and_drops_whole_ones() {
    let indexes = [
        crate::data::Counter::new("HSI.HK"),
        crate::data::Counter::new("HSCEI.HK"),
        crate::data::Counter::new("HSTECH.HK"),
    ];
    let state = crate::tui::systems::WsState(crate::data::ReadyState::Open);

    let quotes = |width: u16| -> Vec<ratatui::layout::Rect> {
        let backend = ratatui::backend::TestBackend::new(width, 1);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|f| crate::tui::views::footer::render(f, f.area(), &indexes, &state))
            .expect("draw");
        crate::tui::mouse::FOOTER_INDEX_RECTS
            .lock()
            .expect("poison")
            .iter()
            .copied()
            .filter(|r| r.width > 0)
            .collect()
    };

    // Wide enough for all three under any locale: they are all shown, and each
    // begins right after the one before rather than at a fixed offset.
    let wide = quotes(150);
    assert_eq!(wide.len(), 3, "all three quotes should fit in 150 columns");
    for pair in wide.windows(2) {
        let gap = pair[1].x - (pair[0].x + pair[0].width);
        assert!(
            gap <= 6,
            "quotes should be adjacent, got a {gap}-column gap"
        );
    }

    // Whatever survives always ends inside the bar — a quote is dropped whole,
    // never clipped mid-number — and narrowing never gains one back.
    let mut previous = usize::MAX;
    for width in [150u16, 120, 100, 80, 60, 40, 20, 10] {
        let kept = quotes(width);
        for r in &kept {
            assert!(
                r.x + r.width <= width,
                "a quote runs past the {width}-column bar"
            );
        }
        assert!(
            kept.len() <= previous,
            "narrowing to {width} columns showed more quotes, not fewer"
        );
        previous = kept.len();
    }
}
