use std::any::Any;
use std::path::PathBuf;

fn default_log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let mut path = dirs::home_dir().expect("Unable to get user home directory");
        path.push("Library/Logs/Longbridge");
        path
    }
    #[cfg(target_os = "windows")]
    {
        let mut path = dirs::data_local_dir().expect("Unable to get local data directory");
        path.push("Longbridge\\Logs");
        path
    }
    #[cfg(target_os = "linux")]
    {
        let mut path = dirs::data_local_dir()
            .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
            .expect("Unable to get data directory");
        path.push("longbridge/logs");
        path
    }
}

fn local_offset() -> time::UtcOffset {
    time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC)
}

#[must_use]
pub fn init() -> impl Any {
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;

    let log_dir = default_log_dir();
    std::fs::create_dir_all(&log_dir).ok();

    let writer = RollingFileAppender::builder()
        .filename_prefix("longbridge")
        .filename_suffix("log")
        .max_log_files(5)
        .rotation(Rotation::DAILY)
        .build(log_dir)
        .expect("fail to create log file");
    let (writer, guard) = tracing_appender::non_blocking(writer);

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
