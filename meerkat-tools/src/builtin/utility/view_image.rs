//! ViewImage tool for reading image files and returning them as multimodal content.

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use base64::Engine;
use meerkat_core::types::{ContentBlock, ToolDef};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Maximum allowed image file size (5 MB).
const MAX_IMAGE_SIZE: u64 = 5 * 1024 * 1024;

/// Supported image extensions and their MIME types.
const SUPPORTED_EXTENSIONS: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
];

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ViewImageArgs {
    /// Path to the image file to view (relative to project root, or absolute within project).
    path: String,
}

/// Tool for reading image files and returning base64-encoded image content blocks.
///
/// Resolves paths relative to the project root, sandboxes against path traversal,
/// validates file extension and size, then returns a `ContentBlock::Image`.
#[derive(Debug, Clone)]
pub struct ViewImageTool {
    project_root: PathBuf,
}

impl ViewImageTool {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

/// Resolve a user-supplied path against the project root, rejecting escapes.
///
/// Mirrors the sandboxing logic in `apply_patch::resolve_patch_path`.
fn resolve_image_path(project_root: &Path, user_path: &Path) -> Result<PathBuf, BuiltinToolError> {
    let mut resolved = if user_path.is_absolute() {
        PathBuf::new()
    } else {
        project_root.to_path_buf()
    };

    for component in user_path.components() {
        match component {
            Component::Prefix(_) => {
                return Err(BuiltinToolError::invalid_args(format!(
                    "unsupported path prefix '{}'",
                    user_path.display()
                )));
            }
            Component::RootDir => resolved = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(segment) => resolved.push(segment),
        }
    }

    if !resolved.starts_with(project_root) {
        return Err(BuiltinToolError::invalid_args(format!(
            "path '{}' escapes the project root",
            user_path.display()
        )));
    }

    Ok(resolved)
}

/// Look up the MIME type for a file extension, returning an error for unsupported types.
fn media_type_for_extension(ext: &str) -> Result<&'static str, BuiltinToolError> {
    let ext_lower = ext.to_ascii_lowercase();
    SUPPORTED_EXTENSIONS
        .iter()
        .find(|(e, _)| *e == ext_lower)
        .map(|(_, mime)| *mime)
        .ok_or_else(|| {
            let supported: Vec<&str> = SUPPORTED_EXTENSIONS.iter().map(|(e, _)| *e).collect();
            BuiltinToolError::invalid_args(format!(
                "unsupported image extension '.{ext}'; supported: {}",
                supported.join(", ")
            ))
        })
}

#[async_trait]
impl BuiltinTool for ViewImageTool {
    fn name(&self) -> &'static str {
        "view_image"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: "Read an image file from the project and return its contents. Supports PNG, JPEG, GIF, WebP, and SVG formats up to 5 MB.".into(),
            input_schema: crate::schema::schema_for::<ViewImageArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: ViewImageArgs = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;

        let user_path = PathBuf::from(&args.path);

        // Resolve and sandbox the path.
        let resolved = resolve_image_path(&self.project_root, &user_path)?;

