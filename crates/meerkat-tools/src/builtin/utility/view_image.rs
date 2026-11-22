//! ViewImage tool for reading image files and returning them as multimodal content.

use super::protected_paths::ProtectedPaths;
use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use base64::Engine;
use meerkat_core::types::{ContentBlock, ToolDef, ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Maximum allowed image file size (5 MB).
const MAX_IMAGE_SIZE: u64 = 5 * 1024 * 1024;

/// File extensions under which a file is viewed.
///
/// The extension only admits the name: the media type of the returned block
/// comes from the file's bytes ([`image_media_type_from_signature`]).
const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ViewImageArgs {
    /// Path to the image file to view (relative to project root, or absolute within project).
    path: String,
}

/// Tool for reading image files and returning base64-encoded image content blocks.
///
/// Resolves paths relative to the project root, sandboxes against path traversal,
/// refuses the engine's own directory and the host's declared paths,
/// validates file extension and size, checks that the file's bytes are a PNG,
/// JPEG, GIF or WebP image, then returns a `ContentBlock::Image` whose media
/// type is the one the bytes show.
#[derive(Debug, Clone)]
pub struct ViewImageTool {
    project_root: PathBuf,
    /// Where the tool never reads: the engine's own directory and, when
    /// the host declared them, its state and input paths under
    /// `<project_root>/`.
    protected: ProtectedPaths,
}

impl ViewImageTool {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            protected: ProtectedPaths::default(),
        }
    }

    /// The tool with the host's declared paths protected as well.
    pub(crate) fn protecting(mut self, protected: ProtectedPaths) -> Self {
        self.protected = protected;
        self
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

/// Admit a file extension, returning an error for unsupported ones.
fn ensure_supported_extension(ext: &str) -> Result<(), BuiltinToolError> {
    let ext_lower = ext.to_ascii_lowercase();
    if SUPPORTED_EXTENSIONS.contains(&ext_lower.as_str()) {
        return Ok(());
    }
    Err(BuiltinToolError::invalid_args(format!(
        "unsupported image extension '.{ext}'; supported: {}",
        SUPPORTED_EXTENSIONS.join(", ")
    )))
}

/// The media type of the image format whose signature `bytes` begin with.
///
/// PNG, JPEG, GIF and WebP are told by their leading bytes. Anything else is
/// not an image this tool returns.
fn image_media_type_from_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Some("image/webp")
    } else {
        None
    }
}

#[async_trait]
impl BuiltinTool for ViewImageTool {
    fn name(&self) -> &'static str {
        "view_image"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            audience: Default::default(),
            name: self.name().into(),
            description: "Read an image file from the project and return its contents. Supports PNG, JPEG, GIF, and WebP formats up to 5 MB; a file whose content is not one of these formats is refused.".into(),
            input_schema: crate::schema::schema_for::<ViewImageArgs>(),
            provenance: Some(ToolProvenance { kind: ToolSourceKind::Builtin, source_id: "builtin".into(), artifact: None }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    /// Reads an image path into the transcript.
    fn mutation_class(&self) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::ReadOnly
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: ViewImageArgs = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;

        let user_path = PathBuf::from(&args.path);

        // Resolve and sandbox the path (lexical check first).
        let resolved = resolve_image_path(&self.project_root, &user_path)?;

        // Before the file is touched: the engine's own directory and the
        // host's declared paths are not the project's to read.
        self.protected
            .admit_tool_read(&self.project_root, &resolved, &user_path)
            .await?;

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
        ensure_supported_extension(ext)?;

        // Check file metadata (existence + size).
        let metadata = tokio::fs::metadata(&resolved).await.map_err(|e| {
            BuiltinToolError::execution_failed(format!("cannot read '{}': {e}", resolved.display()))
        })?;

        // Canonicalize to resolve symlinks, then re-check sandbox.
        // This must happen after the existence check (canonicalize requires the file to exist).
        let canonical_root = self.project_root.canonicalize().map_err(|e| {
            BuiltinToolError::execution_failed(format!("cannot resolve project root: {e}"))
        })?;
        let canonical_path = resolved
            .canonicalize()
            .map_err(|e| BuiltinToolError::execution_failed(format!("cannot resolve path: {e}")))?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err(BuiltinToolError::invalid_args(
                "path escapes project root (symlink detected)",
            ));
        }

