//! 用户 prompt 图片的解码、缩放与重编码（借鉴 Codex `utils/image`）。

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use image::{
    ColorType, DynamicImage, GenericImageView, ImageEncoder, ImageFormat,
    codecs::{jpeg::JpegEncoder, png::PngEncoder, webp::WebPEncoder},
    imageops::FilterType,
};
use thiserror::Error;

pub const MAX_WIDTH: u32 = 2048;
pub const MAX_HEIGHT: u32 = 768;

#[derive(Debug, Error)]
pub enum ImageProcessingError {
    #[error("invalid base64 image content")]
    InvalidBase64(#[source] base64::DecodeError),
    #[error("invalid data URL")]
    InvalidDataUrl,
    #[error("failed to decode image")]
    Decode(#[source] image::ImageError),
    #[error("unsupported image format")]
    UnsupportedFormat,
    #[error("failed to encode image as {format:?}")]
    Encode {
        format: ImageFormat,
        source: image::ImageError,
    },
}

#[derive(Debug, Clone)]
pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub mime: String,
}

impl EncodedImage {
    pub fn to_base64(&self) -> String {
        BASE64_STANDARD.encode(&self.bytes)
    }
}

/// 从 base64 字符串或原始字节处理图片。
pub fn encode_from_bytes(
    raw: &[u8],
    media_type_hint: &str,
) -> Result<EncodedImage, ImageProcessingError> {
    let file_bytes = if looks_like_base64_text(raw) {
        BASE64_STANDARD
            .decode(raw)
            .map_err(ImageProcessingError::InvalidBase64)?
    } else {
        raw.to_vec()
    };
    process_image_bytes(&file_bytes, media_type_hint)
}

/// 解析 `data:{mime};base64,{data}` 并处理图片。
pub fn decode_from_data_url(data_url: &str) -> Result<EncodedImage, ImageProcessingError> {
    let payload = data_url
        .strip_prefix("data:")
        .ok_or(ImageProcessingError::InvalidDataUrl)?;
    let (meta, encoded) = payload
        .split_once(',')
        .ok_or(ImageProcessingError::InvalidDataUrl)?;
    if !meta.ends_with(";base64") {
        return Err(ImageProcessingError::InvalidDataUrl);
    }
    let media_type = meta.trim_end_matches(";base64");
    let bytes = BASE64_STANDARD
        .decode(encoded)
        .map_err(ImageProcessingError::InvalidBase64)?;
    process_image_bytes(&bytes, media_type)
}

fn process_image_bytes(
    file_bytes: &[u8],
    media_type_hint: &str,
) -> Result<EncodedImage, ImageProcessingError> {
    let source_format = image::guess_format(file_bytes)
        .ok()
        .filter(|format| {
            matches!(
                format,
                ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
            )
        })
        .or_else(|| mime_to_format(media_type_hint));

    let dynamic = image::load_from_memory(file_bytes).map_err(ImageProcessingError::Decode)?;
    let (width, height) = dynamic.dimensions();

    if width <= MAX_WIDTH && height <= MAX_HEIGHT {
        if let Some(format) = source_format.filter(can_preserve_source_bytes) {
            return Ok(EncodedImage {
                bytes: file_bytes.to_vec(),
                mime: format_to_mime(format),
            });
        }
    }

    let resized = if width <= MAX_WIDTH && height <= MAX_HEIGHT {
        dynamic
    } else {
        dynamic.resize(MAX_WIDTH, MAX_HEIGHT, FilterType::Triangle)
    };
    let target_format = source_format
        .filter(can_preserve_source_bytes)
        .unwrap_or(ImageFormat::Png);
    let (bytes, output_format) = encode_image(&resized, target_format)?;
    Ok(EncodedImage {
        bytes,
        mime: format_to_mime(output_format),
    })
}

fn looks_like_base64_text(raw: &[u8]) -> bool {
    !raw.is_empty()
        && raw.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'\n' | b'\r')
        })
}

fn can_preserve_source_bytes(format: &ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    )
}

fn encode_image(
    image: &DynamicImage,
    preferred_format: ImageFormat,
) -> Result<(Vec<u8>, ImageFormat), ImageProcessingError> {
    let target_format = match preferred_format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::WebP => ImageFormat::WebP,
        _ => ImageFormat::Png,
    };
    let mut buffer = Vec::new();
    match target_format {
        ImageFormat::Png => {
            let rgba = image.to_rgba8();
            PngEncoder::new(&mut buffer)
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    ColorType::Rgba8.into(),
                )
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        },
        ImageFormat::Jpeg => {
            JpegEncoder::new_with_quality(&mut buffer, 85)
                .encode_image(image)
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        },
        ImageFormat::WebP => {
            let rgba = image.to_rgba8();
            WebPEncoder::new_lossless(&mut buffer)
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    ColorType::Rgba8.into(),
                )
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        },
        _ => return Err(ImageProcessingError::UnsupportedFormat),
    }
    Ok((buffer, target_format))
}

fn format_to_mime(format: ImageFormat) -> String {
    match format {
        ImageFormat::Jpeg => "image/jpeg".into(),
        ImageFormat::Gif => "image/gif".into(),
        ImageFormat::WebP => "image/webp".into(),
        _ => "image/png".into(),
    }
}

fn mime_to_format(media_type: &str) -> Option<ImageFormat> {
    match media_type {
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/png" => Some(ImageFormat::Png),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use image::{ImageBuffer, Rgba};

    use super::*;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_pixel(width, height, Rgba([10u8, 20, 30, 255]));
        let mut buffer = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut buffer), ImageFormat::Png)
            .unwrap();
        buffer
    }

    #[test]
    fn decodes_base64_png_content() {
        let raw = png_bytes(64, 32);
        let encoded = BASE64_STANDARD.encode(&raw);
        let result = encode_from_bytes(encoded.as_bytes(), "image/png").unwrap();
        assert_eq!(result.mime, "image/png");
        assert!(!result.to_base64().is_empty());
    }

    #[test]
    fn downscales_large_image() {
        let raw = png_bytes(4096, 2048);
        let result = encode_from_bytes(&raw, "image/png").unwrap();
        let loaded = image::load_from_memory(&result.bytes).unwrap();
        assert!(loaded.width() <= MAX_WIDTH);
        assert!(loaded.height() <= MAX_HEIGHT);
    }
}
