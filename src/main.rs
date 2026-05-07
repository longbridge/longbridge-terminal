use crate::tui::widgets::Terminal;
use clap::Parser;
use std::io::Write;
use std::time::Instant;

pub mod auth;
pub mod cli;
pub mod data;
pub mod locale;
pub mod logger;
pub mod openapi;
#[cfg_attr(target_family = "windows", path = "os/windows.rs")]
#[cfg_attr(target_family = "unix", path = "os/unix.rs")]
pub mod os;
pub mod region;
pub mod tui;
pub mod update;
pub mod utils;

#[macro_use]
extern crate rust_i18n;
i18n!("locales");

/// Command line arguments (kept for TUI compatibility via `crate::Args`)
#[derive(Clone, Debug)]
pub struct Args {
    pub logout: bool,
}

fn print_cli_error(e: &anyhow::Error, using_api_key: bool) {
    use longbridge::{httpclient::HttpClientError, wsclient::WsClientError, Error as LbError};

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
                if using_api_key && *code == 401_003 {
                    eprintln!(
                        "\nYou are currently using environment variable authentication.\n\
                        Please check that LONGBRIDGE_APP_KEY, LONGBRIDGE_APP_SECRET, and LONGBRIDGE_ACCESS_TOKEN are valid.\n\
                        To switch to OAuth instead, unset these environment variables and restart."
                    );
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
    let _guard = logger::init();

    // Clean up leftover .old binary from a previous Windows update.
    update::cleanup_old_binary();

    let cli = cli::Cli::parse();
    let verbose = cli.verbose;

    locale::init(cli.lang.as_deref());
    rust_i18n::set_locale(locale::get());

    // Kick off background geotest check to refresh the region cache for the next run.
    region::spawn_region_update();

    // Kick off background version check to refresh the update cache for the next run.
    update::spawn_version_check();

    // Show release notes URL once after a version change (e.g. brew upgrade, manual install).
    update::check_and_show_release_notes();

    match cli.command {
        None => {
            // No subcommand: print help and exit
            use clap::CommandFactory;
            cli::Cli::command().print_help().unwrap();
            println!();
        }

        Some(cli::Commands::Tui) => {
            tracing::info!("App started");
            let (quote_receiver, using_api_key, _) = match openapi::init_contexts().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("OAuth2 authentication failed: {e}");
                    return;
                }
            };
            if let Err(e) = openapi::quote().member_id().await {
                print_cli_error(&anyhow::anyhow!(e), using_api_key);
                return;
            }
            tracing::info!("OpenAPI initialized successfully");

            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                Terminal::exit_full_screen();
                hook(info);
            }));

            let _ = std::io::stdout().write_all(b"\n");
            let _ = std::io::stdout().flush();

            Terminal::enter_full_screen();
            tui::app::run(Args { logout: false }, quote_receiver).await;
            Terminal::exit_full_screen();
            return;
        }

        Some(cli::Commands::Init { invite_code }) => {
            if let Err(e) = cli::init::cmd_init(&invite_code) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }

        Some(cli::Commands::Check) => {
            if let Err(e) = cli::check::cmd_check(&cli.format).await {
                print_cli_error(&e, false);
                std::process::exit(1);
            }
        }

        Some(cli::Commands::Update {
            release_notes,
            force,
        }) => {
            if release_notes {
                if let Err(e) = update::cmd_release_notes().await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            } else if let Err(e) = update::cmd_update(verbose, force).await {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            return;
        }

        Some(cli::Commands::Auth {
            cmd: cli::AuthCmd::Login {
                auth_code: true, ..
            },
        }) => {
            if let Err(e) = auth::auth_code_login().await {
                eprintln!("Authentication failed: {e:#}");
                std::process::exit(1);
            }
        }

        Some(cli::Commands::Auth {
            cmd:
                cli::AuthCmd::Login {
                    auth_code: false,
                    verbose,
                },
        }) => {
            if let Err(e) = auth::device_login(verbose).await {
                eprintln!("Authentication failed: {e:#}");
                std::process::exit(1);
            }
        }

        Some(cli::Commands::Auth {
            cmd: cli::AuthCmd::Logout,
        }) => match auth::clear_token() {
            Ok(()) => println!("Successfully logged out."),
            Err(e) => {
                eprintln!("Failed to clear credentials: {e}");
                std::process::exit(1);
            }
        },

        Some(cli::Commands::Auth {
            cmd: cli::AuthCmd::Status { market },
        }) => {
            if let Err(e) = cli::auth::cmd_auth_status(&cli.format, &market).await {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }

        Some(cli::Commands::Completion { shell }) => {
            cli::completion::cmd_completion(shell);
        }

        Some(cmd) => {
            let start = verbose.then(Instant::now);
            // CLI mode: init contexts (auth), then dispatch
            let (using_api_key, http_url) = match openapi::init_contexts().await {
                Ok((_, using_api_key, http_url)) => (using_api_key, http_url),
                Err(e) => {
                    eprintln!("Authentication failed: {e}");
                    std::process::exit(1);
                }
            };
            if verbose {
                eprintln!("* Host: {http_url}");
            }
            if let Err(e) = cli::dispatch(cmd, &cli.format, cli.verbose).await {
                print_cli_error(&e, using_api_key);
                std::process::exit(1);
            }
            if let Some(t) = start {
                let _ = std::io::stdout().flush();
                eprintln!("* Elapsed: {:.3}s", t.elapsed().as_secs_f64());
            }
        }
    }

    update::notify_if_update_available();
}
