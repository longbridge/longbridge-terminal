use std::time::{SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, UtcOffset};

use anyhow::Result;
use longbridge::asset::{GetStatementListOptions, GetStatementOptions, StatementType};
use longbridge::httpclient::Json;
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use super::OutputFormat;

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Format a duration in seconds as a human-readable string (e.g. "2h 14m", "45m", "30s").
fn format_duration(secs: u64) -> String {
    if secs >= 86400 {
        let d = secs / 86400;
        let h = (secs % 86400) / 3600;
        if h == 0 {
            format!("{d}d")
        } else {
            format!("{d}d {h}h")
        }
    } else if secs >= 3600 {
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
    access_token_exp: Option<u64>,
    refresh_token_exp: Option<u64>,
    /// Unix timestamp of when the token file was last written (login time).
    logged_in_at: Option<u64>,
}

/// Decode a numeric field from a JWT payload without verifying the signature.
fn jwt_field(token: &str, field: &str) -> Option<u64> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    v[field].as_u64()
}

fn jwt_exp(token: &str) -> Option<u64> {
    jwt_field(token, "exp")
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
            access_token_exp: None,
            refresh_token_exp: None,
            logged_in_at: None,
        });
    }

    let contents = std::fs::read_to_string(&token_path)?;
    let data: serde_json::Value = serde_json::from_str(&contents)?;

    let logged_in_at = data["logged_in_at"].as_u64().or_else(|| {
        data["refresh_token"]
            .as_str()
            .and_then(|t| jwt_field(t, "iat"))
    });
    let expires_at = data["expires_at"].as_u64().unwrap_or(0);
    let access_token_exp = if expires_at > 0 {
        Some(expires_at)
    } else {
        None
    };
    let refresh_token_exp = data["refresh_token"].as_str().and_then(jwt_exp);

    if expires_at == 0 {
        return Ok(TokenState {
            status: "present",
            detail: String::new(),
            access_token_exp,
            refresh_token_exp,
            logged_in_at,
        });
    }

    if expires_at > now {
        return Ok(TokenState {
            status: "valid",
            detail: String::new(),
            access_token_exp,
            refresh_token_exp,
            logged_in_at,
        });
    }

    // Access token is expired — check if the refresh token is still usable.
    let refresh_token_valid = refresh_token_exp.is_some_and(|exp| exp > now);

    if refresh_token_valid {
        Ok(TokenState {
            status: "refresh_pending",
            detail: format!(
                "access token expired {} ago, will auto-refresh on next command",
                format_duration(now - expires_at)
            ),
            access_token_exp,
            refresh_token_exp,
            logged_in_at,
        })
    } else {
        Ok(TokenState {
            status: "expired",
            detail: format!(
                "{} ago — run {DIM}longbridge auth login{RESET} to re-authenticate",
                format_duration(now - expires_at)
            ),
            access_token_exp,
            refresh_token_exp,
            logged_in_at,
        })
    }
}

#[derive(Debug, Deserialize)]
struct LevelCenterItem {
    /// Package identifier, e.g. `US_QBBO`, `HK_L1_OpenAPI`. Used as the row key.
    #[serde(default)]
    package_key: String,
    /// Display name, e.g. `LV1 实时行情`.
    #[serde(default)]
    name: String,
    /// Long description.
    #[serde(default)]
    description: String,
    /// Market the package is for: `US`, `HK`, `CN`, `SG`.
    #[serde(default)]
    market: String,
    /// Tags such as `["API"]` for OpenAPI-tagged packages.
    #[serde(default)]
    tags: Vec<String>,
    /// Server-rendered text such as `推广期免费` / `暂无权限`.
    #[serde(default)]
    expired_msg: String,
    /// Unix timestamp string for when the entitlement expires (`"0"` means n/a).
    #[serde(default)]
    end_time: String,
}

#[derive(Debug, Deserialize, Default)]
struct LevelCenterData {
    #[serde(default)]
    activated_lists: Vec<LevelCenterItem>,
    #[serde(default)]
    unactivated_lists: Vec<LevelCenterItem>,
}

/// Format the package `end_time` (Unix timestamp string) as `YYYY-MM-DD`.
/// Returns empty string for `"0"`, missing, or unparseable values.
fn format_pkg_expiry(end_time: &str) -> String {
    let ts: i64 = match end_time.parse() {
        Ok(0) | Err(_) => return String::new(),
        Ok(v) => v,
    };
    OffsetDateTime::from_unix_timestamp(ts)
        .map(|dt| {
            format!(
                "expires {}-{:02}-{:02}",
                dt.year(),
                dt.month() as u8,
                dt.day(),
            )
        })
        .unwrap_or_default()
}

async fn fetch_my_quotes(market: &str, account_channel: &str) -> Option<LevelCenterData> {
    // Backend expects uppercase market codes (`ALL`, `HK`, `US`, `CN`, `SG`).
    let market = market.to_uppercase();
    let client = crate::openapi::http_client();
    let resp = client
        .request(Method::GET, "/v1/quote/my-quotes")
        .query_params(vec![
            ("market", market.as_str()),
            ("account_channel", account_channel),
        ])
        .response::<Json<serde_json::Value>>()
        .send()
        .await
        .ok()?;
    serde_json::from_value(resp.0).ok()
}

