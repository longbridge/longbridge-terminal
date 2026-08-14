//! Markdown rendering for assistant answers.
//!
//! Longbridge AI replies in Markdown (headings, bold, lists, code fences,
//! tables). Prose is handed to `tui-markdown` (owned here so it can outlive the
//! source string, and re-wrapped to width preserving per-span styles and wide
//! CJK glyphs — `tui-markdown` does not wrap). Fenced code blocks and pipe
//! tables are pulled out first and drawn ourselves, since `tui-markdown` renders
//! them flat: code gets a shaded block with an optional language tag, and tables
//! get aligned, box-drawn borders.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Shaded background for code blocks.
const CODE_BG: Color = Color::Rgb(38, 38, 38);
const CODE_FG: Color = Color::Rgb(220, 220, 220);
const BORDER: Color = Color::DarkGray;

/// A top-level block of an answer.
enum Block {
    /// Prose handed to `tui-markdown`.
    Markdown(String),
    /// A fenced code block: `(language, lines)`.
    Code(String, Vec<String>),
    /// A pipe table: rows of cells, first row is the header.
    Table(Vec<Vec<String>>),
}

/// Render `md` into owned, width-wrapped styled lines.
pub fn render(md: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for block in split_blocks(md) {
        match block {
            Block::Markdown(text) => {
                let rendered = tui_markdown::from_str(&text);
                for line in &rendered.lines {
                    wrap_line(line, width, &mut out);
                }
            }
            Block::Code(lang, lines) => render_code(&lang, &lines, width, &mut out),
            Block::Table(rows) => render_table(&rows, &mut out),
        }
    }
    out
}

/// Split an answer into prose / code / table blocks in source order.
fn split_blocks(md: &str) -> Vec<Block> {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(lang) = fence_lang(line) {
            flush_prose(&mut prose, &mut blocks);
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && fence_lang(lines[i]).is_none() {
                body.push(lines[i].to_string());
                i += 1;
            }
            i += 1; // consume the closing fence (or run off the end)
            blocks.push(Block::Code(lang, body));
        } else if is_table_header(&lines, i) {
            flush_prose(&mut prose, &mut blocks);
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].contains('|') {
                if !is_separator_row(lines[i]) {
                    rows.push(split_row(lines[i]));
                }
                i += 1;
            }
            blocks.push(Block::Table(rows));
        } else {
            if !prose.is_empty() {
                prose.push('\n');
            }
            prose.push_str(line);
            i += 1;
        }
    }
    flush_prose(&mut prose, &mut blocks);
    blocks
}

fn flush_prose(prose: &mut String, blocks: &mut Vec<Block>) {
    if prose.trim().is_empty() {
        prose.clear();
    } else {
        blocks.push(Block::Markdown(std::mem::take(prose)));
    }
}

/// The language of a fence line (`` ```rust `` → `"rust"`), or `None` if the
/// line is not a fence.
fn fence_lang(line: &str) -> Option<String> {
    let t = line.trim_start();
    t.strip_prefix("```").map(|rest| rest.trim().to_string())
}

/// A table starts where a `|`-bearing row is followed by a separator row.
fn is_table_header(lines: &[&str], i: usize) -> bool {
    lines.get(i).is_some_and(|l| l.contains('|'))
        && lines.get(i + 1).is_some_and(|l| is_separator_row(l))
}

/// A separator row is only pipes, dashes, colons and spaces, with a dash.
fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Split a table row into trimmed cells, dropping the outer pipes.
fn split_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Draw a fenced code block as a shaded, padded box with an optional tag.
fn render_code(lang: &str, lines: &[String], width: usize, out: &mut Vec<Line<'static>>) {
    let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(0);
    let box_w = (content_w + 2).min(width.max(2));
    if !lang.is_empty() {
        out.push(Line::from(Span::styled(
            format!(" {lang} "),
            Style::default()
                .fg(Color::Black)
                .bg(BORDER)
                .add_modifier(Modifier::BOLD),
        )));
    }
    for line in lines {
        out.push(Line::from(Span::styled(
            pad(&format!(" {line}"), box_w),
            Style::default().fg(CODE_FG).bg(CODE_BG),
        )));
    }
}

/// Draw a pipe table with aligned, box-drawn borders.
fn render_table(rows: &[Vec<String>], out: &mut Vec<Line<'static>>) {
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (j, cell) in row.iter().enumerate() {
            widths[j] = widths[j].max(cell.width());
        }
    }
    let border = |left: &str, mid: &str, right: &str| {
        let segs: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        Line::from(Span::styled(
            format!("{left}{}{right}", segs.join(mid)),
            Style::default().fg(BORDER),
        ))
    };
    out.push(border("┌", "┬", "┐"));
    for (r, row) in rows.iter().enumerate() {
        let mut spans = vec![Span::styled("│", Style::default().fg(BORDER))];
        for (j, w) in widths.iter().enumerate() {
            let cell = row.get(j).map_or("", String::as_str);
            let style = if r == 0 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            spans.push(Span::styled(format!(" {} ", pad(cell, *w)), style));
            spans.push(Span::styled("│", Style::default().fg(BORDER)));
        }
        out.push(Line::from(spans));
        if r == 0 {
            out.push(border("├", "┼", "┤"));
        }
    }
    out.push(border("└", "┴", "┘"));
}

/// Pad `s` with spaces to `width` display columns (no truncation).
fn pad(s: &str, width: usize) -> String {
    let d = s.width();
    if d >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - d))
    }
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

    fn joined(lines: &[ratatui::text::Line]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn table_gets_box_borders() {
        let md = "| A | B |\n| --- | --- |\n| 1 | 2 |";
        let text = joined(&render(md, 80));
        assert!(text.contains('┌') && text.contains('┼') && text.contains('└'));
        assert!(text.contains('A') && text.contains('2'));
    }

    #[test]
    fn code_block_is_shaded() {
        let md = "```rust\nlet x = 1;\n```";
        let lines = render(md, 80);
        // The language tag plus at least one shaded code line.
        let shaded = lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.style.bg == Some(super::CODE_BG));
        assert!(shaded, "expected a shaded code line");
    }
}
