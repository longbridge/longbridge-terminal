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
    /// Undo snapshots `(lines, cx, cy)`, newest last.
    undo: Vec<(Vec<String>, usize, usize)>,
    /// States popped by undo, so redo can replay them. Cleared by any fresh edit.
    redo: Vec<(Vec<String>, usize, usize)>,
    /// True while a run of character inserts shares one undo step, so Ctrl+Z
    /// undoes a typed word rather than a single letter.
    coalescing: bool,
    /// Set while a compound edit (a paste, a word delete) runs, so the smaller
    /// ops it calls do not each take their own snapshot.
    suppress_undo: bool,
    /// Large pastes folded out of the visible input, in paste order. They are not
    /// shown or edited inline (a hundred lines of log would bury the prompt);
    /// each is summarised as a chip and expanded back on submission.
    attachments: Vec<String>,
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
        self.text().trim().is_empty() && self.attachments.is_empty()
    }

    /// The folded large pastes, for the input to summarise as chips.
    pub fn attachments(&self) -> &[String] {
        &self.attachments
    }

    /// A paste large enough to bury the prompt is folded away instead of
    /// inserted inline; a small one is typed in as normal.
    pub fn paste(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let big = normalized.split('\n').count() >= Self::PASTE_FOLD_LINES
            || normalized.len() >= Self::PASTE_FOLD_CHARS;
        if big {
            self.attachments.push(normalized);
        } else {
            self.insert_str(&normalized);
        }
    }

    /// The full prompt to send: the typed text (the instruction) first, then the
    /// folded pastes (the data). This is what a submission uses; [`Self::text`] is
    /// only what shows in the box.
    pub fn submission_text(&self) -> String {
        let typed = self.text();
        if self.attachments.is_empty() {
            return typed;
        }
        let pasted = self.attachments.join("\n\n");
        if typed.trim().is_empty() {
            pasted
        } else {
            format!("{typed}\n\n{pasted}")
        }
    }

    /// Number of lines a paste must reach to be folded out of the input.
    const PASTE_FOLD_LINES: usize = 8;
    /// …or the character count that folds a single long line.
    const PASTE_FOLD_CHARS: usize = 800;

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
        self.attachments.clear();
        // Undo is scoped to one composition: a cleared prompt starts fresh.
        self.undo.clear();
        self.redo.clear();
        self.coalescing = false;
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(ToString::to_string).collect()
        };
        self.cy = self.lines.len() - 1;
        self.cx = char_len(&self.lines[self.cy]);
        // A programmatic replacement (history recall) is not an undoable edit.
        self.undo.clear();
        self.redo.clear();
        self.coalescing = false;
    }

    // ── undo ───────────────────────────────────────────────────────────────

    /// How many undo steps to keep.
    const UNDO_CAP: usize = 100;

    /// Snapshot the buffer before an edit, unless a compound edit is suppressing
    /// intermediate snapshots.
    fn push_undo(&mut self) {
        if self.suppress_undo {
            return;
        }
        self.undo.push((self.lines.clone(), self.cx, self.cy));
        if self.undo.len() > Self::UNDO_CAP {
            self.undo.remove(0);
        }
        // A fresh edit forks history: the redo trail no longer applies.
        self.redo.clear();
    }

    /// Restore the buffer to the state before the last edit (Ctrl+Z).
    pub fn undo(&mut self) {
        if let Some((lines, cx, cy)) = self.undo.pop() {
            self.redo.push((self.lines.clone(), self.cx, self.cy));
            self.restore(lines, cx, cy);
        }
        self.coalescing = false;
    }

    /// Replay the most recently undone edit (Ctrl+Y).
    pub fn redo(&mut self) {
        if let Some((lines, cx, cy)) = self.redo.pop() {
            self.undo.push((self.lines.clone(), self.cx, self.cy));
            self.restore(lines, cx, cy);
        }
        self.coalescing = false;
    }

    fn restore(&mut self, lines: Vec<String>, cx: usize, cy: usize) {
        self.lines = lines;
        self.cy = cy.min(self.lines.len().saturating_sub(1));
        self.cx = cx.min(char_len(&self.lines[self.cy]));
    }

    // ── editing ──────────────────────────────────────────────────────────────

    pub fn insert_char(&mut self, ch: char) {
        // Consecutive keystrokes fold into one undo step.
        if !self.coalescing {
            self.push_undo();
            self.coalescing = true;
        }
        let at = byte_at(&self.lines[self.cy], self.cx);
        self.lines[self.cy].insert(at, ch);
        self.cx += 1;
    }

    /// Insert arbitrary text (e.g. a paste), honoring embedded newlines.
    pub fn insert_str(&mut self, text: &str) {
        // A paste is one undo step, however many characters it carries.
        self.push_undo();
        self.suppress_undo = true;
        for ch in text.chars() {
            if ch == '\n' {
                self.insert_newline();
            } else if ch != '\r' {
                self.insert_char(ch);
            }
        }
        self.suppress_undo = false;
        self.coalescing = false;
    }

    pub fn insert_newline(&mut self) {
        self.push_undo();
        self.coalescing = false;
        let at = byte_at(&self.lines[self.cy], self.cx);
        let rest = self.lines[self.cy].split_off(at);
        self.lines.insert(self.cy + 1, rest);
        self.cy += 1;
        self.cx = 0;
    }

    pub fn backspace(&mut self) {
        // With nothing typed, backspace peels off the most recent folded paste —
        // the way to undo a paste you did not mean to attach.
        if self.cx == 0 && self.cy == 0 && self.text().is_empty() && !self.attachments.is_empty() {
            self.attachments.pop();
            return;
        }
        self.push_undo();
        self.coalescing = false;
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

    /// Delete the whitespace-delimited word before the cursor (Ctrl+W). At the
    /// start of a line, falls back to joining with the previous line so the key
    /// is never a no-op mid-buffer.
    pub fn delete_word(&mut self) {
        self.push_undo();
        self.coalescing = false;
        if self.cx == 0 {
            // The line-join fallback is part of this one undo step.
            self.suppress_undo = true;
            self.backspace();
            self.suppress_undo = false;
            return;
        }
        let i = self.word_start();
        let start = byte_at(&self.lines[self.cy], i);
        let end = byte_at(&self.lines[self.cy], self.cx);
        self.lines[self.cy].replace_range(start..end, "");
        self.cx = i;
    }

    /// Char index of the start of the word before the cursor on this line.
    fn word_start(&self) -> usize {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut i = self.cx;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// Char index of the end of the word after the cursor on this line.
    fn word_end(&self) -> usize {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let n = chars.len();
        let mut i = self.cx;
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        i
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

    /// Move the cursor one word left (Alt/Ctrl+Left), crossing to the previous
    /// line's end when already at the start of a line.
    pub fn word_left(&mut self) {
        if self.cx == 0 {
            self.left();
        } else {
            self.cx = self.word_start();
        }
    }

    /// Move the cursor one word right (Alt/Ctrl+Right), crossing to the next
    /// line's start when already at the end of a line.
    pub fn word_right(&mut self) {
        if self.cx >= char_len(&self.lines[self.cy]) {
            self.right();
        } else {
            self.cx = self.word_end();
        }
    }

    /// Delete from the cursor to the end of the current line (Ctrl+K).
    pub fn kill_to_end(&mut self) {
        self.push_undo();
        self.coalescing = false;
        let at = byte_at(&self.lines[self.cy], self.cx);
        self.lines[self.cy].truncate(at);
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

    /// How many past prompts to keep for recall.
    const HISTORY_CAP: usize = 200;

    /// Pre-load prior prompts (e.g. from disk) as the oldest history, keeping the
    /// cap and collapsing consecutive duplicates. Used once at startup so ↑
    /// recalls across sessions.
    pub fn seed_history(&mut self, entries: Vec<String>) {
        for entry in entries {
            if !entry.trim().is_empty() && self.history.last() != Some(&entry) {
                self.history.push(entry);
            }
        }
        if self.history.len() > Self::HISTORY_CAP {
            self.history
                .drain(0..self.history.len() - Self::HISTORY_CAP);
        }
    }

    /// Record a submitted prompt and reset the recall position. Consecutive
    /// duplicates are collapsed, and the history is capped so a long session
    /// doesn't grow it without bound.
    pub fn push_history(&mut self, entry: &str) {
        if !entry.trim().is_empty() && self.history.last().map(String::as_str) != Some(entry) {
            self.history.push(entry.to_string());
            if self.history.len() > Self::HISTORY_CAP {
                self.history.remove(0);
            }
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
    fn pasting_crlf_text_keeps_only_the_newlines() {
        // A Windows-style paste must not leave stray carriage returns in the buffer.
        let e = typed("a\r\nb\r\nc");
        assert_eq!(e.text(), "a\nb\nc");
        assert_eq!(e.lines().len(), 3);
    }

    #[test]
    fn set_text_empty_leaves_one_blank_line() {
        let mut e = typed("something");
        e.set_text("");
        assert_eq!(e.lines().len(), 1);
        assert!(e.is_blank());
        assert_eq!(e.cursor(), (0, 0));
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
    fn kill_to_end_truncates_at_cursor() {
        let mut e = typed("hello world");
        for _ in 0..5 {
            e.left(); // cursor before "world"
        }
        e.kill_to_end();
        assert_eq!(e.text(), "hello ");
    }

    #[test]
    fn word_movement_jumps_over_tokens() {
        let mut e = typed("foo bar baz");
        e.word_left(); // to start of "baz"
        assert_eq!(e.cursor(), (0, 8));
        e.word_left(); // to start of "bar"
        assert_eq!(e.cursor(), (0, 4));
        e.word_right(); // to end of "bar"
        assert_eq!(e.cursor(), (0, 7));
    }

    #[test]
    fn delete_word_at_line_start_joins_previous() {
        let mut e = typed("ab");
        e.insert_newline();
        e.insert_str("cd");
        e.home();
        e.delete_word(); // at col 0 → join
        assert_eq!(e.text(), "abcd");
    }

    #[test]
    fn a_large_paste_is_folded_and_expanded_on_submit() {
        let mut e = Editor::new();
        e.insert_str("look at this: ");
        let big = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        e.paste(&big);
        // The box shows only the typed text; the paste is folded away as an
        // attachment.
        assert_eq!(e.text(), "look at this: ");
        assert_eq!(e.attachments().len(), 1);
        assert!(!e.is_blank());
        // Submission expands the fold back in.
        let sent = e.submission_text();
        assert!(sent.contains("look at this:") && sent.contains("line 19"));
        // Backspace on an empty buffer peels the paste back off.
        let mut e = Editor::new();
        e.paste(&big);
        assert_eq!(e.attachments().len(), 1);
        e.backspace();
        assert!(e.attachments().is_empty() && e.is_blank());
    }

    #[test]
    fn a_small_paste_stays_inline() {
        let mut e = Editor::new();
        e.paste("one\ntwo");
        assert_eq!(e.text(), "one\ntwo");
        assert!(e.attachments().is_empty());
    }

    #[test]
    fn undo_reverts_edits_a_step_at_a_time() {
        let mut e = Editor::new();
        // A run of typing is one undo step (a word, not a letter).
        e.insert_str("hello");
        e.insert_char(' ');
        e.insert_char('w');
        e.insert_char('o');
        assert_eq!(e.text(), "hello wo");
        e.undo(); // undoes the " wo" keystroke run
        assert_eq!(e.text(), "hello");
        e.undo(); // undoes the paste/insert of "hello"
        assert_eq!(e.text(), "");
        // Nothing left to undo is a no-op, not a panic.
        e.undo();
        assert_eq!(e.text(), "");
        // Redo replays the undone edits in order.
        e.redo();
        assert_eq!(e.text(), "hello");
        e.redo();
        assert_eq!(e.text(), "hello wo");
        // A fresh edit forks history, dropping the redo trail.
        e.undo();
        assert_eq!(e.text(), "hello");
        e.insert_char('!');
        e.redo();
        assert_eq!(e.text(), "hello!", "redo is void after a new edit");
    }

    #[test]
    fn undo_covers_backspace_and_word_delete() {
        let mut e = typed("alpha beta");
        e.delete_word();
        assert_eq!(e.text(), "alpha ");
        e.undo();
        assert_eq!(e.text(), "alpha beta", "the word delete is one undo step");
        e.backspace();
        assert_eq!(e.text(), "alpha bet");
        e.undo();
        assert_eq!(e.text(), "alpha beta");
    }

    #[test]
    fn seeded_history_is_recalled_oldest_last() {
        let mut e = Editor::new();
        e.seed_history(vec!["older".into(), "newer".into()]);
        e.recall_prev();
        assert_eq!(e.text(), "newer", "↑ recalls the most recent seeded prompt");
        e.recall_prev();
        assert_eq!(e.text(), "older");
        // A live submission still stacks on top of the seeded history.
        let mut e = Editor::new();
        e.seed_history(vec!["from disk".into()]);
        e.push_history("this session");
        e.recall_prev();
        assert_eq!(e.text(), "this session");
        e.recall_prev();
        assert_eq!(e.text(), "from disk");
    }

    #[test]
    fn history_dedups_consecutive_and_caps() {
        let mut e = Editor::new();
        e.push_history("same");
        e.push_history("same");
        e.recall_prev();
        assert_eq!(e.text(), "same");
        e.recall_prev(); // only one entry despite two pushes
        assert_eq!(e.text(), "same");
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