        if metadata.len() > MAX_IMAGE_SIZE {
            return Err(BuiltinToolError::invalid_args(format!(
                "file size {} bytes exceeds maximum {} bytes",
                metadata.len(),
                MAX_IMAGE_SIZE
            )));
        }

        // Read, check the content and encode.
        let bytes = tokio::fs::read(&resolved).await.map_err(|e| {
            BuiltinToolError::execution_failed(format!(
                "failed to read '{}': {e}",
                resolved.display()
            ))
        })?;

        // The file is returned only when its bytes are an image, and the
        // block's media type is the one the bytes show.
        let media_type = image_media_type_from_signature(&bytes).ok_or_else(|| {
            BuiltinToolError::invalid_args(
                "file content is not a PNG, JPEG, GIF, or WebP image; \
                 view_image returns only image files",
            )
        })?;

        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        Ok(ToolOutput::Blocks(vec![ContentBlock::Image {
            media_type: media_type.to_string(),
            data: meerkat_core::ImageData::Inline { data },
        }]))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
                        let encoded = match data {
                            meerkat_core::ImageData::Inline { data } => data,
                            other => panic!("expected inline image data, got {other:?}"),
                        };
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(encoded)
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
        std::fs::write(&img_path, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();

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
    async fn view_image_returns_inline_image_data() {
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
                ContentBlock::Image { data, .. } => {
                    assert!(matches!(data, meerkat_core::ImageData::Inline { .. }));
                }
                other => panic!("expected Image block, got {other:?}"),
            },
            other => panic!("expected Blocks output, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn view_image_rejects_symlink_escape() {
        let dir = tempdir().unwrap();
        // Create a file outside the project root
        let outside_dir = tempdir().unwrap();
        let outside_img = outside_dir.path().join("secret.png");
        std::fs::write(&outside_img, minimal_png()).unwrap();

        // Create a symlink inside project root pointing outside
        let link_path = dir.path().join("escape.png");
        std::os::unix::fs::symlink(&outside_img, &link_path).unwrap();

        let tool = ViewImageTool::new(dir.path().to_path_buf());
        let result = tool.call(serde_json::json!({"path": "escape.png"})).await;

        match result {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("symlink detected"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error for symlink escape, got {other:?}"),
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

    /// Minimal 1x1 GIF (43 bytes): header, screen descriptor, a two-colour
    /// table, one graphic control extension, one image and the trailer.
    fn minimal_gif() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GIF89a");
        // Logical screen descriptor: 1x1, global colour table of two entries.
        buf.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00]);
        // Global colour table: black, white.
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
        // Graphic control extension.
        buf.extend_from_slice(&[0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00]);
        // Image descriptor: 1x1 at the origin, no local colour table.
        buf.extend_from_slice(&[0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
        // Image data: minimum code size 2, one sub-block, block terminator.
        buf.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
        // Trailer.
        buf.push(0x3B);
        buf
    }

    /// Minimal 1x1 lossless WebP (34 bytes): RIFF container, one VP8L chunk.
    fn minimal_webp() -> Vec<u8> {
        let payload: &[u8] = &[
            0x2F, 0x00, 0x00, 0x00, 0x10, 0x07, 0x10, 0x11, 0x11, 0x88, 0x88, 0xFE, 0x07,
        ];
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"VP8L");
        chunk.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        chunk.extend_from_slice(payload);
        // Chunks are padded to an even length.
        chunk.push(0x00);
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&((4 + chunk.len()) as u32).to_le_bytes());
        buf.extend_from_slice(b"WEBP");
        buf.extend_from_slice(&chunk);
        buf
    }

    /// Minimal 1x1 greyscale baseline JPEG (141 bytes): one quantization
    /// table, one frame, one DC and one AC Huffman table with a single code
    /// each, one scan holding a zero DC difference and an end of block.
    fn minimal_jpeg() -> Vec<u8> {
        let mut buf = Vec::new();
        // Start of image.
        buf.extend_from_slice(&[0xFF, 0xD8]);
        // Quantization table 0: 64 entries of 1.
        buf.extend_from_slice(&[0xFF, 0xDB]);
        buf.extend_from_slice(&67u16.to_be_bytes());
        buf.push(0x00);
        buf.extend_from_slice(&[0x01; 64]);
        // Baseline frame: 8 bits, 1x1, one component using table 0.
        buf.extend_from_slice(&[0xFF, 0xC0]);
        buf.extend_from_slice(&11u16.to_be_bytes());
        buf.extend_from_slice(&[0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00]);
        // Huffman tables (DC 0, AC 0): one code of length 1 for symbol 0.
        for table in [0x00u8, 0x10] {
            buf.extend_from_slice(&[0xFF, 0xC4]);
            buf.extend_from_slice(&20u16.to_be_bytes());
            buf.push(table);
            buf.push(0x01);
            buf.extend_from_slice(&[0x00; 15]);
            buf.push(0x00);
        }
        // Scan: one component, tables 0 and 0, the whole spectrum.
        buf.extend_from_slice(&[0xFF, 0xDA]);
        buf.extend_from_slice(&8u16.to_be_bytes());
        buf.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        // Entropy data: two zero bits, padded with ones.
        buf.push(0x3F);
        // End of image.
        buf.extend_from_slice(&[0xFF, 0xD9]);
        buf
    }

    const PLAIN_TEXT: &[u8] = b"plain text, not an image\n";

    /// Write `bytes` under `name` in a fresh project and view it.
    async fn view_file(name: &str, bytes: &[u8]) -> Result<ToolOutput, BuiltinToolError> {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(name), bytes).unwrap();
        ViewImageTool::new(dir.path().to_path_buf())
            .call(serde_json::json!({ "path": name }))
            .await
    }

