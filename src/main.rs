use crate::widgets::Terminal;
use clap::Parser;
use std::io::Write;

#[macro_use]
mod macros;

pub mod app;
pub mod auth;
pub mod cli;
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

/// Command line arguments (kept for TUI compatibility via crate::Args)
#[derive(Clone, Debug)]
pub struct Args {
    pub logout: bool,
}

fn print_cli_error(e: &anyhow::Error) {
    use longbridge::{Error as LbError, httpclient::HttpClientError, wsclient::WsClientError};

    if let Some(lb_err) = e.downcast_ref::<LbError>() {
        match lb_err {
            LbError::HttpClient(HttpClientError::OpenApi {
                code,
                message,
                trace_id,
            }) => {
                eprintln!("Error: API error (code {code}): {message}");
                if !trace_id.is_empty() {
                    eprintln!("  trace_id: {trace_id}");
                }
                return;
            }
            LbError::WsClient(WsClientError::ResponseError {
                status,
                detail: Some(detail),
            }) => {
                eprintln!(
                    "Error: WebSocket error (status={status}, code={}): {}",
                    detail.code, detail.msg
                );
                return;
            }
            LbError::WsClient(WsClientError::ConnectionClosed {
                reason: Some(reason),
            }) => {
                eprintln!(
                    "Error: Connection closed ({:?}): {}",
                    reason.code, reason.message
                );
                return;
            }
            _ => {}
        }
    }
    eprintln!("Error: {e:#}");
}

#[tokio::main]
async fn main() {
    rust_i18n::set_locale("en");
    let _guard = logger::init();

    let cli = cli::Cli::parse();

    // Handle legacy --logout flag
    if cli.logout {
        match auth::clear_token() {
            Ok(()) => println!("Successfully logged out."),
            Err(e) => {
                eprintln!("Failed to clear credentials: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    match cli.command {
        None => {
            // No subcommand: launch TUI
            tracing::info!("App started");
            let quote_receiver = match openapi::init_contexts().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("OAuth2 authentication failed: {e}");
                    return;
                }
            };
            tracing::info!("OpenAPI initialized successfully");

            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                Terminal::exit_full_screen();
                hook(info);
            }));

            let _ = std::io::stdout().write_all(b"\n");
            let _ = std::io::stdout().flush();

            Terminal::enter_full_screen();
            app::run(Args { logout: false }, quote_receiver).await;
            Terminal::exit_full_screen();
        }

        Some(cli::Commands::Login) => match openapi::init_contexts().await {
            Ok(_) => println!("Successfully authenticated."),
            Err(e) => {
                eprintln!("Authentication failed: {e}");
                std::process::exit(1);
            }
        },

        Some(cli::Commands::Logout) => match auth::clear_token() {
            Ok(()) => println!("Successfully logged out."),
            Err(e) => {
                eprintln!("Failed to clear credentials: {e}");
                std::process::exit(1);
            }
        },

        Some(cmd) => {
            // CLI mode: init contexts (auth), then dispatch
            if let Err(e) = openapi::init_contexts().await {
                eprintln!("Authentication failed: {e}");
                std::process::exit(1);
            }
            if let Err(e) = cli::dispatch(cmd, &cli.format).await {
                print_cli_error(&e);
                std::process::exit(1);
            }
        }
    }
}
