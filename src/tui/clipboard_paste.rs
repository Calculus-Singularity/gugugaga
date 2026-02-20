//! Clipboard helpers for image paste support.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub enum PasteImageError {
    ClipboardUnavailable(String),
    NoImage(String),
    EncodeFailed(String),
    IoError(String),
}

impl std::fmt::Display for PasteImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasteImageError::ClipboardUnavailable(msg) => write!(f, "clipboard unavailable: {msg}"),
            PasteImageError::NoImage(msg) => write!(f, "no image on clipboard: {msg}"),
            PasteImageError::EncodeFailed(msg) => write!(f, "could not encode image: {msg}"),
            PasteImageError::IoError(msg) => write!(f, "io error: {msg}"),
        }
    }
}

impl std::error::Error for PasteImageError {}

#[derive(Debug, Clone)]
pub struct PastedImageInfo {
    pub width: u32,
    pub height: u32,
}

/// Read image data from clipboard and persist it as a temporary PNG file.
pub fn paste_image_to_temp_png() -> Result<(PathBuf, PastedImageInfo), PasteImageError> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| PasteImageError::ClipboardUnavailable(e.to_string()))?;

    let image = cb
        .get_image()
        .map_err(|e| PasteImageError::NoImage(e.to_string()))?;

    let width = image.width as u32;
    let height = image.height as u32;
    let Some(rgba_img) = image::RgbaImage::from_raw(width, height, image.bytes.into_owned()) else {
        return Err(PasteImageError::EncodeFailed(
            "invalid RGBA image buffer".to_string(),
        ));
    };

    let dyn_img = image::DynamicImage::ImageRgba8(rgba_img);
    let mut png = Vec::<u8>::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png);
        dyn_img
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| PasteImageError::EncodeFailed(e.to_string()))?;
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = format!("gugugaga-clipboard-{}-{stamp}.png", std::process::id());
    let path = std::env::temp_dir().join(file_name);
    std::fs::write(&path, png).map_err(|e| PasteImageError::IoError(e.to_string()))?;

    Ok((path, PastedImageInfo { width, height }))
}
