use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::fs;
use std::path::PathBuf;

/// Get the path to the latest log file
fn get_latest_log_file() -> Option<PathBuf> {
    let log_dir = crate::logger::default_log_dir();

    // Find all log files in the directory
    let mut log_files: Vec<PathBuf> = fs::read_dir(&log_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == "longbridge.log")
        })
        .collect();

    // Sort by modification time (newest first)
    log_files.sort_by(|a, b| {
        let time_a = fs::metadata(a).and_then(|m| m.modified()).ok();
        let time_b = fs::metadata(b).and_then(|m| m.modified()).ok();

        match (time_a, time_b) {
            (Some(ta), Some(tb)) => tb.cmp(&ta), // Reverse order (newest first)
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    log_files.into_iter().next()
}

/// Read the last N lines from the log file
fn read_last_lines(path: &PathBuf, count: usize) -> Vec<String> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<String> = content
                .lines()
                .map(std::string::ToString::to_string)
                .collect();
            let start = lines.len().saturating_sub(count);
            lines[start..].to_vec()
        }
        Err(_) => vec![],
    }
}

/// Log panel widget
pub struct LogPanel {
    lines: Vec<String>,
    visible: bool,
}

impl LogPanel {
    /// Create a new log panel
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            visible: false,
        }
    }

    /// Refresh log content from file
    pub fn refresh(&mut self) {
        if let Some(log_file) = get_latest_log_file() {
            self.lines = read_last_lines(&log_file, 100);
        }
    }

    /// Set visibility
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if visible {
            self.refresh();
        }
    }

    /// Toggle visibility
    pub fn toggle(&mut self) {
        self.set_visible(!self.visible);
    }

    /// Check if visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Render the log panel as a floating overlay
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        // Refresh log content every time we render (auto-refresh)
        self.refresh();

        // Clear the area behind the log panel to block background content
        frame.render_widget(Clear, area);

        // Render log panel with background
        let block = Block::default()
            .title(" Debug Log [`] ")
            .bg(Color::Black)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(Color::Black));

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Prepare log lines for display
        let display_lines: Vec<Line> = self
            .lines
            .iter()
            .rev()
            .take(inner_area.height as usize)
            .rev()
            .map(|line| {
                // Colorize log levels
                if line.contains("ERROR") {
                    Line::from(Span::styled(line.clone(), Style::default().fg(Color::Red)))
                } else if line.contains("WARN") {
                    Line::from(Span::styled(
                        line.clone(),
                        Style::default().fg(Color::Yellow),
                    ))
                } else if line.contains("INFO") {
                    Line::from(Span::styled(
                        line.clone(),
                        Style::default().fg(Color::Green),
                    ))
                } else if line.contains("DEBUG") {
                    Line::from(Span::styled(line.clone(), Style::default().fg(Color::Cyan)))
                } else {
                    Line::from(line.clone())
                }
            })
            .collect();

        let paragraph = Paragraph::new(display_lines).style(Style::default().bg(Color::Black));
        frame.render_widget(paragraph, inner_area);
    }
}

impl Default for LogPanel {
    fn default() -> Self {
        Self::new()
    }
}
