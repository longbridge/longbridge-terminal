// SEC filing HTML to Markdown converter.
//
// Handles inline XBRL tags (ix:nonNumeric, ix:continuation, etc.) as transparent
// wrappers, strips hidden/metadata sections, and converts tables to Markdown format.

use std::fmt::Write as _;

use scraper::{ElementRef, Html, Node};

/// Convert SEC filing HTML to clean Markdown text.
pub fn convert(html: &str) -> String {
    let doc = Html::parse_document(html);
    let mut conv = Converter::new();

    let selector = scraper::Selector::parse("body").expect("valid selector");
    if let Some(body) = doc.select(&selector).next() {
        walk(body, &mut conv);
    } else {
        walk(doc.root_element(), &mut conv);
    }

    conv.finish()
}

// ---------------------------------------------------------------------------
// Converter state
// ---------------------------------------------------------------------------

struct Converter {
    output: String,
    current_line: String,
    /// Blank lines to inject before the next non-empty block.
    blank_lines_pending: u8,
}

impl Converter {
    fn new() -> Self {
        Self {
            output: String::new(),
            current_line: String::new(),
            blank_lines_pending: 0,
        }
    }

    /// Push inline text onto the current line.  Inserts a space when needed,
    /// but never before punctuation characters that naturally follow the
    /// previous token without a space.
    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let needs_space = !self.current_line.is_empty()
            && !self.current_line.ends_with(' ')
            && !text.starts_with(' ')
            && !matches!(
                text.chars().next(),
                Some(',' | '.' | ')' | ']' | '!' | '?' | ';' | ':' | '%')
            );
        if needs_space {
            self.current_line.push(' ');
        }
        self.current_line.push_str(text);
    }

    /// Flush the current line to `output`.
    fn flush_line(&mut self) {
        let line = self.current_line.trim().to_string();
        self.current_line.clear();
        if line.is_empty() {
            return;
        }
        if !self.output.is_empty() {
            let newlines = self.blank_lines_pending.max(1);
            for _ in 0..newlines {
                self.output.push('\n');
            }
        }
        self.blank_lines_pending = 0;
        self.output.push_str(&line);
    }

    fn begin_block(&mut self) {
        self.flush_line();
        // Two newlines = one blank line, which is required for separate
        // paragraphs in Markdown.
        if !self.output.is_empty() && self.blank_lines_pending < 2 {
            self.blank_lines_pending = 2;
        }
    }

    fn end_block(&mut self) {
        self.flush_line();
        if !self.output.is_empty() && self.blank_lines_pending < 2 {
            self.blank_lines_pending = 2;
        }
    }

    fn finish(mut self) -> String {
        self.flush_line();
        // Collapse runs of more than two newlines and trim trailing whitespace.
        let mut result = String::with_capacity(self.output.len());
        let mut newline_run = 0u8;
        for ch in self.output.chars() {
            if ch == '\n' {
                newline_run += 1;
                if newline_run <= 2 {
                    result.push(ch);
                }
            } else {
                newline_run = 0;
                result.push(ch);
            }
        }
        result.trim_end().to_string()
    }
}

// ---------------------------------------------------------------------------
// DOM walker
// ---------------------------------------------------------------------------

