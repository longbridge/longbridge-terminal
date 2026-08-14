//! Markdown rendering for assistant answers.
//!
//! Longbridge AI replies in Markdown (headings, bold, lists, code fences,
//! tables), plus two things of its own: ```` ```vis-chart ```` specs and `$$`
//! display math for finance formulas.
//!
//! Prose is handed to `tui-markdown` (owned here so it can outlive the source
//! string, and re-wrapped to width preserving per-span styles and wide CJK
//! glyphs — `tui-markdown` does not wrap). The blocks it renders flat are pulled
//! out first and drawn here instead:
//!
//! - **code** — a shaded block with an optional language tag
//! - **tables** — aligned box-drawn borders, fitted to width, with each cell run
//!   through `tui-markdown` so a cell and a sentence agree on `**x**` and `\$`
//! - **charts** — the braille plot from [`crate::cli::agent::render`], the same
//!   one `agent chat` prints, rather than the JSON that produced it
//! - **math** — LaTeX flattened to readable text in a gutter
//! - **`---`** — a full-width rule

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Shaded background for code blocks.
const CODE_BG: Color = Color::Rgb(38, 38, 38);
const CODE_FG: Color = Color::Rgb(220, 220, 220);
const BORDER: Color = Color::DarkGray;
/// Display-math text, dimmer than prose so a formula reads as set apart.
const MATH_FG: Color = Color::Rgb(180, 190, 205);

/// Fence language the agent uses for chart specs.
const CHART_LANG: &str = "vis-chart";

// Syntax-highlight token colors (language-agnostic).
const KW: Color = Color::Rgb(86, 156, 214); // keywords
const STR: Color = Color::Rgb(206, 145, 120); // string literals
const NUM: Color = Color::Rgb(181, 206, 168); // numbers
const COMMENT: Color = Color::Rgb(106, 153, 85); // comments

/// Keywords shared across the languages Longbridge AI is likely to emit
/// (Rust / Python / JS / TS / Go / shell). Matched whole-word only.
const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "class",
    "const",
    "continue",
    "def",
    "elif",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "let",
    "match",
    "mod",
    "mut",
    "new",
    "None",
    "not",
    "null",
    "or",
    "package",
    "pass",
    "print",
    "pub",
    "raise",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "switch",
    "trait",
    "true",
    "try",
    "type",
    "use",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// A top-level block of an answer.
enum Block {
    /// Prose handed to `tui-markdown`.
    Markdown(String),
    /// A fenced code block: `(language, lines)`.
    Code(String, Vec<String>),
    /// A pipe table: rows of cells, first row is the header.
    Table(Vec<Vec<String>>),
    /// A ```` ```vis-chart ```` spec, drawn with the shared chart renderer.
    Chart(Value),
    /// A `$$…$$` display-math block, kept as its source lines.
    Math(Vec<String>),
    /// A `---` thematic break.
    Rule,
}

/// Render `md` into owned, width-wrapped styled lines.
pub fn render(md: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for block in split_blocks(md) {
        // Blocks we draw ourselves carry no margin of their own, so without a
        // separator a table would butt straight against the sentence above it.
        if !out.is_empty() && !is_blank(out.last()) {
            out.push(Line::from(String::new()));
        }
        match block {
            Block::Markdown(text) => {
                let rendered = tui_markdown::from_str(&text);
                for line in &rendered.lines {
                    wrap_line(line, width, &mut out);
                }
            }
            Block::Code(lang, lines) => render_code(&lang, &lines, width, &mut out),
            Block::Table(rows) => render_table(&rows, width, &mut out),
            Block::Chart(spec) => render_chart(&spec, width, &mut out),
            Block::Math(lines) => render_math(&lines, width, &mut out),
            Block::Rule => out.push(Line::from(Span::styled(
                "─".repeat(width.max(1)),
                Style::default().fg(BORDER),
            ))),
        }
    }
    // Prose blocks end with their own trailing blank; drop it so the answer
    // does not float above the status line.
    while out.last().is_some_and(|l| is_blank(Some(l))) {
        out.pop();
    }
    out
}

