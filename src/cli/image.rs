use std::path::PathBuf;

use anyhow::{bail, Result};
use base64::Engine as _;
use clap::ValueEnum;
use serde::Deserialize;

const IMGBB_API_KEY: &str = "12000efab18c07f904a6b105c7e61f61";

#[derive(ValueEnum, Clone, Debug)]
pub enum ImageProvider {
    Imgur,
    Imgbb,
}

pub async fn cmd_image_upload(path: PathBuf, provider: Option<ImageProvider>) -> Result<()> {
    let bytes = std::fs::read(&path)?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    let mime = mime_from_path(&path);
    let client = reqwest::Client::new();

    if let Some(p) = provider {
        let url = match p {
            ImageProvider::Imgur => upload_imgur(&client, bytes, filename, mime).await?,
            ImageProvider::Imgbb => upload_imgbb(&client, bytes).await?,
        };
        println!("{url}");
        return Ok(());
    }

    match upload_imgur(&client, bytes.clone(), filename.clone(), mime).await {
        Ok(url) => {
            println!("{url}");
            return Ok(());
        }
        Err(e) => eprintln!("Imgur failed ({e}), trying imgbb..."),
    }

    match upload_imgbb(&client, bytes).await {
        Ok(url) => println!("{url}"),
        Err(e) => bail!("imgbb upload also failed ({e}). All upload attempts failed."),
    }
    Ok(())
}

async fn upload_imgur(
    client: &reqwest::Client,
    bytes: Vec<u8>,
    filename: String,
    mime: &'static str,
) -> Result<String> {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)?;
    let form = reqwest::multipart::Form::new().part("image", part);

    let resp = client
        .post("https://api.imgur.com/3/image")
        .header("Authorization", "Client-ID 546c25a59c58ad7")
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
        bail!("upload rejected");
    }
    Ok(body.data.link)
}

async fn upload_imgbb(client: &reqwest::Client, bytes: Vec<u8>) -> Result<String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let resp = client
        .post("https://api.imgbb.com/1/upload")
        .form(&[("key", IMGBB_API_KEY), ("image", &b64)])
        .send()
        .await?;

    #[derive(Deserialize)]
    struct ImageData {
        url: String,
    }
    #[derive(Deserialize)]
    struct Response {
        success: bool,
        data: ImageData,
    }

    let body: Response = resp.json().await?;
    if !body.success {
        bail!("upload rejected");
    }
    Ok(body.data.url)
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