fn walk(elem: ElementRef<'_>, conv: &mut Converter) {
    let el = elem.value();
    let tag = el.name();

    if is_hidden(el) || is_skip_tag(tag) {
        return;
    }

    if has_page_break_before(el) {
        conv.flush_line();
        if !conv.output.is_empty() {
            conv.output.push_str("\n\n---\n");
        }
        conv.blank_lines_pending = 1;
    }

    match tag {
        // XBRL metadata containers: skip entire subtree.
        t if is_xbrl_metadata(t) => {}

        // XBRL inline wrappers: transparent, process children.
        t if is_xbrl_inline(t) => walk_children(elem, conv),

        // Headings.
        "h1" => heading(elem, conv, "# "),
        "h2" => heading(elem, conv, "## "),
        "h3" => heading(elem, conv, "### "),
        "h4" | "h5" | "h6" => heading(elem, conv, "#### "),

        // Block containers.
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside" | "nav"
        | "blockquote" | "pre" | "address" | "figure" | "figcaption" | "details" | "summary" => {
            conv.begin_block();
            walk_children(elem, conv);
            conv.end_block();
        }

        "br" => conv.flush_line(),

        "hr" => {
            conv.begin_block();
            conv.push_text("---");
            conv.end_block();
        }

        "table" => {
            let html = table_to_html(elem);
            if !html.is_empty() {
                conv.begin_block();
                conv.push_text(&html);
                conv.end_block();
            }
        }

        "ul" => {
            conv.begin_block();
            for li in iter_li(elem) {
                let text = collect_text(li);
                let text = text
                    .trim_start_matches(|c: char| "•·∙◦▪▫".contains(c))
                    .trim();
                if !text.is_empty() {
                    conv.push_text(&format!("- {text}"));
                    conv.flush_line();
                }
            }
            conv.end_block();
        }

        "ol" => {
            conv.begin_block();
            for (i, li) in iter_li(elem).into_iter().enumerate() {
                let text = collect_text(li);
                let text = text.trim();
                if !text.is_empty() {
                    conv.push_text(&format!("{}. {text}", i + 1));
                    conv.flush_line();
                }
            }
            conv.end_block();
        }

        "li" => {
            let text = collect_text(elem);
            let text = text.trim();
            if !text.is_empty() {
                conv.begin_block();
                conv.push_text(&format!("- {text}"));
                conv.end_block();
            }
        }

        "b" | "strong" => {
            let text = collect_text(elem);
            let text = text.trim();
            if !text.is_empty() {
                conv.push_text(&format!("**{text}**"));
            }
        }

        "i" | "em" => {
            let text = collect_text(elem);
            let text = text.trim();
            if !text.is_empty() {
                conv.push_text(&format!("*{text}*"));
            }
        }

        // Inline containers: handle style-based bold/italic.
        "span" | "a" | "label" | "sup" | "sub" | "u" | "s" | "del" | "mark" | "cite" | "abbr"
        | "code" | "samp" | "kbd" | "var" | "time" | "data" => {
            let bold = is_bold_style(el);
            let italic = is_italic_style(el);
            if bold || italic {
                let text = collect_text(elem);
                let text = text.trim();
                if !text.is_empty() {
                    let wrap = match (bold, italic) {
                        (true, true) => "***",
                        (true, false) => "**",
                        (false, true) => "*",
                        _ => unreachable!(),
                    };
                    conv.push_text(&format!("{wrap}{text}{wrap}"));
                }
            } else {
                walk_children(elem, conv);
            }
        }

        _ => walk_children(elem, conv),
    }

    if has_page_break_after(el) {
        conv.flush_line();
        if !conv.output.is_empty() {
            conv.output.push_str("\n\n---\n");
        }
        conv.blank_lines_pending = 1;
    }
}

fn heading(elem: ElementRef<'_>, conv: &mut Converter, prefix: &str) {
    conv.begin_block();
    conv.push_text(prefix);
    walk_children(elem, conv);
    conv.end_block();
}

