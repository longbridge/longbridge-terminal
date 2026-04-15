use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use longbridge::asset::{GetStatementListOptions, GetStatementOptions, StatementType};
use serde_json::json;

use super::OutputFormat;

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Format a duration in seconds as a human-readable string (e.g. "2h 14m", "45m", "30s").
fn format_duration(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    } else if secs >= 60 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    } else {
        format!("{secs}s")
    }
}

struct TokenState {
    status: &'static str,
    detail: String,
}

fn read_token_state() -> Result<TokenState> {
    let token_path = crate::auth::token_file_path()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if !token_path.exists() {
        return Ok(TokenState {
            status: "not_found",
            detail: format!("run {DIM}longbridge auth login{RESET} to authenticate"),
        });
    }

    let contents = std::fs::read_to_string(&token_path)?;
    let data: serde_json::Value = serde_json::from_str(&contents)?;
    let expires_at = data["expires_at"].as_u64().unwrap_or(0);

    if expires_at == 0 {
        return Ok(TokenState {
            status: "present",
            detail: String::new(),
        });
    }

    if expires_at > now {
        Ok(TokenState {
            status: "valid",
            detail: format!("expires in {}", format_duration(expires_at - now)),
        })
    } else {
        Ok(TokenState {
            status: "expired",
            detail: format!(
                "{} ago — run {DIM}longbridge auth login{RESET} to re-authenticate",
                format_duration(now - expires_at)
            ),
        })
    }
}

struct AccountInfo {
    member_id: i64,
    quote_level: String,
    account_no: Option<String>,
    account_type: Option<String>,
    name: Option<String>,
}

/// Fetch account_no and account_type from the most recent daily statement.
/// Returns None if no statement is available or the fetch fails.
async fn fetch_account_info_from_statement() -> Option<(String, String, String)> {
    let ctx = crate::openapi::statement();

    let list_resp = ctx
        .statements(
            GetStatementListOptions::new(StatementType::Daily)
                .page(1)
                .page_size(1),
        )
        .await
        .ok()?;

    let file_key = list_resp.list.into_iter().next()?.file_key;

    let dl_resp = ctx
        .statement_download_url(GetStatementOptions::new(&file_key))
        .await
        .ok()?;

    let body = reqwest::Client::new()
        .get(&dl_resp.url)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;

    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let mi = &value["MemberInfo"];

    let account_no = mi["AccountNo"].as_str().filter(|s| !s.is_empty())?.to_owned();
    let account_type = mi["AccountType"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let name = mi["NameEn"]
        .as_str()
        .or_else(|| mi["Name"].as_str())
        .unwrap_or("")
        .to_owned();

    Some((account_no, account_type, name))
}

async fn fetch_account_info() -> Result<AccountInfo> {
    let (member_id, quote_level, statement_info) = tokio::join!(
        crate::openapi::quote().member_id(),
        crate::openapi::quote().quote_level(),
        fetch_account_info_from_statement(),
    );

    let (account_no, account_type, name) = match statement_info {
        Some((no, t, n)) => (Some(no), Some(t).filter(|s| !s.is_empty()), Some(n).filter(|s| !s.is_empty())),
        None => (None, None, None),
    };

    Ok(AccountInfo {
        member_id: member_id?,
        quote_level: quote_level?,
        account_no,
        account_type,
        name,
    })
}

pub async fn cmd_auth_status(format: &OutputFormat) -> Result<()> {
    // ── Token (local) ─────────────────────────────────────────────────────────
    let token_path = crate::auth::token_file_path()?;
    let token = read_token_state()?;

    // ── Connect and fetch account info ────────────────────────────────────────
    let account = match crate::openapi::init_contexts().await {
        Ok(_) => match fetch_account_info().await {
            Ok(info) => Some(info),
            Err(_) => None,
        },
        Err(_) => None,
    };

    match format {
        OutputFormat::Json => {
            let mut value = json!({
                "token": {
                    "status": token.status,
                    "path": token_path.display().to_string(),
                },
            });

            if let Some(acc) = &account {
                value["account"] = json!({
                    "member_id": acc.member_id,
                    "quote_level": acc.quote_level,
                    "account_no": acc.account_no,
                    "account_type": acc.account_type,
                    "name": acc.name,
                });
            }

            println!("{}", serde_json::to_string_pretty(&value)?);
        }

        OutputFormat::Pretty => {
            // Token section
            let (icon, label) = match token.status {
                "not_found" => (format!("{RED}✗{RESET}"), format!("{RED}not found{RESET}")),
                "expired" => (format!("{YELLOW}!{RESET}"), format!("{YELLOW}expired{RESET}")),
                _ => (format!("{GREEN}✓{RESET}"), format!("{GREEN}{}{RESET}", token.status)),
            };
            println!("Token");
            println!("  {icon}  {label}  {}", token.detail);
            println!("  {DIM}{}{RESET}", token_path.display());

            // Account section
            if let Some(acc) = &account {
                println!();
                println!("Account");
                if let Some(name) = &acc.name {
                    println!("  {:<14} {name}", "name");
                }
                if let Some(account_no) = &acc.account_no {
                    println!("  {:<14} {account_no}", "account_no");
                }
                if let Some(account_type) = &acc.account_type {
                    println!("  {:<14} {account_type}", "account_type");
                }
                println!("  {:<14} {}", "member_id", acc.member_id);
                println!("  {:<14} {}", "quote_level", acc.quote_level);
            }
        }
    }

    Ok(())
}
