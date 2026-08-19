use crate::tui::widgets::Terminal;
use clap::{CommandFactory, FromArgMatches};
use std::io::Write;
use std::time::Instant;

pub mod ai;
/// Sensors Analytics. `analytics::core` is shared verbatim with the desktop
/// app; everything terminal-specific lives beside it in `analytics/mod.rs`.
pub mod analytics;
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
pub mod secure_storage;
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
    // Strip terminal control/escape sequences from server-controlled text
    // before it hits stderr, so a hostile API error cannot repaint the
    // terminal. Reuses the shared helper (keeps newlines/tabs).
    use crate::utils::text::strip_control_chars as sanitize_server_text;

    if let Some(lb_err) = e.downcast_ref::<LbError>() {
        match lb_err {
            LbError::HttpClient(HttpClientError::OpenApi {
                code,
                message,
                trace_id,
            }) => {
                eprintln!(
                    "Error: API error (code {code}): {}",
                    sanitize_server_text(message)
                );
                if !trace_id.is_empty() {
                    eprintln!("  trace_id: {}", sanitize_server_text(trace_id));
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
                    detail.code,
                    sanitize_server_text(&detail.msg)
                );
                if let Some(guidance) =
                    option_quote_permission_guidance(detail.code, std::env::args())
                {
                    eprintln!("\n{guidance}");
                }
                return;
            }
            LbError::WsClient(WsClientError::ConnectionClosed {
                reason: Some(reason),
            }) => {
                eprintln!(
                    "Error: Connection closed ({:?}): {}",
                    reason.code,
                    sanitize_server_text(&reason.message)
                );
                return;
            }
            _ => {}
        }
    }

    // Network-layer failures (`HttpClientError::Http`, WebSocket connect
    // errors) carry no structured detail, so they fall through to the raw
    // message here.
    let rendered = sanitize_server_text(&format!("{e:#}"));
    eprintln!("Error: {rendered}");
    if let Some(guidance) = cn_access_point_guidance(&rendered) {
        eprintln!("\n{guidance}");
    }
}

/// Guidance for a failed request against the China Mainland access point.
///
/// A client outside China Mainland can end up pinned to `longbridge.cn` — by a
/// region cache written while travelling, or by a proxy that exits in China —
/// and then every request fails to connect. The bare transport error names the
/// host but gives no hint that the host is the problem, or that it can be
/// overridden.
fn cn_access_point_guidance(rendered: &str) -> Option<&'static str> {
    let is_connect_failure = rendered.contains("error sending request")
        || rendered.contains("Connect")
        || rendered.contains("timeout")
        || rendered.contains("dns error");

    (rendered.contains("longbridge.cn") && is_connect_failure).then_some(
        "This request used the China Mainland access point (longbridge.cn),\n\
         which is normally unreachable from outside China Mainland.\n\
         Re-detect: longbridge check\n\
         Override:  LONGBRIDGE_REGION=global longbridge <command>",
    )
}

fn is_option_quote_command(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    let positional: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .filter(|arg| !arg.starts_with('-'))
        .collect();

    positional
        .windows(2)
        .any(|pair| pair[0] == "option" && pair[1] == "quote")
}

fn option_quote_permission_guidance(
    error_code: u64,
    args: impl IntoIterator<Item = impl AsRef<str>>,
) -> Option<&'static str> {
    (error_code == 301_604 && is_option_quote_command(args)).then_some(
        "US option quotes require the \"OPRA US Options\" OpenAPI market data permission.\n\
         OpenAPI permissions are separate from App / PC / Web permissions.\n\
         Subscribe: https://open.longbridge.com/pricing/\n\
         Check access: longbridge auth status",
    )
}

/// The subcommand the user ran, as clap resolved it: `"kline"`, `"alert add"`.
///
/// Taken from the parse rather than from a rescan of `argv`, which is the only
/// way to get this right. Scanning cannot tell a subcommand from a global
/// option's value (`--lang zh`), loses the second level entirely (`alert add`
/// reads as `alert`), and needs an `unknown` bucket for anything it fails to
/// recognise. clap already knows the answer; this asks it.
///
/// Names only — never argument values, which carry symbols, account numbers and
/// order details.
fn command_path(matches: &clap::ArgMatches) -> String {
    let mut path = Vec::new();
    let mut current = matches;
    // Two levels is every group this CLI has (`alert add`, `auth login`).
    while let Some((name, sub)) = current.subcommand() {
        path.push(name.to_owned());
        current = sub;
    }
    path.join(" ")
}