fn walk_children(elem: ElementRef<'_>, conv: &mut Converter) {
    for node in elem.children() {
        match node.value() {
            Node::Text(t) => {
                let text = normalize_text(&t.text);
                if !text.is_empty() {
                    conv.push_text(&text);
                }
            }
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(node) {
                    walk(child, conv);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Table rendering: clean HTML (style stripped, XBRL unwrapped)
// ---------------------------------------------------------------------------

/// Render a table as compact HTML: preserve all original structure (colspan,
/// rowspan, thead/tbody, td/th tags) but strip every style/class/layout
/// attribute.  Cell content is extracted as plain text (XBRL wrappers removed).
fn table_to_html(table: ElementRef<'_>) -> String {
    let mut out = String::new();
    render_table_node(table, &mut out);
    out
}

fn render_table_node(elem: ElementRef<'_>, out: &mut String) {
    let tag = elem.value().name();

    match tag {
        "table" | "thead" | "tbody" | "tfoot" | "tr" => {
            // Check if this entire element has any visible text at all.
            if tag == "table" && collect_text(elem).trim().is_empty() {
                return;
            }

            // Open tag (no attributes for table-structural elements).
            out.push('<');
            out.push_str(tag);
            out.push('>');

            for node in elem.children() {
                if let Some(child) = ElementRef::wrap(node) {
                    let ctag = child.value().name();
                    if is_hidden(child.value()) {
                        continue;
                    }
                    match ctag {
                        "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" => {
                            render_table_node(child, out);
                        }
                        _ => {} // skip non-table children of structural elements
                    }
                }
            }

            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }

        "td" | "th" => {
            // Keep only colspan and rowspan.
            let mut attrs = String::new();
            if let Some(cs) = elem.value().attr("colspan").filter(|&v| v != "1") {
                let _ = write!(attrs, " colspan=\"{cs}\"");
            }
            if let Some(rs) = elem.value().attr("rowspan").filter(|&v| v != "1") {
                let _ = write!(attrs, " rowspan=\"{rs}\"");
            }
            let text = collect_text(elem);
            let _ = write!(out, "<{tag}{attrs}>{}</{tag}>", html_escape(&text));
        }

        _ => {}
    }
}

/// Escape text for embedding inside HTML element content.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// Text collection (for tables, bold, etc.)
// ---------------------------------------------------------------------------

fn collect_text(elem: ElementRef<'_>) -> String {
    let mut parts: Vec<String> = Vec::new();
    collect_text_rec(elem, &mut parts);
    // Join parts without adding a space before punctuation characters.
    let mut result = String::new();
    for part in &parts {
        if part.is_empty() {
            continue;
        }
        let no_space_before = matches!(
            part.trim_start().chars().next(),
            Some(',' | '.' | ')' | ']' | '!' | '?' | ';' | ':' | '%')
        );
        let need_space = !result.is_empty()
            && !result.ends_with(' ')
            && !part.starts_with(' ')
            && !no_space_before;
        if need_space {
            result.push(' ');
        }
        result.push_str(part);
    }
    // Final whitespace normalization.
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collect_text_rec(elem: ElementRef<'_>, parts: &mut Vec<String>) {
    let el = elem.value();
    if is_hidden(el) || is_skip_tag(el.name()) || is_xbrl_metadata(el.name()) {
        return;
    }
    for node in elem.children() {
        match node.value() {
            Node::Text(t) => {
                let s = normalize_text(&t.text);
                if !s.is_empty() {
                    parts.push(s);
                }
            }
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(node) {
                    collect_text_rec(child, parts);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Helper predicates
// ---------------------------------------------------------------------------

fn is_hidden(el: &scraper::node::Element) -> bool {
    let style = el
        .attr("style")
        .unwrap_or("")
        .replace(' ', "")
        .to_lowercase();
    style.contains("display:none")
}

fn is_skip_tag(tag: &str) -> bool {
    matches!(
        tag,
        "head" | "style" | "script" | "link" | "meta" | "noscript" | "template"
    )
}

/// XBRL elements whose entire subtree should be discarded.
fn is_xbrl_metadata(tag: &str) -> bool {
    let local = tag.rfind(':').map_or(tag, |i| &tag[i + 1..]);
    matches!(
        local,
        "header"
            | "hidden"
            | "references"
            | "resources"
            | "schemaref"
            | "context"
            | "unit"
            | "period"
            | "entity"
            | "measure"
            | "divide"
            | "unitnumerator"
            | "unitdenominator"
            | "identifier"
            | "startdate"
            | "enddate"
            | "instant"
    ) || tag.starts_with("xbrli:")
        || tag.starts_with("link:")
        || tag.starts_with("xbrldi:")
        || tag.starts_with("xbrldt:")
}

/// XBRL inline elements that wrap visible content – treated as transparent.
fn is_xbrl_inline(tag: &str) -> bool {
    let local = tag.rfind(':').map_or(tag, |i| &tag[i + 1..]);
    matches!(
        local,
        "nonnumeric" | "nonfraction" | "continuation" | "fraction" | "numerator" | "denominator"
    ) || tag.starts_with("ixt:")
        || tag.starts_with("ixt-sec:")
}

fn is_bold_style(el: &scraper::node::Element) -> bool {
    let style = el
        .attr("style")
        .unwrap_or("")
        .replace(' ', "")
        .to_lowercase();
    style.contains("font-weight:bold") || style.contains("font-weight:700")
}

fn is_italic_style(el: &scraper::node::Element) -> bool {
    let style = el
        .attr("style")
        .unwrap_or("")
        .replace(' ', "")
        .to_lowercase();
    style.contains("font-style:italic")
}

fn has_page_break_before(el: &scraper::node::Element) -> bool {
    let style = el
        .attr("style")
        .unwrap_or("")
        .replace(' ', "")
        .to_lowercase();
    style.contains("page-break-before:always")
        || style.contains("break-before:page")
        || style.contains("break-before:always")
}

fn has_page_break_after(el: &scraper::node::Element) -> bool {
    let style = el
        .attr("style")
        .unwrap_or("")
        .replace(' ', "")
        .to_lowercase();
    style.contains("page-break-after:always")
        || style.contains("break-after:page")
        || style.contains("break-after:always")
}

fn iter_li(list: ElementRef<'_>) -> Vec<ElementRef<'_>> {
    list.children()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == "li")
        .collect()
}

fn normalize_text(s: &str) -> String {
    let s = s.replace('\u{00a0}', " ");
    let has_leading = s.starts_with(|c: char| c.is_ascii_whitespace());
    let has_trailing = s.ends_with(|c: char| c.is_ascii_whitespace());
    let normalized: String = s
        .split(|c: char| c.is_ascii_whitespace())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return String::new();
    }
    let lead = if has_leading { " " } else { "" };
    let trail = if has_trailing { " " } else { "" };
    format!("{lead}{normalized}{trail}")
}