struct AccountInfo {
    /// `None` when the underlying quote-scope call fails (e.g. token lacks
    /// quote scope, staging member service down). Other fields still render
    /// independently.
    member_id: Option<i64>,
    level_center: Option<LevelCenterData>,
    account_no: Option<String>,
    account_type: Option<String>,
    name: Option<String>,
    account_channel: Option<String>,
}

/// Fetch `account_no` and `account_type` from the most recent daily statement.
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

    let account_no = mi["AccountNo"]
        .as_str()
        .filter(|s| !s.is_empty())?
        .to_owned();
    let account_type = mi["AccountType"].as_str().unwrap_or("").to_owned();
    let name = mi["NameEn"]
        .as_str()
        .or_else(|| mi["Name"].as_str())
        .unwrap_or("")
        .to_owned();

    Some((account_no, account_type, name))
}

async fn fetch_account_channel_from_positions() -> Option<String> {
    let ctx = crate::openapi::trade();
    let resp = ctx.stock_positions(None).await.ok()?;
    resp.channels.into_iter().next().map(|c| c.account_channel)
}

async fn fetch_account_info(market: &str) -> Result<AccountInfo> {
    let account_channel = crate::auth::account_channel_or_default();
    let (member_id, level_center, statement_info, account_channel) = tokio::join!(
        crate::openapi::quote().member_id(),
        fetch_my_quotes(market, &account_channel),
        fetch_account_info_from_statement(),
        fetch_account_channel_from_positions(),
    );

    let (account_no, account_type, name) = match statement_info {
        Some((no, t, n)) => (
            Some(no),
            Some(t).filter(|s| !s.is_empty()),
            Some(n).filter(|s| !s.is_empty()),
        ),
        None => (None, None, None),
    };

    Ok(AccountInfo {
        member_id: member_id.ok(),
        level_center,
        account_no,
        account_type,
        name,
        account_channel,
    })
}