    /// The media type and decoded bytes of an output that is one image block.
    fn single_image(output: ToolOutput) -> (String, Vec<u8>) {
        let ToolOutput::Blocks(blocks) = output else {
            panic!("expected Blocks output, got {output:?}");
        };
        assert_eq!(blocks.len(), 1, "expected one block, got {blocks:?}");
        match &blocks[0] {
            ContentBlock::Image {
                media_type,
                data: meerkat_core::ImageData::Inline { data },
            } => (
                media_type.clone(),
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap(),
            ),
            other => panic!("expected an inline Image block, got {other:?}"),
        }
    }

    /// The call returned no block: it was refused because the file's bytes
    /// are not an image.
    fn assert_refused_as_not_an_image(result: Result<ToolOutput, BuiltinToolError>) {
        match result {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("is not a PNG, JPEG, GIF, or WebP image"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error and no block, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn view_image_refuses_text_under_png_name() {
        assert_refused_as_not_an_image(view_file("notes.png", PLAIN_TEXT).await);
    }

    #[tokio::test]
    async fn view_image_refuses_text_under_jpg_name() {
        assert_refused_as_not_an_image(view_file("notes.jpg", PLAIN_TEXT).await);
    }

    #[tokio::test]
    async fn view_image_refuses_text_under_jpeg_name() {
        assert_refused_as_not_an_image(view_file("notes.jpeg", PLAIN_TEXT).await);
    }

    #[tokio::test]
    async fn view_image_refuses_text_under_gif_name() {
        assert_refused_as_not_an_image(view_file("notes.gif", PLAIN_TEXT).await);
    }

    #[tokio::test]
    async fn view_image_refuses_text_under_webp_name() {
        assert_refused_as_not_an_image(view_file("notes.webp", PLAIN_TEXT).await);
    }

    #[tokio::test]
    async fn view_image_refuses_empty_file_under_png_name() {
        assert_refused_as_not_an_image(view_file("empty.png", b"").await);
    }

    #[tokio::test]
    async fn view_image_refuses_text_starting_with_gif_letters() {
        assert_refused_as_not_an_image(view_file("list.gif", b"GIFT list\n").await);
    }

    #[tokio::test]
    async fn view_image_refuses_riff_file_that_is_not_webp() {
        let mut wave = Vec::new();
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&12u32.to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&[0x00; 4]);
        assert_refused_as_not_an_image(view_file("sound.webp", &wave).await);
    }

    #[tokio::test]
    async fn view_image_returns_png_bytes_under_jpg_name_as_png() {
        let output = view_file("renamed.jpg", &minimal_png())
            .await
            .expect("a real image is viewed");
        let (media_type, bytes) = single_image(output);
        assert_eq!(media_type, "image/png");
        assert_eq!(bytes, minimal_png());
    }

    #[tokio::test]
    async fn view_image_returns_real_jpeg() {
        let output = view_file("photo.jpeg", &minimal_jpeg())
            .await
            .expect("a real image is viewed");
        let (media_type, bytes) = single_image(output);
        assert_eq!(media_type, "image/jpeg");
        assert_eq!(bytes, minimal_jpeg());
    }

    #[tokio::test]
    async fn view_image_returns_real_gif() {
        let output = view_file("pixel.gif", &minimal_gif())
            .await
            .expect("a real image is viewed");
        let (media_type, bytes) = single_image(output);
        assert_eq!(media_type, "image/gif");
        assert_eq!(bytes, minimal_gif());
    }

    #[tokio::test]
    async fn view_image_returns_real_webp() {
        let output = view_file("pixel.webp", &minimal_webp())
            .await
            .expect("a real image is viewed");
        let (media_type, bytes) = single_image(output);
        assert_eq!(media_type, "image/webp");
        assert_eq!(bytes, minimal_webp());
    }

    #[tokio::test]
    async fn view_image_refuses_svg_name() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
        match view_file("drawing.svg", svg).await {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("unsupported image extension '.svg'"),
                    "unexpected error message: {msg}"
                );
                assert!(
                    msg.ends_with("supported: png, jpg, jpeg, gif, webp"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error and no block, got {other:?}"),
        }
    }

    #[test]
    fn view_image_description_names_the_returned_formats() {
        let dir = tempdir().unwrap();
        let description = ViewImageTool::new(dir.path().to_path_buf())
            .def()
            .description;
        assert!(
            description.contains("PNG, JPEG, GIF, and WebP"),
            "unexpected description: {description}"
        );
        assert!(
            !description.to_ascii_lowercase().contains("svg"),
            "unexpected description: {description}"
        );
    }

    #[tokio::test]
    async fn view_image_keeps_extension_rule_for_real_png_under_txt_name() {
        match view_file("image.txt", &minimal_png()).await {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("unsupported image extension"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error, got {other:?}"),
        }
    }

    /// The call returned no block: it was refused because the path lies
    /// where the file tools do not read.
    fn assert_refused_as_not_read(result: Result<ToolOutput, BuiltinToolError>, case: &str) {
        match result {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("engine's own directory") && msg.contains("do not read there"),
                    "{case}: unexpected error message: {msg}"
                );
            }
            other => panic!("{case}: expected InvalidArgs error and no block, got {other:?}"),
        }
    }

    /// Write a real PNG at `relative` under `root`, creating its directories.
    fn write_png(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, minimal_png()).unwrap();
    }

