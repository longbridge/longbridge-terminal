use crate::widgets::Terminal;
use std::io::Write;

#[macro_use]
mod macros;

pub mod app;
pub mod auth;
pub mod data;
pub mod kline;
pub mod logger;
pub mod openapi;
#[cfg_attr(target_family = "windows", path = "os/windows.rs")]
#[cfg_attr(target_family = "unix", path = "os/unix.rs")]
pub mod os;
pub mod render;
pub mod systems;
pub mod ui;
pub mod utils;
pub mod widgets;

mod views;

#[macro_use]
extern crate rust_i18n;
i18n!("locales");

/// Command line arguments
#[derive(Clone, Debug)]
pub struct Args {
    pub logout: bool,
}

#[tokio::main]
async fn main() {
    // Set default locale to English
    rust_i18n::set_locale("en");

    // Initialize logger
    let _guard = logger::init();

    // Parse command line arguments
    let matches = clap::Command::new("Longbridge Terminal")
        .version(env!("CARGO_PKG_VERSION"))
        .about("A TUI stock trading terminal")
        .arg(
            clap::Arg::new("logout")
                .long("logout")
                .help("Clear stored OAuth token and exit")
                .takes_value(false),
        )
        .get_matches();

    let args = Args {
        logout: matches.is_present("logout"),
    };

    // Handle logout command
    if args.logout {
        match auth::clear_token() {
            Ok(()) => {
                println!("✓ Successfully logged out.");
                tracing::info!("User logged out, credentials cleared");
            }
            Err(e) => {
                eprintln!("Failed to clear credentials: {e}");
                tracing::error!("Failed to clear credentials: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    tracing::info!("App started");

    // Initialize OpenAPI first (before entering fullscreen mode, so SDK outputs stay in main screen)
    let quote_receiver = match openapi::init_contexts().await {
        Ok(receiver) => receiver,
        Err(e) => {
            eprintln!("OAuth2 authentication failed: {e}");
            eprintln!();
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

    let _ = std::io::stdout().write_all(b"\n");
    let _ = std::io::stdout().flush();

    // Now enter fullscreen mode (SDK is initialized, alternate screen is clean)
    Terminal::enter_full_screen();
    app::run(args, quote_receiver).await;
    Terminal::exit_full_screen();
}
