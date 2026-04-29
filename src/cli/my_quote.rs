use anyhow::Result;
use qrcode::render::unicode;
use qrcode::QrCode;

pub fn print_mall_qr(_account_channel: &str) -> Result<()> {
    // The renderer adds a 4-char quiet zone on each side (= 4 QR modules).
    // Replace the left quiet zone with 2 spaces so the code stays aligned
    // to the terminal margin while keeping a 2-module quiet zone for scanning.
    const QZ: &str = "    "; // renderer's default left quiet zone
    const INDENT: &str = "  ";

    let url = "https://activity.lbkrs.com/spa/mall";
    let code = QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::L)
        .map_err(|e| anyhow::anyhow!("Failed to generate QR code: {e}"))?;

    let image = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Dark)
        .light_color(unicode::Dense1x2::Light)
        .build();

    let lines: Vec<&str> = image.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .unwrap_or(lines.len() - 1);
    let trimmed = lines[start..=end]
        .iter()
        .map(|l| {
            let body = l.strip_prefix(QZ).unwrap_or(l);
            format!("{INDENT}{body}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    println!("{}", t!("my_quote.qr_title"));
    println!();
    println!("{trimmed}");
    let hint = t!("my_quote.scan_hint");
    for line in hint.lines() {
        println!("{INDENT}{line}");
    }

    Ok(())
}