    #[tokio::test]
    async fn view_image_refuses_a_file_in_the_engine_directory() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write_png(&root, ".rkat/logo.png");
        write_png(&root, "packages/app/.rkat/assets/logo.png");
        write_png(&root, "docs/.Rkat/logo.png");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let tool = ViewImageTool::new(root.clone());

        let absolute = root.join(".rkat/logo.png");
        for path in [
            ".rkat/logo.png",
            "./.rkat/logo.png",
            "assets/../.rkat/logo.png",
            absolute.to_str().unwrap(),
            "packages/app/.rkat/assets/logo.png",
            "docs/.Rkat/logo.png",
            // A file that does not exist is refused the same way.
            ".rkat/absent.png",
        ] {
            let result = tool.call(serde_json::json!({ "path": path })).await;
            assert_refused_as_not_read(result, path);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn view_image_refuses_a_link_into_the_engine_directory() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write_png(&root, ".rkat/logo.png");
        symlink(root.join(".rkat"), root.join("assets")).unwrap();
        symlink(".rkat/logo.png", root.join("alias.png")).unwrap();
        let tool = ViewImageTool::new(root.clone());

        for path in ["assets/logo.png", "alias.png"] {
            let result = tool.call(serde_json::json!({ "path": path })).await;
            assert_refused_as_not_read(result, path);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn view_image_refuses_a_link_that_leads_nowhere() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        symlink(root.join("missing.png"), root.join("link.png")).unwrap();
        let tool = ViewImageTool::new(root.clone());

        match tool.call(serde_json::json!({ "path": "link.png" })).await {
            Err(BuiltinToolError::InvalidArgs(msg)) => {
                assert!(
                    msg.contains("symbolic link whose target does not exist")
                        && msg.ends_with("do not read through it"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected InvalidArgs error and no block, got {other:?}"),
        }
    }

    /// A refused path answers one sentence whatever stands there: a file,
    /// nothing, a file over the size limit, a name that is no image's, or a
    /// symbolic link that leads nowhere.
    #[cfg(unix)]
    #[tokio::test]
    async fn view_image_answers_a_refused_path_the_same_whatever_stands_there() {
        use super::super::protected_paths::ReadRefusal;
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for directory in [".rkat", "state"] {
            write_png(&root, &format!("{directory}/logo.png"));
            let directory = root.join(directory);
            std::fs::File::create(directory.join("huge.png"))
                .unwrap()
                .set_len(MAX_IMAGE_SIZE + 1)
                .unwrap();
            std::fs::write(directory.join("config.toml"), "kept").unwrap();
            symlink(root.join("missing-target"), directory.join("dangling.png")).unwrap();
        }
        let tool = ViewImageTool::new(root.clone())
            .protecting(ProtectedPaths::with_host_paths([root.join("state")]));

        for directory in [".rkat", "state"] {
            for name in [
                "logo.png",
                "absent.png",
                "huge.png",
                "config.toml",
                "dangling.png",
            ] {
                let path = format!("{directory}/{name}");
                match tool.call(serde_json::json!({ "path": path })).await {
                    Err(BuiltinToolError::InvalidArgs(msg)) => assert_eq!(
                        msg,
                        ReadRefusal::Protected.reason(Path::new(&path)),
                        "{path}"
                    ),
                    other => {
                        panic!("{path}: expected InvalidArgs error and no block, got {other:?}")
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn view_image_returns_a_file_beside_the_engine_directory() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write_png(&root, ".rkat/logo.png");
        let tool = ViewImageTool::new(root.clone());

        for path in ["logo.png", ".rkat.png", "assets/rkat/logo.png"] {
            write_png(&root, path);
            let output = tool
                .call(serde_json::json!({ "path": path }))
                .await
                .unwrap_or_else(|err| panic!("{path}: a project file is viewed, got {err:?}"));
            let (media_type, bytes) = single_image(output);
            assert_eq!(media_type, "image/png", "{path}");
            assert_eq!(bytes, minimal_png(), "{path}");
        }
    }
}
