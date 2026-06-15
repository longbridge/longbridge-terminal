use std::any::Any;
use std::path::{Path, PathBuf};

type LogWriter = tracing_subscriber::fmt::writer::BoxMakeWriter;
type LogGuard = Option<tracing_appender::non_blocking::WorkerGuard>;

pub fn default_log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("Library/Logs/Longbridge");
        path
    }
    #[cfg(target_os = "windows")]
    {
        let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("Longbridge\\Logs");
        path
    }
    #[cfg(target_os = "linux")]
    {
        let mut path = dirs::data_local_dir()
            .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        path.push("longbridge/logs");
        path
    }
}

fn local_offset() -> time::UtcOffset {
    time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC)
}

fn build_rolling_file_appender(
    log_dir: &Path,
) -> Result<tracing_appender::rolling::RollingFileAppender, tracing_appender::rolling::InitError> {
    use tracing_appender::rolling::{RollingFileAppender, Rotation};

    RollingFileAppender::builder()
        .filename_prefix("longbridge")
        .filename_suffix("log")
        .max_log_files(5)
        .rotation(Rotation::DAILY)
        .build(log_dir)
}

fn non_blocking_log_writer(
    writer: tracing_appender::rolling::RollingFileAppender,
) -> (LogWriter, LogGuard) {
    let (writer, guard) = tracing_appender::non_blocking(writer);
    (
        tracing_subscriber::fmt::writer::BoxMakeWriter::new(writer),
        Some(guard),
    )
}

fn build_log_writer(log_dir: &Path, fallback_dir: &Path) -> (LogWriter, LogGuard) {
    match build_rolling_file_appender(log_dir) {
        Ok(writer) => non_blocking_log_writer(writer),
        Err(e) => {
            eprintln!(
                "Warning: Failed to create log file in {}: {e}",
                log_dir.display()
            );
            match build_rolling_file_appender(fallback_dir) {
                Ok(writer) => non_blocking_log_writer(writer),
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to create fallback log file in {}: {e}",
                        fallback_dir.display()
                    );
                    (
                        tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::sink),
                        None,
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_writer_falls_back_without_guard_when_file_appenders_fail() {
        let primary = tempfile::NamedTempFile::new().expect("primary temp file");
        let fallback = tempfile::NamedTempFile::new().expect("fallback temp file");

        let (_writer, guard) = build_log_writer(primary.path(), fallback.path());

        assert!(guard.is_none());
    }
}

#[must_use]
pub fn init() -> impl Any {
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;

    let log_dir = default_log_dir();
    std::fs::create_dir_all(&log_dir).ok();

    let (writer, guard) = build_log_writer(&log_dir, Path::new("."));

    let timer = fmt::time::OffsetTime::new(
        local_offset(),
        time::format_description::well_known::Rfc3339,
    );
    let file_line = cfg!(debug_assertions);

    let subscriber = fmt::layer()
        .with_ansi(false)
        .with_timer(timer)
        .with_thread_ids(true)
        .with_file(file_line)
        .with_line_number(file_line)
        .with_writer(writer);

    let dirs = "error,longbridge=debug";
    let dirs = std::env::var("LONGBRIDGE_LOG").unwrap_or_else(|_| dirs.to_string());
    let subscriber = subscriber.with_filter(tracing_subscriber::EnvFilter::new(dirs));

    tracing_subscriber::registry().with(subscriber).init();
    guard
}
