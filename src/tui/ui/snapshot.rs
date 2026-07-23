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
        Color::Indexed(_) => "#a6acb8",
    };
    Some(s.to_string())
}

fn esc(sym: &str) -> String {
    sym.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
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
    let html = buffer_to_html(&buf, name);
    let path = out_dir().join(format!("{name}.html"));
    std::fs::write(&path, html).expect("write snapshot");
    println!("ui-snapshot: {}", path.display());
}

#[test]
fn ui_snapshot_export() {
    rust_i18n::set_locale("en");

    // Navbar (top bar) on the Orders screen — proves the pipeline end-to-end
    // with no fixtures (reads only global keymap + default account channel).
    snapshot("navbar", 150, 1, |f| {
        crate::tui::views::navbar::render(f, f.area(), crate::tui::app::AppState::Orders);
    });
}