/// Whether a rendered line has no visible content.
fn is_blank(line: Option<&Line<'static>>) -> bool {
    line.is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
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
            // Consume the closing fence (or run off the end).
            i += 1;
            // A chart spec is drawn as a chart; unparseable JSON stays a code
            // block so a malformed spec is still visible rather than dropped.
            match serde_json::from_str::<Value>(body.join("\n").trim()).ok() {
                Some(spec) if lang == CHART_LANG => blocks.push(Block::Chart(spec)),
                _ => blocks.push(Block::Code(lang, body)),
            }
        } else if is_math_fence(line) {
            flush_prose(&mut prose, &mut blocks);
            let mut body = Vec::new();
            // `$$ … $$` on one line, or a fence pair spanning several.
            if let Some(inner) = one_line_math(line) {
                body.push(inner);
            } else {
                i += 1;
                while i < lines.len() && !is_math_fence(lines[i]) {
                    body.push(lines[i].to_string());
                    i += 1;
                }
            }
            i += 1;
            blocks.push(Block::Math(body));
        } else if is_thematic_break(line)
            && i.checked_sub(1).is_none_or(|p| lines[p].trim().is_empty())
        {
            // A break stands alone. Directly under text, `---` is a setext
            // heading underline instead, so the preceding line must be blank.
            flush_prose(&mut prose, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
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

/// Whether `line` opens or closes a `$$` display-math block.
fn is_math_fence(line: &str) -> bool {
    line.trim().starts_with("$$")
}

/// The body of a one-line `$$ … $$`, or `None` if the `$$` only opens a block.
fn one_line_math(line: &str) -> Option<String> {
    let inner = line.trim().strip_prefix("$$")?.strip_suffix("$$")?;
    Some(inner.trim().to_string())
}

/// Whether `line` is a thematic break (`---`, `***`, `___`).
fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3
        && matches!(t.chars().next(), Some('-' | '*' | '_'))
        && t.chars().all(|c| c == t.chars().next().unwrap_or(' '))
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
        let cells = highlight(&format!(" {line}"));
        let mut spans = coalesce(&cells).spans;
        // Extend the shaded background to the full block width.
        let used: usize = cells
            .iter()
            .map(|(c, _)| UnicodeWidthChar::width(*c).unwrap_or(0))
            .sum();
        if used < box_w {
            spans.push(Span::styled(
                " ".repeat(box_w - used),
                Style::default().bg(CODE_BG),
            ));
        }
        out.push(Line::from(spans));
    }
}

/// Classify each char of a code line into a foreground color (over `CODE_BG`),
/// a small language-agnostic highlighter: comments, strings, numbers, keywords.
fn highlight(line: &str) -> Vec<(char, Style)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut color = vec![CODE_FG; n];
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/')
            || c == '#' && chars.get(i + 1).is_none_or(|n| n.is_whitespace())
        {
            color[i..].fill(COMMENT);
            break;
        }
        if matches!(c, '"' | '\'' | '`') {
            if let Some(end) = close_quote(&chars, i) {
                color[i..=end].fill(STR);
                i = end + 1;
                continue;
            }
        }
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < n && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '.' | '_')) {
                j += 1;
            }
            color[i..j].fill(NUM);
            i = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = i + 1;
            while j < n && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if KEYWORDS.contains(&word.as_str()) {
                color[i..j].fill(KW);
            }
            i = j;
            continue;
        }
        i += 1;
    }
    chars
        .into_iter()
        .zip(color)
        .map(|(ch, fg)| (ch, Style::default().fg(fg).bg(CODE_BG)))
        .collect()
}