#[tokio::main]
async fn main() {
    match cli::schema::handle_schema_args(std::env::args_os()) {
        Ok(cli::schema::SchemaOutcome::NotRequested) => {}
        Ok(cli::schema::SchemaOutcome::Handled) => return,
        Ok(cli::schema::SchemaOutcome::Error) => std::process::exit(1),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }

    let _guard = logger::init();

    // Clean up leftover .old binary from a previous Windows update.
    update::cleanup_old_binary();

    // Parsed through `ArgMatches` rather than `Cli::parse()` so the resolved
    // subcommand path is available for reporting; the typed value is the same
    // one `parse` would have produced.
    let matches = cli::Cli::command().get_matches();
    let cli = cli::Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let verbose = cli.verbose;

    locale::init(cli.lang.as_deref());
    rust_i18n::set_locale(locale::get());

    // `agent --skill` prints a static, built-in document: no auth, no
    // network. Handle it before the region-cache refresh (and the background
    // version check) so it stays reliably offline and instant, which is what
    // makes it usable from a harness bootstrap step.
    if let Some(cli::Commands::Agent { skill: true, .. }) = &cli.command {
        cli::agent::skills::print_skills_doc();
        return;
    }

    // The same reasoning covers every request that is answerable without the
    // network: decide it here, before the region probe and `init_contexts`.
    // Reaching these from `dispatch` would be too late — a logged-out user
    // asking `longbridge agent` what it can do would get an auth failure
    // instead of the help they asked for.
    match &cli.command {
        // A command group with no subcommand: show the options, exit like
        // clap does for a missing mandatory subcommand.
        Some(cli::Commands::Agent { cmd: None, .. }) => {
            cli::exit_with_subcommand_help("agent");
        }
        // `--interactive` needs a terminal to prompt on, so it cannot be
        // combined with machine-readable output. Rejecting it here keeps the
        // failure free of any token refresh or region request.
        Some(cli::Commands::Agent { cmd: Some(sub), .. }) => {
            if let Err(e) = cli::agent::chat::ensure_interactive_supported_for(sub, &cli.format) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        _ => {}
    }

    // Re-probe the access-point region if the cached verdict has gone stale.
    // Usually a no-op; only the first run after the cache TTL expires waits.
    // `check` detects unconditionally, so skip the routine refresh for it.
    if !matches!(cli.command, Some(cli::Commands::Check)) {
        region::refresh_region_cache().await;
    }

    // Kick off background version check to refresh the update cache for the next run.
    update::spawn_version_check();

    // Show release notes URL once after a version change (e.g. brew upgrade, manual install).
    update::check_and_show_release_notes();

    // Armed before the dispatch, not inside it: several commands (auth, tui, ai,
    // completion, acp) have match arms of their own, so anything wired into the
    // catch-all arm would silently miss them — which it did, until a run of
    // `auth status` produced no event at all.
    //
    // Before the OpenAPI context too: a command that fails to authenticate is
    // exactly the kind worth recording.
    //
    // `completion` is the one command left out. The README has users put
    // `source <(longbridge completion zsh)` in their shell profile, so it runs
    // on every new shell — reporting there would make it the most-used command
    // in the product by a wide margin, and would put a network round trip in
    // front of every prompt.
    let reports = !matches!(cli.command, Some(cli::Commands::Completion { .. }));
    // Worked out once and used for both decisions below. `ai`, `serve` and `acp`
    // stay open, so they beat and report on entry; deciding that from a second
    // list of subcommands further down is how the two lists drift apart.
    let shape = match &cli.command {
        Some(cli::Commands::Tui) => analytics::Shape::Tui,
        Some(cli::Commands::Ai { .. } | cli::Commands::Serve | cli::Commands::Acp { .. }) => {
            analytics::Shape::Session
        }
        _ => analytics::Shape::Command,
    };
    if reports {
        analytics::init(shape);
        analytics::install_crash_hook();
    }
    // `tui` is left out here as well: it reports its own launch event, and
    // counting it as a command too would double every total.
    if reports && cli.command.is_some() && !matches!(cli.command, Some(cli::Commands::Tui)) {
        analytics::arm_command(command_path(&matches));
        // A session is killed far more often than it exits, so its run event
        // goes out now; a one-shot command reports on the way out instead, where
        // it can say how it went. See `analytics`.
        if shape.reports_on_entry() {
            analytics::report_started();
        }
    }

    match cli.command {
        None => {
            // No subcommand: print help and exit
            cli::Cli::command().print_help().unwrap();
            println!();
        }

        Some(cli::Commands::Tui) => {
            tracing::info!("App started");
            analytics::track(analytics::event::TUI_LAUNCH, serde_json::json!({}));
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
                // Recorded before the terminal is restored, so a panic during
                // the restore itself still leaves a report for the next run.
                analytics::note_crash(&info.to_string());
                Terminal::exit_full_screen();
                hook(info);
            }));

            let _ = std::io::stdout().write_all(b"\n");
            let _ = std::io::stdout().flush();

            Terminal::enter_full_screen();
            tui::app::run(Args { logout: false }, quote_receiver).await;
            Terminal::exit_full_screen();
            // The TUI outlives its own requests while running, but its last
            // ones are raised on the way out — after this point the runtime
            // goes away and anything unsent goes with it.
            analytics::flush().await;
            return;
        }

        // `longbridge ai`: the interactive Longbridge AI chat TUI. Needs a live
        // context, so a failed init exits (a prompt turn cannot run without it).
        Some(cli::Commands::Ai { agent }) => {
            // Hydrate the persisted chat preferences (tool-call display, quote
            // cards, notifications, the ticker tape). The market TUI does this on
            // startup; `longbridge ai` launches on its own, so without this the
            // Settings view would forget every change on the next launch.
            crate::tui::settings::load_and_apply();
            // The chat opens signed out. Everything that needs credentials is
            // guarded inside it, and Settings offers to sign in — which then builds
            // the contexts in place, so the reader carries on in the same session.
            // Refusing to start was the wrong call: signing in is the one thing you
            // would come here to do without a token.
            let quote_receiver: Option<ai::QuoteStream> = match openapi::init_contexts().await {
                Ok((rx, _, _)) => Some(Box::pin(rx)),
                Err(_) => None,
            };

            // Reported after the contexts are built, because whether the chat
            // opened signed in is the interesting half: it opens either way, and
            // a reader who arrives without a token behaves nothing like one who
            // arrives with one.
            ai::analytics::launch(&agent, quote_receiver.is_some());

            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                // `ai::run` also turns on bracketed paste and focus reporting;
                // `exit_full_screen` only undoes the alternate screen and mouse
                // capture, so disable them here too or a panic leaves the
                // terminal echoing paste brackets and focus escapes.
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::event::DisableBracketedPaste,
                    crossterm::event::DisableFocusChange,
                );
                Terminal::exit_full_screen();
                hook(info);
            }));

            Terminal::enter_full_screen();
            let result = ai::run(agent, quote_receiver).await;
            Terminal::exit_full_screen();
            match result {
                // Signing in or out is reported here, outside the alternate
                // screen — printed inside it, the message would scroll away with
                // it.
                Ok(Some(note)) => println!("{note}"),
                Ok(None) => {}
                Err(e) => eprintln!("Error: {e}"),
            }
            // This arm returns rather than falling through to the flush at the
            // end of `main`, so it has to flush for itself. Without this the
            // chat's own events — including the last turn of the session — were
            // cancelled with the runtime and never sent. The run event itself
            // has already gone out: a session reports on the way in.
            analytics::flush().await;
            return;
        }

        // `serve` is the only command that keeps the market WebSocket: every
        // other one discards the push stream after `init_contexts`.
        Some(cli::Commands::Serve) => {
            let (quote_receiver, using_api_key, _) = match openapi::init_contexts().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Authentication failed: {e}");
                    analytics::flush().await;
                    std::process::exit(1);
                }
            };
            if let Err(e) = openapi::quote().member_id().await {
                print_cli_error(&anyhow::anyhow!(e), using_api_key);
                analytics::flush().await;
                std::process::exit(1);
            }
            if let Err(e) = cli::serve::run(quote_receiver).await {
                eprintln!("Error: {e}");
                analytics::flush().await;
                std::process::exit(1);
            }
            analytics::flush().await;
            return;
        }

        Some(cli::Commands::Init { invite_code }) => {
            if let Err(e) = cli::init::cmd_init(&invite_code) {
                eprintln!("Error: {e}");
                analytics::finish_command(analytics::Outcome::Error).await;
                std::process::exit(1);
            }
        }

        Some(cli::Commands::Check) => {
            if let Err(e) = cli::check::cmd_check(&cli.format).await {
                print_cli_error(&e, false);
                analytics::finish_command(analytics::Outcome::Error).await;
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
                    analytics::finish_command(analytics::Outcome::Error).await;
                    std::process::exit(1);
                }
            } else if let Err(e) = update::cmd_update(verbose, force).await {
                eprintln!("Error: {e}");
                analytics::finish_command(analytics::Outcome::Error).await;
                std::process::exit(1);
            }
            analytics::finish_command(analytics::Outcome::Ok).await;
            return;
        }

        Some(cli::Commands::Auth {
            cmd:
                cli::AuthCmd::Login {
                    auth_code: Some(code),
                    client_name,
                    ..
                },
        }) => {
            // `--auth-code <CODE>` (non-empty): exchange an authorization code in
            // a single synchronous call. `--auth-code` with no value falls back to
            // the browser Authorization Code flow.
            let result = if code.is_empty() {
                auth::auth_code_login(client_name).await
            } else {
                auth::auth_code_exchange_login(&code).await
            };
            if let Err(e) = result {
                eprintln!("Authentication failed: {e:#}");
                analytics::finish_command(analytics::Outcome::AuthFailed).await;
                std::process::exit(1);
            }
        }

        // ACP clients start terminal auth by re-running the launch command with
        // the auth method's args appended, i.e. `longbridge acp auth login`.
        // That alias lands here and behaves like a plain `longbridge auth login`.
        Some(
            cli::Commands::Auth {
                cmd:
                    cli::AuthCmd::Login {
                        auth_code: None,
                        client_name,
                        verbose,
                    },
            }
            | cli::Commands::Acp {
                cmd:
                    Some(cli::AcpCmd::Auth {
                        action: cli::AcpAuthAction::Login,
                        client_name,
                        verbose,
                    }),
                ..
            },
        ) => {
            if let Err(e) = auth::device_login(verbose, client_name).await {
                eprintln!("Authentication failed: {e:#}");
                analytics::finish_command(analytics::Outcome::AuthFailed).await;
                std::process::exit(1);
            }
        }

        Some(
            cli::Commands::Auth {
                cmd: cli::AuthCmd::Logout,
            }
            | cli::Commands::Acp {
                cmd:
                    Some(cli::AcpCmd::Auth {
                        action: cli::AcpAuthAction::Logout,
                        ..
                    }),
                ..
            },
        ) => match auth::clear_token().await {
            Ok(()) => println!("Successfully logged out."),
            Err(e) => {
                eprintln!("Failed to clear credentials: {e}");
                analytics::finish_command(analytics::Outcome::Error).await;
                std::process::exit(1);
            }
        },

        Some(cli::Commands::Auth {
            cmd: cli::AuthCmd::Status { market },
        }) => {
            if let Err(e) = cli::auth::cmd_auth_status(&cli.format, &market).await {
                eprintln!("Error: {e}");
                analytics::finish_command(analytics::Outcome::Error).await;
                std::process::exit(1);
            }
        }

        Some(cli::Commands::Completion { shell }) => {
            cli::completion::cmd_completion(shell);
        }

        Some(cli::Commands::Acp { agent_id, cmd: _ }) => {
            let agent_id = agent_id
                .or_else(|| std::env::var("LONGBRIDGE_AGENT_ID").ok())
                .unwrap_or_else(|| cli::agent::DEFAULT_AGENT_UID.to_string());
            let auth_methods = vec![longbridge_ai_acp::acp::schema::v1::AuthMethod::Terminal(
                longbridge_ai_acp::acp::schema::v1::AuthMethodTerminal::new(
                    "longbridge-login",
                    "Log in to Longbridge",
                )
                .description("Authenticate with Longbridge OAuth in an interactive terminal")
                .args(vec!["auth".into(), "login".into()]),
            )];
            let result = longbridge_ai_acp::serve_stdio_with_auth_methods(
                openapi::AuthenticationRequiredAgent::new(&agent_id),
                auth_methods,
            )
            .await;
            if let Err(e) = result {
                print_cli_error(&anyhow::anyhow!(e), false);
                analytics::flush().await;
                std::process::exit(1);
            }
        }

        // `Agent { skill: true }` never reaches here: it is handled above,
        // before any network work.
        Some(cmd) => {
            let start = verbose.then(Instant::now);
            // CLI mode: init contexts (auth), then dispatch
            let (using_api_key, http_url) = match openapi::init_contexts().await {
                Ok((_, using_api_key, http_url)) => (using_api_key, http_url),
                Err(e) => {
                    eprintln!("Authentication failed: {e}");
                    // Reported before exiting: a failure to authenticate is
                    // exactly the kind of run worth knowing about, and
                    // `process::exit` gives background tasks no chance to run.
                    analytics::finish_command(analytics::Outcome::AuthFailed).await;
                    std::process::exit(1);
                }
            };
            if verbose {
                eprintln!("* Host: {http_url}");
            }
            if let Err(e) = cli::dispatch(cmd, &cli.format, cli.verbose).await {
                print_cli_error(&e, using_api_key);
                analytics::finish_command(analytics::Outcome::Error).await;
                std::process::exit(1);
            }
            if let Some(t) = start {
                let _ = std::io::stdout().flush();
                eprintln!("* Elapsed: {:.3}s", t.elapsed().as_secs_f64());
            }
        }
    }

    update::notify_if_update_available();

    // Every path that reaches here is about to return from `main`, which drops
    // the runtime and cancels anything still sending. The exits above report on
    // their own; this covers the rest.
    analytics::finish_command(analytics::Outcome::Ok).await;
}

