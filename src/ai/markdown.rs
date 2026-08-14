//! Markdown rendering for assistant answers.
//!
//! Longbridge AI replies in Markdown (headings, bold, lists, code fences,
//! tables). `tui-markdown` turns that into styled [`Line`]s using its default
//! stylesheet; this module owns the result (so it can outlive the source
//! string) and re-wraps each line to the body width, preserving per-span styles
//! and honoring wide (CJK) glyphs — `tui-markdown` itself does not wrap.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Render `md` into owned, width-wrapped styled lines.
pub fn render(md: &str, width: usize) -> Vec<Line<'static>> {
    let text = tui_markdown::from_str(md);
    let mut out = Vec::new();
    for line in &text.lines {
        wrap_line(line, width, &mut out);
    }
    out
}

/// Wrap one styled line to `width` display columns, splitting between glyphs and
/// carrying each glyph's style onto the continuation lines.
fn wrap_line(line: &Line, width: usize, out: &mut Vec<Line<'static>>) {
    let flat: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    if flat.is_empty() {
        out.push(Line::from(String::new()));
        return;
    }
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut w = 0;
    for (ch, style) in flat {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width > 0 && w + cw > width && !cur.is_empty() {
            out.push(coalesce(&cur));
            cur.clear();
            w = 0;
        }
        cur.push((ch, style));
        w += cw;
    }
    out.push(coalesce(&cur));
}

/// Merge a run of styled chars into a [`Line`], grouping adjacent equal styles
/// into single spans.
fn coalesce(chars: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut current: Option<Style> = None;
    for (ch, style) in chars {
        if current != Some(*style) {
            if let Some(prev) = current {
                spans.push(Span::styled(std::mem::take(&mut buf), prev));
            }
            current = Some(*style);
        }
        buf.push(*ch);
    }
    if let Some(prev) = current {
        spans.push(Span::styled(buf, prev));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn bold_becomes_a_styled_span() {
        let lines = render("**hi** there", 80);
        let styled = lines.iter().flat_map(|l| &l.spans).any(|s| {
            s.content.contains("hi")
                && s.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
        });
        assert!(styled, "expected a bold span for **hi**");
    }

    #[test]
    fn long_line_wraps_to_width() {
        let lines = render("aaaa bbbb cccc dddd", 9);
        assert!(lines.len() > 1, "expected wrapping at width 9");
        for line in &lines {
            let w: usize = line
                .spans
                .iter()
                .flat_map(|s| s.content.chars())
                .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
                .sum();
            assert!(w <= 9, "line exceeds width: {w}");
        }
    }
}