/// Index of the closing quote matching the one at `start`, honoring `\` escapes,
/// or `None` if the string is not closed on this line.
fn close_quote(chars: &[char], start: usize) -> Option<usize> {
    let quote = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c == quote => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Narrowest a column may be squeezed to before the table is left to overflow.
const MIN_COL: usize = 6;

/// Draw a pipe table with aligned, box-drawn borders, fitted to `width`.
///
/// Cells are inline Markdown — the answers are full of `**bold**` labels and
/// `\$` escapes — so each one is parsed with the same renderer as prose instead
/// of being printed literally. Columns that do not fit are narrowed and their
/// cells wrapped over several physical rows, since truncating a price or a date
/// is worse than a taller table.
fn render_table(rows: &[Vec<String>], width: usize, out: &mut Vec<Line<'static>>) {
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    // Parse every cell up front: widths must be measured on rendered text, not
    // on the source (`**最新价**` is 3 glyphs wide, not 7).
    let cells: Vec<Vec<Vec<(char, Style)>>> = rows
        .iter()
        .map(|row| {
            (0..cols)
                .map(|j| inline_chars(row.get(j).map_or("", String::as_str)))
                .collect()
        })
        .collect();

    let widths = fit_columns(&cells, cols, width);
    let border = |left: &str, mid: &str, right: &str| {
        let segs: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        Line::from(Span::styled(
            format!("{left}{}{right}", segs.join(mid)),
            Style::default().fg(BORDER),
        ))
    };
    let bar = || Span::styled("│", Style::default().fg(BORDER));

    out.push(border("┌", "┬", "┐"));
    for (r, row) in cells.iter().enumerate() {
        // Wrap each cell to its column, then emit as many physical lines as the
        // tallest cell in the row needs.
        let wrapped: Vec<Vec<Vec<(char, Style)>>> = row
            .iter()
            .zip(&widths)
            .map(|(cell, w)| wrap_chars(cell, *w))
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for line_idx in 0..height {
            let mut spans = vec![bar()];
            for (j, w) in widths.iter().enumerate() {
                let empty = Vec::new();
                let part = wrapped
                    .get(j)
                    .and_then(|c| c.get(line_idx))
                    .unwrap_or(&empty);
                spans.push(Span::raw(" "));
                let used: usize = part
                    .iter()
                    .map(|(c, _)| UnicodeWidthChar::width(*c).unwrap_or(0))
                    .sum();
                // A header cell is bold even where the source did not say so.
                let head = r == 0;
                for span in coalesce(part).spans {
                    spans.push(if head {
                        let style = span.style.add_modifier(Modifier::BOLD);
                        Span::styled(span.content, style)
                    } else {
                        span
                    });
                }
                spans.push(Span::raw(format!(
                    "{} ",
                    " ".repeat(w.saturating_sub(used))
                )));
                spans.push(bar());
            }
            out.push(Line::from(spans));
        }
        if r == 0 {
            out.push(border("├", "┼", "┤"));
        }
    }
    out.push(border("└", "┴", "┘"));
}

/// Column widths that fit `width`, shrinking the widest column repeatedly until
/// the table fits or every column has hit [`MIN_COL`].
fn fit_columns(cells: &[Vec<Vec<(char, Style)>>], cols: usize, width: usize) -> Vec<usize> {
    let mut widths = vec![0usize; cols];
    for row in cells {
        for (j, cell) in row.iter().enumerate() {
            let w: usize = cell
                .iter()
                .map(|(c, _)| UnicodeWidthChar::width(*c).unwrap_or(0))
                .sum();
            widths[j] = widths[j].max(w);
        }
    }
    // Frame cost: one bar per column plus a closing bar, and two pad columns each.
    let frame = 3 * cols + 1;
    let budget = width.saturating_sub(frame);
    while widths.iter().sum::<usize>() > budget {
        let Some(j) = widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > MIN_COL)
            .max_by_key(|(_, w)| **w)
            .map(|(j, _)| j)
        else {
            break; // nothing left to give: let it overflow rather than truncate
        };
        widths[j] -= 1;
    }
    widths
}