        // Validate extension.
        let ext = resolved
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| {
                BuiltinToolError::invalid_args(format!(
                    "file '{}' has no extension",
                    resolved.display()
                ))
            })?;
        let media_type = media_type_for_extension(ext)?;

        // Check file metadata (existence + size).
        let metadata = tokio::fs::metadata(&resolved).await.map_err(|e| {
            BuiltinToolError::execution_failed(format!("cannot read '{}': {e}", resolved.display()))
        })?;

        if metadata.len() > MAX_IMAGE_SIZE {
            return Err(BuiltinToolError::invalid_args(format!(
                "file size {} bytes exceeds maximum {} bytes",
                metadata.len(),
                MAX_IMAGE_SIZE
            )));
        }

        // Read and encode.
        let bytes = tokio::fs::read(&resolved).await.map_err(|e| {
            BuiltinToolError::execution_failed(format!(
                "failed to read '{}': {e}",
                resolved.display()
            ))
        })?;

        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        Ok(ToolOutput::Blocks(vec![ContentBlock::Image {
            media_type: media_type.to_string(),
            data,
            source_path: Some(resolved.to_string_lossy().into_owned()),
        }]))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Minimal valid 1x1 PNG (67 bytes).
    fn minimal_png() -> Vec<u8> {
        // 8-byte signature + IHDR + IDAT + IEND
        let mut buf = Vec::new();
        // PNG signature
        buf.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR chunk: length=13, type, data, CRC
        let ihdr_data: [u8; 13] = [
            0, 0, 0, 1, // width=1
            0, 0, 0, 1, // height=1
            8, // bit depth
            2, // color type (RGB)
            0, // compression
            0, // filter
            0, // interlace
        ];
        buf.extend_from_slice(&(13u32).to_be_bytes());
        buf.extend_from_slice(b"IHDR");
        buf.extend_from_slice(&ihdr_data);
        let crc = crc32(&[b"IHDR", &ihdr_data[..]].concat());
        buf.extend_from_slice(&crc.to_be_bytes());
        // IDAT chunk (deflate of a single row: filter=0, R, G, B)
        let idat_payload: &[u8] = &[
            0x78, 0x01, 0x62, 0x60, 0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01,
        ];
        buf.extend_from_slice(&(idat_payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(b"IDAT");
        buf.extend_from_slice(idat_payload);
        let crc = crc32(&[b"IDAT", idat_payload].concat());
        buf.extend_from_slice(&crc.to_be_bytes());
        // IEND chunk
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(b"IEND");
        let crc = crc32(b"IEND");
        buf.extend_from_slice(&crc.to_be_bytes());
        buf
    }

    /// Simple CRC-32 for PNG chunks (IEEE polynomial).
    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB8_8320;
                } else {
                    crc >>= 1;
                }
            }
        }
        !crc
    }

    #[tokio::test]
    async fn view_image_reads_png() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("test.png");
        std::fs::write(&img_path, minimal_png()).unwrap();

        let tool = ViewImageTool::new(dir.path().to_path_buf());
        let output = tool
            .call(serde_json::json!({"path": "test.png"}))
            .await
            .expect("should succeed");

        match output {
            ToolOutput::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::Image {
                        media_type, data, ..
                    } => {
                        assert_eq!(media_type, "image/png");
                        // Verify base64 round-trips
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap();
                        assert_eq!(decoded, minimal_png());
                    }
                    other => panic!("expected Image block, got {other:?}"),
                }
            }
            other => panic!("expected Blocks output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn view_image_reads_jpeg() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("photo.jpg");
        // JPEG files start with 0xFF 0xD8; write minimal marker bytes.
        std::fs::write(&img_path, &[0xFF, 0xD8, 0xFF, 0xD9]).unwrap();

        let tool = ViewImageTool::new(dir.path().to_path_buf());
        let output = tool
            .call(serde_json::json!({"path": "photo.jpg"}))
            .await
            .expect("should succeed");

        match output {
            ToolOutput::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::Image { media_type, .. } => {
                        assert_eq!(media_type, "image/jpeg");
                    }
                    other => panic!("expected Image block, got {other:?}"),
                }
            }
            other => panic!("expected Blocks output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn view_image_rejects_path_escape() {
        let dir = tempdir().unwrap();
        let tool = ViewImageTool::new(dir.path().to_path_buf());

        let result = tool
            .call(serde_json::json!({"path": "../../../etc/passwd.png"}))
            .await;

        match result {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("escapes the project root"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn view_image_rejects_unsupported_extension() {
        let dir = tempdir().unwrap();
        let txt_path = dir.path().join("readme.txt");
        std::fs::write(&txt_path, "hello").unwrap();

        let tool = ViewImageTool::new(dir.path().to_path_buf());
        let result = tool.call(serde_json::json!({"path": "readme.txt"})).await;

        match result {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("unsupported image extension"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn view_image_rejects_oversized_file() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("huge.png");
        // Write just over 5 MB
        let data = vec![0u8; (MAX_IMAGE_SIZE + 1) as usize];
        std::fs::write(&img_path, data).unwrap();

        let tool = ViewImageTool::new(dir.path().to_path_buf());
        let result = tool.call(serde_json::json!({"path": "huge.png"})).await;

        match result {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("exceeds maximum"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn view_image_includes_source_path() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("icon.png");
        std::fs::write(&img_path, minimal_png()).unwrap();

        let tool = ViewImageTool::new(dir.path().to_path_buf());
        let output = tool
            .call(serde_json::json!({"path": "icon.png"}))
            .await
            .expect("should succeed");

        match output {
            ToolOutput::Blocks(blocks) => match &blocks[0] {
                ContentBlock::Image { source_path, .. } => {
                    let sp = source_path.as_deref().expect("source_path should be set");
                    assert!(
                        sp.ends_with("icon.png"),
                        "source_path should end with icon.png, got: {sp}"
                    );
                    assert!(
                        PathBuf::from(sp).is_absolute(),
                        "source_path should be absolute, got: {sp}"
                    );
                }
                other => panic!("expected Image block, got {other:?}"),
            },
            other => panic!("expected Blocks output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn view_image_nonexistent_file_errors() {
        let dir = tempdir().unwrap();
        let tool = ViewImageTool::new(dir.path().to_path_buf());

        let result = tool
            .call(serde_json::json!({"path": "does_not_exist.png"}))
            .await;

        match result {
            Err(BuiltinToolError::ExecutionFailed(msg)) => {
                assert!(
                    msg.contains("cannot read"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected ExecutionFailed error, got {other:?}"),
        }
    }
}
