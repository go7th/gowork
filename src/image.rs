use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::path::Path;

/// Detect MIME type from file extension
fn mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/png",
    }
}

/// Read an image file and encode as a data URL
pub fn image_to_data_url(path: &str) -> Result<String> {
    let p = Path::new(path);
    let bytes = std::fs::read(p)
        .with_context(|| format!("Failed to read image: {}", path))?;
    let mime = mime_type(p);
    let encoded = STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, encoded))
}

/// Parse user input for @image references.
/// Returns (cleaned_text, list_of_image_paths).
/// Syntax: "@./screenshot.png" or "@/abs/path/img.jpg"
pub fn parse_image_refs(input: &str) -> (String, Vec<String>) {
    let mut images = Vec::new();
    let mut cleaned_parts: Vec<String> = Vec::new();

    for token in input.split_whitespace() {
        if let Some(rest) = token.strip_prefix('@') {
            // Check if it looks like an image path
            let lower = rest.to_lowercase();
            if lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".gif")
                || lower.ends_with(".webp")
                || lower.ends_with(".bmp")
            {
                if Path::new(rest).exists() {
                    images.push(rest.to_string());
                    continue;
                }
            }
        }
        cleaned_parts.push(token.to_string());
    }

    (cleaned_parts.join(" "), images)
}