/// Parse inline Markdown into styled chars, using the same engine as prose so a
/// table cell and a sentence agree on what `**x**` and `\$` mean.
fn inline_chars(text: &str) -> Vec<(char, Style)> {
    tui_markdown::from_str(text)
        .lines
        .iter()
        .flat_map(|l| {
            l.spans
                .iter()
                .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Split styled chars into runs no wider than `width` display columns.
fn wrap_chars(chars: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
    let mut out = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut w = 0;
    for &(ch, style) in chars {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width > 0 && w + cw > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push((ch, style));
        w += cw;
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

/// Draw a `vis-chart` spec with the same renderer `agent chat` uses, so the TUI
/// shows the braille plot rather than the JSON that produced it. The renderer
/// emits ANSI, which is converted back into styled lines.
fn render_chart(spec: &Value, width: usize, out: &mut Vec<Line<'static>>) {
    let text = crate::cli::agent::render::render_vis_chart(spec, width, true);
    match ansi_to_tui::IntoText::into_text(&text) {
        Ok(parsed) => {
            for line in &parsed.lines {
                wrap_line(line, width, out);
            }
        }
        // Fall back to the uncolored form rather than losing the chart.
        Err(_) => {
            for line in crate::cli::agent::render::render_vis_chart(spec, width, false).lines() {
                out.push(Line::from(line.to_string()));
            }
        }
    }
}

/// LaTeX fragments the agent's finance formulas actually use, mapped to plain
/// text. A terminal cannot typeset math, but the alternative is a wall of
/// backslashes, and these few rules cover the `\text{…} \times \max` style the
/// answers are written in.
const LATEX: &[(&str, &str)] = &[
    ("\\begin{array}{l}", ""),
    ("\\begin{array}{c}", ""),
    ("\\begin{array}{r}", ""),
    ("\\end{array}", ""),
    // `\left.` / `\right.` mean "no delimiter on this side", so the trailing
    // dot goes with them. Longest match first: plain `\left` follows.
    ("\\left.", ""),
    ("\\right.", ""),
    ("\\left", ""),
    ("\\right", ""),
    ("\\quad", " "),
    ("\\,", " "),
    ("\\;", " "),
    ("\\times", "×"),
    ("\\cdot", "·"),
    ("\\div", "÷"),
    ("\\pm", "±"),
    ("\\approx", "≈"),
    ("\\neq", "≠"),
    ("\\geq", "≥"),
    ("\\ge", "≥"),
    ("\\leq", "≤"),
    ("\\le", "≤"),
    ("\\max", "max"),
    ("\\min", "min"),
    ("\\sum", "Σ"),
    ("\\Delta", "Δ"),
    ("\\sigma", "σ"),
    ("\\%", "%"),
    ("\\{", "{"),
    ("\\}", "}"),
];

/// Flatten one line of LaTeX into readable text.
fn latex_to_text(src: &str) -> String {
    let mut s = src.to_string();
    // `\text{…}` and `\mathrm{…}` are pure wrappers: keep only their contents.
    for wrapper in ["\\text", "\\mathrm", "\\mathbf", "\\operatorname"] {
        while let Some(at) = s.find(&format!("{wrapper}{{")) {
            let open = at + wrapper.len();
            let Some(close) = s[open..].find('}').map(|p| open + p) else {
                break;
            };
            let inner = s[open + 1..close].trim().to_string();
            s.replace_range(at..=close, &inner);
        }
    }
    for (from, to) in LATEX {
        s = s.replace(from, to);
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Draw a display-math block: each source row on its own line, marked with a
/// gutter bar so it reads as a set-apart formula.
fn render_math(lines: &[String], width: usize, out: &mut Vec<Line<'static>>) {
    let style = Style::default().fg(MATH_FG);
    // `\\` is LaTeX's row break; splitting on it keeps a multi-row `array`
    // readable instead of collapsing the whole formula onto one line.
    for row in lines.join("\n").split("\\\\") {
        let text = latex_to_text(row);
        if text.is_empty() {
            continue;
        }
        let mut wrapped = Vec::new();
        wrap_line(
            &Line::from(Span::styled(text, style)),
            width.saturating_sub(4).max(1),
            &mut wrapped,
        );
        for line in wrapped {
            let mut spans = vec![Span::styled("  │ ", Style::default().fg(BORDER))];
            spans.extend(line.spans);
            out.push(Line::from(spans));
        }
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

    #[test]
    fn code_keywords_and_strings_are_colored() {
        let md = "```rust\nlet s = \"hi\";\n```";
        let spans: Vec<_> = render(md, 80).into_iter().flat_map(|l| l.spans).collect();
        let kw = spans
            .iter()
            .find(|s| s.content == "let")
            .expect("keyword span");
        assert_eq!(kw.style.fg, Some(super::KW));
        let string = spans
            .iter()
            .find(|s| s.content.contains("\"hi\""))
            .expect("string span");
        assert_eq!(string.style.fg, Some(super::STR));
    }

    /// Cells are inline Markdown: `**bold**` must bold rather than print its
    /// asterisks, and `\$` must yield a bare `$`. Table and prose used to
    /// disagree here, which is visible in every answer the agent writes.
    #[test]
    fn table_cells_parse_inline_markdown() {
        let md = "| 指标 | 数值 |\n|---|---|\n| **最新价** | \\$135.995 |";
        let text: String = render(md, 80)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(!text.contains("**"), "asterisks must not survive: {text}");
        assert!(!text.contains("\\$"), "escape must not survive: {text}");
        assert!(text.contains("$135.995"), "value must survive: {text}");
        let bold = render(md, 80)
            .iter()
            .flat_map(|l| l.spans.clone())
            .any(|s| {
                s.content.contains("最新价")
                    && s.style
                        .add_modifier
                        .contains(ratatui::style::Modifier::BOLD)
            });
        assert!(bold, "a bold cell must render bold");
    }

    /// A wide table is squeezed and wrapped to fit, never left to overflow and
    /// tear its own borders apart on the terminal's own wrap.
    #[test]
    fn wide_table_is_fitted_to_width() {
        let md = "| a | b | c |\n|---|---|---|\n|                   一二三四五六七八九十 | 一二三四五六七八九十 | 一二三四五六七八九十 |";
        for width in [40usize, 60, 100] {
            for line in render(md, width) {
                let w: usize = line
                    .spans
                    .iter()
                    .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                assert!(w <= width, "line of {w} cols exceeds width {width}");
            }
        }
    }

    #[test]
    fn thematic_break_becomes_a_rule() {
        let lines = render("above\n\n---\n\nbelow", 20);
        assert!(
            lines.iter().any(|l| {
                l.spans
                    .iter()
                    .any(|s| s.content.starts_with('─') && s.content.chars().count() == 20)
            }),
            "`---` should draw a full-width rule"
        );
    }

    /// A chart spec is drawn, not dumped: the JSON keys must not reach the
    /// screen, and the series values should.
    #[test]
    fn chart_spec_is_drawn_not_printed() {
        let md = "```vis-chart\n{\"type\":\"line\",\"title\":\"T\",                  \"data\":[{\"time\":\"a\",\"value\":1.0},{\"time\":\"b\",\"value\":2.0}]}\n```";
        let text: String = render(md, 60)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            !text.contains("\"type\""),
            "raw JSON must not render: {text}"
        );
        assert!(text.contains('T'), "the chart title should render: {text}");
    }

    /// An unparseable spec keeps its fence as a code block: a broken chart must
    /// still be visible rather than silently dropped.
    #[test]
    fn malformed_chart_falls_back_to_code() {
        let text: String = render("```vis-chart\nnot json\n```", 60)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("not json"), "content must survive: {text}");
    }

    #[test]
    fn display_math_is_flattened_to_readable_text() {
        let md = "$$\n\\text{ 保证金 } = \\max\\left\\{ 20\\% \\times A \\right.\n$$";
        let text: String = render(md, 80)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("保证金"), "text wrapper unwrapped: {text}");
        assert!(text.contains('×'), "\\times becomes ×: {text}");
        assert!(!text.contains("\\max"), "commands are resolved: {text}");
        assert!(!text.contains("\\right"), "delimiters are resolved: {text}");
    }

    /// `\\` is LaTeX's row break, so a multi-row array stays multi-row.
    #[test]
    fn display_math_keeps_array_rows_apart() {
        let md = "$$\n\\begin{array}{l} A \\\\ B \\end{array}\n$$";
        let rows = render(md, 80).len();
        assert!(
            rows >= 2,
            "two array rows should render as two lines, got {rows}"
        );
    }

    /// A table must not butt straight against the sentence above it.
    #[test]
    fn blocks_are_separated_by_a_blank_line() {
        let lines = render("intro text\n| a |\n|---|\n| 1 |", 40);
        let blank = lines
            .iter()
            .position(|l| l.spans.iter().all(|s| s.content.trim().is_empty()));
        let border = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content.starts_with('┌')));
        assert!(
            matches!((blank, border), (Some(b), Some(t)) if b < t),
            "expected a blank line before the table"
        );
    }
}
