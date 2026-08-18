//! Persistent prompt history for `longbridge ai`.
//!
//! Same-session recall lives in [`super::editor::Editor`]; this is what makes
//! ↑/↓ reach prompts from *previous* sessions too, the way a shell's history
//! does. It is a best-effort convenience file — every operation degrades to a
//! no-op on failure rather than interrupting the chat.
//!
//! Each line is one JSON-encoded string, so a multi-line prompt survives the
//! round trip intact instead of being split across history entries.

use std::io::Write;
use std::path::PathBuf;

/// How many prompts to keep on disk, matching the editor's in-memory cap.
const CAP: usize = 200;

/// `~/.longbridge/ai/prompt-history`.
fn path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".longbridge")
            .join("ai")
            .join("prompt-history"),
    )
}

/// The saved prompts, oldest first. A missing or unreadable file is simply no
/// history.
pub fn load() -> Vec<String> {
    let Some(p) = path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(p) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|l| serde_json::from_str::<String>(l).ok())
        .collect()
}

/// Append one prompt, collapsing a consecutive duplicate and capping the file.
/// Best-effort: any I/O failure is swallowed.
pub fn append(entry: &str) {
    if entry.trim().is_empty() {
        return;
    }
    let Some(p) = path() else {
        return;
    };
    let mut entries = load();
    // A prompt re-sent unchanged should not stack up, mirroring the editor.
    if entries.last().map(String::as_str) == Some(entry) {
        return;
    }
    entries.push(entry.to_string());
    if entries.len() > CAP {
        entries.drain(0..entries.len() - CAP);
    }
    write_all(&p, &entries);
}

/// Rewrite the whole file from `entries`. Cheap — the list is at most `CAP`.
fn write_all(p: &PathBuf, entries: &[String]) {
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out = String::new();
    for e in entries {
        if let Ok(json) = serde_json::to_string(e) {
            out.push_str(&json);
            out.push('\n');
        }
    }
    if let Ok(mut f) = std::fs::File::create(p) {
        let _ = f.write_all(out.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A multi-line prompt survives the JSON-per-line round trip rather than
    /// being torn into separate history entries.
    #[test]
    fn a_multiline_prompt_round_trips() {
        let entries = vec!["one".to_string(), "two\nlines".to_string()];
        let dir = std::env::temp_dir().join(format!("lb-hist-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("prompt-history");
        write_all(&p, &entries);
        let content = std::fs::read_to_string(&p).unwrap();
        let loaded: Vec<String> = content
            .lines()
            .filter_map(|l| serde_json::from_str::<String>(l).ok())
            .collect();
        assert_eq!(loaded, entries);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
