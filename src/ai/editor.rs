//! A small multi-line text editor for the `longbridge ai` prompt.
//!
//! grok-build ships an entire `xai-ratatui-textarea` crate; this is the
//! proportionate slice for a chat prompt: a cursor over UTF-8 lines with the
//! editing operations users expect (arrow movement, word delete, paste of
//! multi-line text) plus a recall stack for previously submitted prompts.
//! Cursor coordinates are `(line, char-index)`; rendering converts the column
//! to a display width so wide (CJK) glyphs line up.

use unicode_width::UnicodeWidthStr;

#[derive(Default)]
pub struct Editor {
    lines: Vec<String>,
    /// Char index within the current line (not a byte offset).
    cx: usize,
    /// Current line index.
    cy: usize,
    /// Previously submitted prompts, oldest first.
    history: Vec<String>,
    /// Position while browsing `history`; `None` means editing live text.
    hist: Option<usize>,
    /// Live text stashed when history browsing began, restored on exit.
    stash: Option<String>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            ..Self::default()
        }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_blank(&self) -> bool {
        self.text().trim().is_empty()
    }

    /// `true` while the buffer holds a single line — used to decide whether an
    /// up/down key should move the cursor or recall history.
    pub fn is_single_line(&self) -> bool {
        self.lines.len() == 1
    }

    /// Cursor position as `(line, display-column)` for placing the caret.
    pub fn cursor(&self) -> (usize, usize) {
        let col = char_slice(&self.lines[self.cy], 0, self.cx).width();
        (self.cy, col)
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cx = 0;
        self.cy = 0;
        self.hist = None;
        self.stash = None;
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(ToString::to_string).collect()
        };
        self.cy = self.lines.len() - 1;
        self.cx = char_len(&self.lines[self.cy]);
    }

    // ── editing ──────────────────────────────────────────────────────────────

    pub fn insert_char(&mut self, ch: char) {
        let at = byte_at(&self.lines[self.cy], self.cx);
        self.lines[self.cy].insert(at, ch);
        self.cx += 1;
    }

    /// Insert arbitrary text (e.g. a paste), honoring embedded newlines.
    pub fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                self.insert_newline();
            } else if ch != '\r' {
                self.insert_char(ch);
            }
        }
    }

    pub fn insert_newline(&mut self) {
        let at = byte_at(&self.lines[self.cy], self.cx);
        let rest = self.lines[self.cy].split_off(at);
        self.lines.insert(self.cy + 1, rest);
        self.cy += 1;
        self.cx = 0;
    }

    pub fn backspace(&mut self) {
        if self.cx > 0 {
            let start = byte_at(&self.lines[self.cy], self.cx - 1);
            let end = byte_at(&self.lines[self.cy], self.cx);
            self.lines[self.cy].replace_range(start..end, "");
            self.cx -= 1;
        } else if self.cy > 0 {
            let cur = self.lines.remove(self.cy);
            self.cy -= 1;
            self.cx = char_len(&self.lines[self.cy]);
            self.lines[self.cy].push_str(&cur);
        }
    }

    /// Delete the whitespace-delimited word before the cursor (Ctrl+W).
    pub fn delete_word(&mut self) {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut i = self.cx;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let start = byte_at(&self.lines[self.cy], i);
        let end = byte_at(&self.lines[self.cy], self.cx);
        self.lines[self.cy].replace_range(start..end, "");
        self.cx = i;
    }

    // ── movement ─────────────────────────────────────────────────────────────

    pub fn left(&mut self) {
        if self.cx > 0 {
            self.cx -= 1;
        } else if self.cy > 0 {
            self.cy -= 1;
            self.cx = char_len(&self.lines[self.cy]);
        }
    }

    pub fn right(&mut self) {
        if self.cx < char_len(&self.lines[self.cy]) {
            self.cx += 1;
        } else if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = 0;
        }
    }

    pub fn home(&mut self) {
        self.cx = 0;
    }

    pub fn end(&mut self) {
        self.cx = char_len(&self.lines[self.cy]);
    }

    /// Move up a line if possible; returns `false` if already on the first line
    /// (so the caller can fall back to history recall).
    pub fn up(&mut self) -> bool {
        if self.cy == 0 {
            return false;
        }
        self.cy -= 1;
        self.cx = self.cx.min(char_len(&self.lines[self.cy]));
        true
    }

    /// Move down a line if possible; returns `false` if already on the last
    /// line.
    pub fn down(&mut self) -> bool {
        if self.cy + 1 >= self.lines.len() {
            return false;
        }
        self.cy += 1;
        self.cx = self.cx.min(char_len(&self.lines[self.cy]));
        true
    }

    // ── history ──────────────────────────────────────────────────────────────

    /// Record a submitted prompt and reset the recall position.
    pub fn push_history(&mut self, entry: &str) {
        if !entry.trim().is_empty() {
            self.history.push(entry.to_string());
        }
        self.hist = None;
        self.stash = None;
    }

    /// Recall the previous prompt (older). Stashes live text on first step.
    pub fn recall_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let target = match self.hist {
            None => {
                self.stash = Some(self.text());
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.hist = Some(target);
        let text = self.history[target].clone();
        self.set_text(&text);
    }

    /// Recall the next prompt (newer), returning to live text past the end.
    pub fn recall_next(&mut self) {
        match self.hist {
            Some(i) if i + 1 < self.history.len() => {
                self.hist = Some(i + 1);
                let text = self.history[i + 1].clone();
                self.set_text(&text);
            }
            Some(_) => {
                self.hist = None;
                let text = self.stash.take().unwrap_or_default();
                self.set_text(&text);
            }
            None => {}
        }
    }
}

/// Byte offset of char index `ci` (clamped to the string length).
fn byte_at(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// The substring spanning char indices `[start, end)`.
fn char_slice(s: &str, start: usize, end: usize) -> String {
    s.chars().skip(start).take(end - start).collect()
}

#[cfg(test)]
mod tests {
    use super::Editor;

    fn typed(text: &str) -> Editor {
        let mut e = Editor::new();
        e.insert_str(text);
        e
    }

    #[test]
    fn insert_and_backspace_edit_mid_string() {
        let mut e = typed("abd");
        e.left(); // between b and d
        e.insert_char('c'); // abcd
        assert_eq!(e.text(), "abcd");
        e.backspace(); // abd
        assert_eq!(e.text(), "abd");
    }

    #[test]
    fn delete_word_removes_last_token() {
        let mut e = typed("hello world");
        e.delete_word();
        assert_eq!(e.text(), "hello ");
    }

    #[test]
    fn newline_splits_and_backspace_rejoins() {
        let mut e = typed("ab");
        e.insert_newline(); // "ab\n"
        e.insert_str("cd"); // "ab\ncd"
        assert_eq!(e.lines().len(), 2);
        e.home();
        e.backspace(); // join line 2 into line 1
        assert_eq!(e.text(), "abcd");
        assert_eq!(e.lines().len(), 1);
    }

    #[test]
    fn history_recall_walks_prompts() {
        let mut e = Editor::new();
        e.push_history("first");
        e.push_history("second");
        e.recall_prev();
        assert_eq!(e.text(), "second");
        e.recall_prev();
        assert_eq!(e.text(), "first");
        e.recall_next();
        assert_eq!(e.text(), "second");
    }

    #[test]
    fn cursor_column_counts_wide_glyphs() {
        let mut e = typed("你好");
        // Two full-width glyphs → cursor sits at display column 4.
        assert_eq!(e.cursor(), (0, 4));
        e.left();
        assert_eq!(e.cursor(), (0, 2));
    }
}
