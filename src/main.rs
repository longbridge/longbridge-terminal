use crate::widgets::Terminal;

#[macro_use]
mod macros;

pub mod api;
pub mod app;
pub mod data;
pub mod helper;
pub mod kline;
pub mod logger;
pub mod openapi;
#[cfg_attr(target_family = "windows", path = "os/windows.rs")]
#[cfg_attr(target_family = "unix", path = "os/unix.rs")]
pub mod os;
pub mod system;
pub mod ui;
pub mod widgets;

mod views;

#[macro_use]
extern crate rust_i18n;
i18n!("locales");

/// Simplified command line arguments (temporary)
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub logout: bool,
}

#[tokio::main]
async fn main() {
    // Initialize logger
    let _guard = logger::init();
    tracing::info!("App started");

    // Initialize OpenAPI first (before entering fullscreen mode, so SDK outputs stay in main screen)
    let quote_receiver = match openapi::init_contexts().await {
        Ok(receiver) => receiver,
        Err(e) => {
            openapi::print_config_guide();
            eprintln!("\nError details: {}", e);
            return;
        }
    };

    tracing::info!("OpenAPI initialized successfully");

    // Set up panic hook to restore terminal
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        Terminal::exit_full_screen();
        hook(info);
    }));

    // Clean terminal state to ensure no residual output
    use std::io::Write;
    let _ = std::io::stdout().write_all(b"\n");
    let _ = std::io::stdout().flush();

    // Now enter fullscreen mode (SDK is initialized, alternate screen is clean)
    Terminal::enter_full_screen();
    let args = Args::default();
    app::run(args, quote_receiver).await;
    Terminal::exit_full_screen();
}
