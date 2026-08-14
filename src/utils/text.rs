use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Terminal display width (CJK / fullwidth glyphs count as 2 columns), as
/// opposed to `chars().count()` which undercounts wide glyphs and would
/// misalign labels, bars, and boxes containing Chinese/Japanese/Korean text.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Right-pad `s` with spaces until it reaches `width` display columns.
pub fn pad_display(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - w))
    }
}

/// Truncate `s` to at most `max` display columns, marking a clip with `…`.
///
/// Measured in columns, not `chars()`: a CJK string clipped by character count
/// still overflows a width-constrained layout by up to 2×.
pub fn truncate_width(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    // One column is reserved for the ellipsis.
    let budget = max - 1;
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Strip control characters from server-originated text before it reaches
/// the terminal. C0 controls are removed except `\n` and `\t` (needed for
/// readable formatting); `ESC` (0x1b, the entry point for ANSI/OSC escape
/// sequences) and `DEL` (0x7f) are removed too. Without this, a
/// malicious/buggy answer, question, tool name, or reference field could
/// smuggle terminal escape sequences (e.g. an OSC title-bar rewrite or an
/// SGR color reset) into stdout/stderr.
pub fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || (!c.is_control()))
        .collect()
}

/// Strip HTML tags from a string, returning plain text.
pub fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}
