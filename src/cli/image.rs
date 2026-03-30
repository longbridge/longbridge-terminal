use std::path::PathBuf;

use anyhow::{bail, Result};
use serde::Deserialize;

/// Upload a local image file to Imgur anonymously and return the public URL.
pub async fn cmd_image_upload(path: PathBuf) -> Result<()> {
    let client_id = "546c25a59c58ad7";

    let bytes = std::fs::read(&path)?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    let mime = mime_from_path(&path);

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)?;
    let form = reqwest::multipart::Form::new().part("image", part);

    let resp = reqwest::Client::new()
        .post("https://api.imgur.com/3/image")
        .header("Authorization", format!("Client-ID {client_id}"))
        .multipart(form)
        .send()
        .await?;

    #[derive(Deserialize)]
    struct ImageData {
        link: String,
    }
    #[derive(Deserialize)]
    struct Response {
        success: bool,
        data: ImageData,
    }

    let body: Response = resp.json().await?;
    if !body.success {
        bail!("Imgur upload failed");
    }

    println!("{}", body.data.link);
    Ok(())
}

fn mime_from_path(path: &PathBuf) -> &'static str {
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