#[cfg(test)]
mod command_path_tests {
    use super::command_path;
    use clap::CommandFactory;

    /// Built on a thread of its own because this CLI's clap tree is deep enough
    /// to overflow a test thread's default stack while it is being constructed —
    /// a property of the tree's size in a debug build, not of the parse.
    fn path_of<const N: usize>(args: [&str; N]) -> String {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || command_path(&crate::cli::Cli::command().get_matches_from(args)))
            .expect("spawn")
            .join()
            .expect("parse")
    }

    /// The plain case, and the reason the name is worth reporting at all.
    #[test]
    fn names_the_command() {
        assert_eq!(path_of(["longbridge", "quote", "700.HK"]), "quote");
    }

    /// A group's second level is where the interesting distinction lives:
    /// listing alerts and creating one are not the same activity.
    #[test]
    fn names_the_second_level_too() {
        assert_eq!(
            path_of(["longbridge", "alert", "add", "TSLA.US", "--price", "200"]),
            "alert add"
        );
        assert_eq!(
            path_of(["longbridge", "profit-analysis", "detail", "700.HK"]),
            "profit-analysis detail"
        );
    }

    /// A group used without a subcommand is its own case, not a missing name.
    #[test]
    fn a_bare_group_is_just_the_group() {
        assert_eq!(path_of(["longbridge", "alert"]), "alert");
    }

    /// A global option's value is a bare argument too. Scanning `argv` for the
    /// first bare word would report `zh` as the command.
    #[test]
    fn a_global_options_value_is_not_the_command() {
        assert_eq!(
            path_of(["longbridge", "--lang", "zh", "quote", "700.HK"]),
            "quote"
        );
    }

    /// Nothing to report when nothing was run: `longbridge` alone prints help.
    #[test]
    fn no_subcommand_is_empty() {
        assert_eq!(path_of(["longbridge"]), "");
    }
}