pub async fn cmd_auth_status(format: &OutputFormat, market: &str) -> Result<()> {
    // ── Token (local) ─────────────────────────────────────────────────────────
    let token_path = crate::auth::token_file_path()?;
    let token = read_token_state()?;

    // ── Connect and fetch account info ────────────────────────────────────────
    let account = match crate::openapi::init_contexts().await {
        Ok(_) => fetch_account_info(market).await.ok(),
        Err(_) => None,
    };

    match format {
        OutputFormat::Json => {
            let mut value = json!({
                "token": {
                    "status": token.status,
                    "logged_in_at": token.logged_in_at,
                    "path": token_path.display().to_string(),
                },
            });

            if let Some(acc) = &account {
                let to_json = |items: &[LevelCenterItem]| -> Vec<serde_json::Value> {
                    items
                        .iter()
                        .map(|p| {
                            json!({
                                "package_key": p.package_key,
                                "name": p.name,
                                "description": p.description,
                                "market": p.market,
                                "tags": p.tags,
                                "expired_msg": p.expired_msg,
                                "end_time": p.end_time,
                            })
                        })
                        .collect()
                };
                let (activated, unactivated) = acc
                    .level_center
                    .as_ref()
                    .map(|lc| (to_json(&lc.activated_lists), to_json(&lc.unactivated_lists)))
                    .unwrap_or_default();
                value["account"] = json!({
                    "member_id": acc.member_id,
                    "activated_packages": activated,
                    "unactivated_packages": unactivated,
                    "account_no": acc.account_no,
                    "account_type": acc.account_type,
                    "account_channel": acc.account_channel,
                    "name": acc.name,
                });
            }

            println!("{}", serde_json::to_string_pretty(&value)?);
        }

        OutputFormat::Pretty => {
            const W: usize = 13; // key column width

            // ── Token ──────────────────────────────────────────────────────────
            let (status_str, status_color) = match token.status {
                "not_found" => ("not found", RED),
                "expired" => ("expired", RED),
                "refresh_pending" => ("refresh pending", YELLOW),
                _ => ("valid", GREEN),
            };
            println!("Token");
            if token.detail.is_empty() {
                println!(
                    "{:<W$} {color}{status_str}{RESET}",
                    "Status",
                    W = W,
                    color = status_color
                );
            } else {
                println!(
                    "{:<W$} {color}{status_str}{RESET}  {}",
                    "Status",
                    token.detail,
                    W = W,
                    color = status_color,
                );
            }
            let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
            let fmt_exp = |ts: u64| -> String {
                let Ok(dt) = OffsetDateTime::from_unix_timestamp(ts.cast_signed())
                    .map(|utc| utc.to_offset(local_offset))
                else {
                    return String::new();
                };
                let now_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let rel = if ts > now_secs {
                    format!("in {}", format_duration(ts - now_secs))
                } else {
                    format!("{} ago", format_duration(now_secs - ts))
                };
                format!(
                    "{}-{:02}-{:02} {:02}:{:02}  {DIM}({}){RESET}",
                    dt.year(),
                    dt.month() as u8,
                    dt.day(),
                    dt.hour(),
                    dt.minute(),
                    rel,
                )
            };
            if let Some(exp) = token.access_token_exp {
                println!("{:<W$} {}", "AccessToken", fmt_exp(exp), W = W);
            }
            if let Some(exp) = token.refresh_token_exp {
                println!("{:<W$} {}", "RefreshToken", fmt_exp(exp), W = W);
            }
            if let Some(ts) = token.logged_in_at {
                if let Ok(dt) = OffsetDateTime::from_unix_timestamp(ts.cast_signed())
                    .map(|utc| utc.to_offset(local_offset))
                {
                    println!(
                        "{:<W$} {}-{:02}-{:02} {:02}:{:02}",
                        "Logged In",
                        dt.year(),
                        dt.month() as u8,
                        dt.day(),
                        dt.hour(),
                        dt.minute(),
                        W = W,
                    );
                }
            }
            let display_path = dirs::home_dir()
                .and_then(|h| {
                    token_path
                        .strip_prefix(&h)
                        .ok()
                        .map(|p| format!("~/{}", p.display()))
                })
                .unwrap_or_else(|| token_path.display().to_string());
            println!("{:<W$} {DIM}{display_path}{RESET}", "Session Path", W = W);

            // ── Account ────────────────────────────────────────────────────────
            if let Some(acc) = &account {
                println!();

                if let Some(name) = &acc.name {
                    println!("{:<W$} {name}", "Name", W = W);
                }
                let mut acct_parts = Vec::new();
                if let Some(no) = &acc.account_no {
                    acct_parts.push(no.as_str());
                }
                if let Some(at) = &acc.account_type {
                    acct_parts.push(at.as_str());
                }
                if !acct_parts.is_empty() {
                    let acct_str = if acct_parts.len() >= 2 {
                        format!("{} · {}", acct_parts[0], acct_parts[1..].join(", "))
                    } else {
                        acct_parts[0].to_string()
                    };
                    println!("{:<W$} {acct_str}", "Account", W = W);
                } else if acc
                    .account_channel
                    .as_deref()
                    .is_some_and(|ch| ch == "lb_papertrading")
                {
                    println!("{:<W$} Paper Trading", "Account", W = W);
                }
                if let Some(mid) = acc.member_id {
                    println!("{:<W$} {}", "Member Id", mid, W = W);
                }

                // ── Quote Level ─────────────────────────────────────────────────
                let empty = LevelCenterData::default();
                let lc = acc.level_center.as_ref().unwrap_or(&empty);

                let print_pkg = |pkg: &LevelCenterItem, active: bool| {
                    // The new `/v1/quote/my-quotes` schema includes a dedicated
                    // `market` field; fall back to splitting the package_key
                    // (`US_QBBO` -> `US`) for resilience.
                    let market = if pkg.market.is_empty() {
                        pkg.package_key.split('_').next().unwrap_or("")
                    } else {
                        pkg.market.as_str()
                    };
                    let tag_suffix = if pkg.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", pkg.tags.join(","))
                    };
                    let expiry = format_pkg_expiry(&pkg.end_time);
                    if active {
                        println!(
                            "  {market}  {GREEN}{}{RESET}{tag_suffix}{}",
                            pkg.name,
                            if expiry.is_empty() {
                                String::new()
                            } else {
                                format!("  ({expiry})")
                            }
                        );
                    } else {
                        let msg = if pkg.expired_msg.is_empty() {
                            String::new()
                        } else {
                            format!("  {DIM}{}{RESET}", pkg.expired_msg)
                        };
                        println!("  {market}  {DIM}{}{RESET}{tag_suffix}{msg}", pkg.name);
                    }
                    let sub = if pkg.description.is_empty() {
                        pkg.package_key.clone()
                    } else {
                        format!("{} · {}", pkg.package_key, pkg.description)
                    };
                    println!("      {DIM}{sub}{RESET}");
                };

                println!();
                println!("{}", t!("my_quote.subscribed"));
                if lc.activated_lists.is_empty() {
                    println!("  {DIM}{}{RESET}", t!("my_quote.no_data"));
                } else {
                    for pkg in &lc.activated_lists {
                        print_pkg(pkg, true);
                    }
                }

                println!();
                println!("{}", t!("my_quote.unsubscribed"));
                if lc.unactivated_lists.is_empty() {
                    println!("  {DIM}{}{RESET}", t!("my_quote.no_data"));
                } else {
                    for pkg in &lc.unactivated_lists {
                        print_pkg(pkg, false);
                    }
                }

                // ── Quote Mall QR code ───────────────────────────────────────────
                println!();
                let channel = crate::auth::account_channel_or_default();
                let _ = super::my_quote::print_mall_qr(&channel);
            }
        }
    }

    Ok(())
}
