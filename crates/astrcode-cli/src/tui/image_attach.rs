//! TUI 图片附件：从本地路径读取并编码为 protocol [`Attachment`]。

use std::path::Path;

use astrcode_core::user_prompt::image_label;
use astrcode_protocol::commands::Attachment;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn is_image_path(path: &str) -> bool {
    Path::new(path.trim())
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn attachment_from_path(path: &Path) -> Result<Attachment, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("read `{}`: {error}", path.display()))?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image")
        .to_string();
    Ok(Attachment {
        filename: filename.clone(),
        content: BASE64_STANDARD.encode(bytes),
        media_type: mime_from_filename(&filename),
    })
}

pub fn placeholder_for_attachment(index: usize, attachment: &Attachment) -> String {
    image_label(index, &attachment.filename)
}

fn mime_from_filename(filename: &str) -> String {
    match Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "bmp" => "image/bmp".into(),
        _ => "image/png".into(),
    }
}
