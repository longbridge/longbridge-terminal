use std::path::{Path, PathBuf};

use anyhow::Result;
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct UploadUrlRequest {
    original_file_name: String,
    content_type: &'static str,
}

#[derive(Deserialize)]
struct UploadUrlResponse {
    signed_url: String,
    url: String,
    content_type: String,
}

pub async fn cmd_image_upload(path: PathBuf) -> Result<()> {
    let bytes = std::fs::read(&path)?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    let mime = mime_from_path(&path);

    // Step 1: get presigned upload URL from Longbridge API
    let client = crate::openapi::http_client();
    let resp = client
        .request(Method::POST, "/v1/content/upload_url")
        .body(longbridge::httpclient::Json(UploadUrlRequest {
            original_file_name: filename,
            content_type: mime,
        }))
        .response::<longbridge::httpclient::Json<UploadUrlResponse>>()
        .send()
        .await?
        .0;

    // Step 2: PUT the image bytes to the presigned URL
    reqwest::Client::new()
        .put(&resp.signed_url)
        .header("Content-Type", resp.content_type)
        .body(bytes)
        .send()
        .await?
        .error_for_status()?;

    println!("{}", resp.url);
    Ok(())
}

fn mime_from_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}
