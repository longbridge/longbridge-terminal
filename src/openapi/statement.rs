//! PDF statement endpoints (`/v1/statement/pdf/*`).
//!
//! Statements issued before the JSON statement service exist only as
//! password-protected PDFs. These endpoints list them and return a presigned
//! download URL together with the PDF password. The SDK has no wrapper for
//! them yet, so they are called through the shared authenticated HTTP client.

use anyhow::{Context, Result};
use longbridge::httpclient::{Json, Method};
use serde::{Deserialize, Serialize};

/// One PDF statement as listed by `GET /v1/statement/pdf/list`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PdfStatementItem {
    /// Display label, e.g. `2024.03` (monthly) or `2024.03.19` (daily).
    pub display_name: String,
    /// Opaque file key for `pdf_download`, ends with `.pdf`.
    pub key: String,
    #[serde(default)]
    pub cache_key: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PdfStatementList {
    #[serde(default)]
    items: Vec<PdfStatementItem>,
}

/// Response of `GET /v1/statement/pdf/download`.
///
/// The PDF is password-protected; the password is not part of the response
/// but follows a fixed rule, see [`PDF_PASSWORD_RULE`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PdfDownload {
    /// Presigned URL of the PDF file.
    pub url: String,
    #[serde(default)]
    pub cache_key: String,
}

/// How to unlock a downloaded PDF statement, shown after every download.
pub const PDF_PASSWORD_RULE: &str = "The PDF is password-protected. Password: the last 4 digits \
    of the mobile number used for the account + the last 4 characters of the ID used for account \
    opening, uppercase letters and digits only (drop brackets and other symbols). Example: mobile \
    12345678 and ID 123456(X) give 5678456X.";

/// Statement kind as the PDF endpoints encode it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfStatementKind {
    Daily = 0,
    Monthly = 1,
}

/// Largest page the endpoint accepts.
const PAGE_SIZE: u32 = 100;

/// `GET /v1/statement/pdf/list` — every PDF statement of `kind` whose month
/// falls in `start_month..=end_month` (both `yyyyMM`), following pagination.
pub async fn pdf_statements(
    kind: PdfStatementKind,
    start_month: &str,
    end_month: &str,
) -> Result<Vec<PdfStatementItem>> {
    let mut all = Vec::new();
    let mut page = 1u32;
    loop {
        let items = pdf_statements_page(kind, start_month, end_month, page).await?;
        let last = items.len() < PAGE_SIZE as usize;
        all.extend(items);
        if last {
            return Ok(all);
        }
        page += 1;
    }
}

async fn pdf_statements_page(
    kind: PdfStatementKind,
    start_month: &str,
    end_month: &str,
    page: u32,
) -> Result<Vec<PdfStatementItem>> {
    #[derive(Serialize)]
    struct Query<'a> {
        kind: i32,
        start_dt: &'a str,
        end_dt: &'a str,
        page: u32,
        size: u32,
    }
    let start_month = start_month.to_string();
    let end_month = end_month.to_string();
    let resp = crate::openapi::global_rate_limiter()
        .execute("statement_pdf_list", || {
            let start_month = start_month.clone();
            let end_month = end_month.clone();
            Box::pin(async move {
                crate::openapi::http_client()
                    .request(Method::GET, "/v1/statement/pdf/list")
                    .query_params(Query {
                        kind: kind as i32,
                        start_dt: &start_month,
                        end_dt: &end_month,
                        page,
                        size: PAGE_SIZE,
                    })
                    .response::<Json<PdfStatementList>>()
                    .send()
                    .await
                    .map(|json| json.0)
            })
        })
        .await
        .context("Failed to list PDF statements")?;
    Ok(resp.items)
}

/// `GET /v1/statement/pdf/download` — presigned URL and password for `key`.
pub async fn pdf_download(key: &str) -> Result<PdfDownload> {
    #[derive(Serialize)]
    struct Query<'a> {
        key: &'a str,
    }
    let key = key.to_string();
    let resp = crate::openapi::global_rate_limiter()
        .execute("statement_pdf_download", || {
            let key = key.clone();
            Box::pin(async move {
                crate::openapi::http_client()
                    .request(Method::GET, "/v1/statement/pdf/download")
                    .query_params(Query { key: &key })
                    .response::<Json<PdfDownload>>()
                    .send()
                    .await
                    .map(|json| json.0)
            })
        })
        .await
        .context("Failed to get PDF statement download URL")?;
    Ok(resp)
}