#[cfg(test)]
mod error_guidance_tests {
    use super::{is_option_quote_command, option_quote_permission_guidance};

    #[test]
    fn detects_option_quote_command() {
        assert!(is_option_quote_command([
            "longbridge",
            "--format",
            "json",
            "option",
            "quote",
            "AAPL260722C320000.US",
        ]));
    }

    #[test]
    fn ignores_other_quote_commands() {
        assert!(!is_option_quote_command(
            ["longbridge", "quote", "AAPL.US",]
        ));
        assert!(!is_option_quote_command([
            "longbridge",
            "option",
            "chain",
            "AAPL.US",
        ]));
    }

    #[test]
    fn guides_option_quote_access_errors() {
        let guidance = option_quote_permission_guidance(
            301_604,
            ["longbridge", "option", "quote", "AAPL260722C320000.US"],
        )
        .expect("permission guidance");

        assert!(guidance.contains("OPRA US Options"));
        assert!(guidance.contains("https://open.longbridge.com/pricing/"));
        assert!(guidance.contains("longbridge auth status"));
    }

    #[test]
    fn does_not_guide_unrelated_errors() {
        assert!(option_quote_permission_guidance(
            301_600,
            ["longbridge", "option", "quote", "INVALID.US"],
        )
        .is_none());
        assert!(
            option_quote_permission_guidance(301_604, ["longbridge", "quote", "AAPL.US"],)
                .is_none()
        );
    }
}
