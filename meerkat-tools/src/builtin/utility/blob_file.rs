//! Blob file bridge tools.

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use base64::Engine;
use meerkat_core::types::{ToolDef, ToolProvenance, ToolSourceKind};
use meerkat_core::{BlobId, BlobStore};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

const MAX_LOAD_FILE_SIZE: u64 = 25 * 1024 * 1024;

const MEDIA_TYPES_BY_EXTENSION: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("txt", "text/plain; charset=utf-8"),
    ("md", "text/markdown; charset=utf-8"),
    ("json", "application/json"),
    ("csv", "text/csv; charset=utf-8"),
    ("pdf", "application/pdf"),
];

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct BlobSaveFileArgs {
    /// Blob ID to save.
    blob_id: String,
    /// Destination path, relative to project root or absolute within the project.
    path: String,
    /// Allow replacing an existing file. Defaults to false.
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct BlobLoadFileArgs {
    /// Source file path, relative to project root or absolute within the project.
    path: String,
    /// Optional media type. Inferred from the file extension when omitted.
    #[serde(default)]
    media_type: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct BlobInspectArgs {
    /// Blob ID to inspect.
    blob_id: String,
}

#[derive(Clone)]
pub struct BlobSaveFileTool {
    project_root: PathBuf,
    blob_store: Arc<dyn BlobStore>,
}

impl BlobSaveFileTool {
    pub fn new(project_root: PathBuf, blob_store: Arc<dyn BlobStore>) -> Self {
        Self {
            project_root,
            blob_store,
        }
    }
}

#[derive(Clone)]
pub struct BlobLoadFileTool {
    project_root: PathBuf,
    blob_store: Arc<dyn BlobStore>,
}

impl BlobLoadFileTool {
    pub fn new(project_root: PathBuf, blob_store: Arc<dyn BlobStore>) -> Self {
        Self {
            project_root,
            blob_store,
        }
    }
}

#[derive(Clone)]
pub struct BlobInspectTool {
    blob_store: Arc<dyn BlobStore>,
}

impl BlobInspectTool {
    pub fn new(blob_store: Arc<dyn BlobStore>) -> Self {
        Self { blob_store }
    }
}

fn resolve_project_path(
    project_root: &Path,
    user_path: &Path,
) -> Result<PathBuf, BuiltinToolError> {
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

async fn canonical_project_root(project_root: &Path) -> Result<PathBuf, BuiltinToolError> {
    tokio::fs::canonicalize(project_root).await.map_err(|err| {
        BuiltinToolError::execution_failed(format!(
            "cannot resolve project root '{}': {err}",
            project_root.display()
        ))
    })
}

fn ensure_under_root(
    canonical_root: &Path,
    canonical_path: &Path,
    original: &Path,
) -> Result<(), BuiltinToolError> {
    if canonical_path.starts_with(canonical_root) {
        Ok(())
    } else {
        Err(BuiltinToolError::invalid_args(format!(
            "path '{}' escapes the project root (symlink detected)",
            original.display()
        )))
    }
}

async fn ensure_existing_file_under_root(
    project_root: &Path,
    resolved: &Path,
    original: &Path,
) -> Result<std::fs::Metadata, BuiltinToolError> {
    let metadata = tokio::fs::metadata(resolved).await.map_err(|err| {
        BuiltinToolError::execution_failed(format!("cannot read '{}': {err}", resolved.display()))
    })?;
    if !metadata.is_file() {
        return Err(BuiltinToolError::invalid_args(format!(
            "'{}' is not a file",
            original.display()
        )));
    }
    let canonical_root = canonical_project_root(project_root).await?;
    let canonical_path = tokio::fs::canonicalize(resolved).await.map_err(|err| {
        BuiltinToolError::execution_failed(format!(
            "cannot resolve '{}': {err}",
            resolved.display()
        ))
    })?;
    ensure_under_root(&canonical_root, &canonical_path, original)?;
    Ok(metadata)
}

async fn ensure_writable_parent_under_root(
    project_root: &Path,
    resolved: &Path,
    original: &Path,
) -> Result<(), BuiltinToolError> {
    let parent = resolved.parent().ok_or_else(|| {
        BuiltinToolError::invalid_args(format!("path '{}' has no parent", original.display()))
    })?;
    let canonical_root = canonical_project_root(project_root).await?;
    let relative_parent = parent.strip_prefix(project_root).map_err(|_| {
        BuiltinToolError::invalid_args(format!(
            "path '{}' escapes the project root",
            original.display()
        ))
    })?;
    let mut current = project_root.to_path_buf();
    for component in relative_parent.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(segment) => current.push(segment),
            _ => {
                return Err(BuiltinToolError::invalid_args(format!(
                    "path '{}' escapes the project root",
                    original.display()
                )));
            }
        }

        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                let canonical_current = tokio::fs::canonicalize(&current).await.map_err(|err| {
                    BuiltinToolError::execution_failed(format!(
                        "cannot resolve '{}': {err}",
                        current.display()
                    ))
                })?;
                ensure_under_root(&canonical_root, &canonical_current, original)?;
                let target_metadata = if metadata.file_type().is_symlink() {
                    tokio::fs::metadata(&current).await.map_err(|err| {
                        BuiltinToolError::execution_failed(format!(
                            "cannot inspect parent directory '{}': {err}",
                            current.display()
                        ))
                    })?
                } else {
                    metadata
                };
                if !target_metadata.is_dir() {
                    return Err(BuiltinToolError::invalid_args(format!(
                        "'{}' is not a directory",
                        current.display()
                    )));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&current).await.map_err(|err| {
                    BuiltinToolError::execution_failed(format!(
                        "failed to create parent directory '{}': {err}",
                        current.display()
                    ))
                })?;
                let canonical_current = tokio::fs::canonicalize(&current).await.map_err(|err| {
                    BuiltinToolError::execution_failed(format!(
                        "cannot resolve '{}': {err}",
                        current.display()
                    ))
                })?;
                ensure_under_root(&canonical_root, &canonical_current, original)?;
            }
            Err(err) => {
                return Err(BuiltinToolError::execution_failed(format!(
                    "cannot inspect parent directory '{}': {err}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

fn infer_media_type(path: &Path) -> Result<String, BuiltinToolError> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| {
            BuiltinToolError::invalid_args(format!(
                "cannot infer media type for '{}'; provide media_type",
                path.display()
            ))
        })?;
    let extension = extension.to_ascii_lowercase();
    MEDIA_TYPES_BY_EXTENSION
        .iter()
        .find(|(ext, _)| *ext == extension)
        .map(|(_, media_type)| (*media_type).to_string())
        .ok_or_else(|| {
            BuiltinToolError::invalid_args(format!(
                "cannot infer media type from extension '.{extension}'; provide media_type"
            ))
        })
}

fn relative_or_absolute_display(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[async_trait]
impl BuiltinTool for BlobSaveFileTool {
    fn name(&self) -> &'static str {
        "blob_save_file"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: "Save blob bytes from the session blob store to a file inside the project root. For generated-image requests, first call generate_image, then call this tool with the returned blob id. Does not transcode formats and never returns raw bytes.".into(),
            input_schema: crate::schema::schema_for::<BlobSaveFileArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "builtin".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: BlobSaveFileArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))?;
        let user_path = PathBuf::from(&args.path);
        let resolved = resolve_project_path(&self.project_root, &user_path)?;
        let payload = self
            .blob_store
            .get(&BlobId::new(args.blob_id.clone()))
            .await
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.data.as_bytes())
            .map_err(|err| {
                BuiltinToolError::execution_failed(format!(
                    "blob '{}' payload is not valid base64: {err}",
                    payload.blob_id
                ))
            })?;

        ensure_writable_parent_under_root(&self.project_root, &resolved, &user_path).await?;
        if tokio::fs::try_exists(&resolved).await.map_err(|err| {
            BuiltinToolError::execution_failed(format!(
                "failed to check destination '{}': {err}",
                resolved.display()
            ))
        })? {
            if !args.overwrite {
                return Err(BuiltinToolError::invalid_args(format!(
                    "destination '{}' already exists; set overwrite=true to replace it",
                    user_path.display()
                )));
            }
            ensure_existing_file_under_root(&self.project_root, &resolved, &user_path).await?;
        }

        let mut options = tokio::fs::OpenOptions::new();
        options.write(true);
        if args.overwrite {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let mut file = options.open(&resolved).await.map_err(|err| {
            BuiltinToolError::execution_failed(format!(
                "failed to open destination '{}': {err}",
                resolved.display()
            ))
        })?;
        file.write_all(&bytes).await.map_err(|err| {
            BuiltinToolError::execution_failed(format!(
                "failed to write destination '{}': {err}",
                resolved.display()
            ))
        })?;
        file.flush().await.map_err(|err| {
            BuiltinToolError::execution_failed(format!(
                "failed to flush destination '{}': {err}",
                resolved.display()
            ))
        })?;

        Ok(ToolOutput::Json(json!({
            "blob_id": payload.blob_id,
            "media_type": payload.media_type,
            "path": relative_or_absolute_display(&self.project_root, &resolved),
            "bytes_written": bytes.len(),
        })))
    }
}

#[async_trait]
impl BuiltinTool for BlobLoadFileTool {
    fn name(&self) -> &'static str {
        "blob_load_file"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: "Read a file from inside the project root and store its bytes in the session blob store. Returns a blob id that other blob-aware tools can use.".into(),
            input_schema: crate::schema::schema_for::<BlobLoadFileArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "builtin".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: BlobLoadFileArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))?;
        let user_path = PathBuf::from(&args.path);
        let resolved = resolve_project_path(&self.project_root, &user_path)?;
        let metadata =
            ensure_existing_file_under_root(&self.project_root, &resolved, &user_path).await?;
        if metadata.len() > MAX_LOAD_FILE_SIZE {
            return Err(BuiltinToolError::invalid_args(format!(
                "file size {} bytes exceeds maximum {} bytes",
                metadata.len(),
                MAX_LOAD_FILE_SIZE
            )));
        }
        let media_type = match args.media_type {
            Some(media_type) => media_type,
            None => infer_media_type(&resolved)?,
        };
        let bytes = tokio::fs::read(&resolved).await.map_err(|err| {
            BuiltinToolError::execution_failed(format!(
                "failed to read '{}': {err}",
                resolved.display()
            ))
        })?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let blob_ref = self
            .blob_store
            .put_artifact(&media_type, &data)
            .await
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
        Ok(ToolOutput::Json(json!({
            "blob_id": blob_ref.blob_id,
            "media_type": blob_ref.media_type,
            "path": relative_or_absolute_display(&self.project_root, &resolved),
            "bytes_read": bytes.len(),
        })))
    }
}

#[async_trait]
impl BuiltinTool for BlobInspectTool {
    fn name(&self) -> &'static str {
        "blob_inspect"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: "Inspect a blob in the session blob store without returning raw bytes. Returns blob id, media type, and decoded byte size.".into(),
            input_schema: crate::schema::schema_for::<BlobInspectArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "builtin".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: BlobInspectArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))?;
        let payload = self
            .blob_store
            .get(&BlobId::new(args.blob_id))
            .await
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
        let size_bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.data.as_bytes())
            .map_err(|err| {
                BuiltinToolError::execution_failed(format!(
                    "blob '{}' payload is not valid base64: {err}",
                    payload.blob_id
                ))
            })?
            .len();
        Ok(ToolOutput::Json(json!({
            "blob_id": payload.blob_id,
            "media_type": payload.media_type,
            "size_bytes": size_bytes,
        })))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{BlobPayload, BlobRef, BlobStoreError};
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct TestBlobStore {
        blobs: Mutex<HashMap<BlobId, BlobPayload>>,
    }

    #[async_trait]
    impl BlobStore for TestBlobStore {
        async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
            let blob_id = BlobId::new(format!("sha256:test-{media_type}-{data}"));
            let payload = BlobPayload {
                blob_id: blob_id.clone(),
                media_type: media_type.to_string(),
                data: data.to_string(),
            };
            self.blobs.lock().await.insert(blob_id.clone(), payload);
            Ok(BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            })
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            self.blobs
                .lock()
                .await
                .get(blob_id)
                .cloned()
                .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
            self.blobs.lock().await.remove(blob_id);
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    fn png_bytes() -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3]
    }

    async fn seeded_store() -> (Arc<dyn BlobStore>, BlobId) {
        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let data = base64::engine::general_purpose::STANDARD.encode(png_bytes());
        let blob_ref = store.put_image("image/png", &data).await.unwrap();
        (store, blob_ref.blob_id)
    }

    #[tokio::test]
    async fn save_file_writes_blob_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let (store, blob_id) = seeded_store().await;
        let tool = BlobSaveFileTool::new(temp.path().to_path_buf(), store);
        let output = tool
            .call(json!({
                "blob_id": blob_id.as_str(),
                "path": "out/top-news-infographic.png"
            }))
            .await
            .unwrap()
            .into_json()
            .unwrap();

        let written = tokio::fs::read(temp.path().join("out/top-news-infographic.png"))
            .await
            .unwrap();
        assert_eq!(
            &written[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        assert_eq!(output["media_type"], "image/png");
        assert_eq!(output["bytes_written"], png_bytes().len());
    }

    #[tokio::test]
    async fn save_file_rejects_overwrite_by_default() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("out.png"), b"old")
            .await
            .unwrap();
        let (store, blob_id) = seeded_store().await;
        let tool = BlobSaveFileTool::new(temp.path().to_path_buf(), store);
        let err = tool
            .call(json!({
                "blob_id": blob_id.as_str(),
                "path": "out.png"
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn save_file_allows_explicit_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("out.png"), b"old")
            .await
            .unwrap();
        let (store, blob_id) = seeded_store().await;
        let tool = BlobSaveFileTool::new(temp.path().to_path_buf(), store);
        tool.call(json!({
            "blob_id": blob_id.as_str(),
            "path": "out.png",
            "overwrite": true
        }))
        .await
        .unwrap();
        let written = tokio::fs::read(temp.path().join("out.png")).await.unwrap();
        assert_eq!(written, png_bytes());
    }

    #[tokio::test]
    async fn save_file_rejects_missing_blob() {
        let temp = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let tool = BlobSaveFileTool::new(temp.path().to_path_buf(), store);
        let err = tool
            .call(json!({
                "blob_id": "sha256:missing",
                "path": "out.png"
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("blob not found"));
    }

    #[tokio::test]
    async fn load_file_stores_blob_with_inferred_media_type() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("source.png"), png_bytes())
            .await
            .unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let tool = BlobLoadFileTool::new(temp.path().to_path_buf(), store.clone());
        let output = tool
            .call(json!({"path": "source.png"}))
            .await
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(output["media_type"], "image/png");
        let blob_id = BlobId::new(output["blob_id"].as_str().unwrap());
        let payload = store.get(&blob_id).await.unwrap();
        assert_eq!(payload.media_type, "image/png");
    }

    #[tokio::test]
    async fn load_file_accepts_explicit_media_type() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("source.bin"), b"abc")
            .await
            .unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let tool = BlobLoadFileTool::new(temp.path().to_path_buf(), store);
        let output = tool
            .call(json!({"path": "source.bin", "media_type": "application/custom"}))
            .await
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(output["media_type"], "application/custom");
        assert_eq!(output["bytes_read"], 3);
    }

    #[tokio::test]
    async fn load_file_requires_media_type_when_extension_is_unknown() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("source.unknown"), b"abc")
            .await
            .unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let tool = BlobLoadFileTool::new(temp.path().to_path_buf(), store);
        let err = tool
            .call(json!({"path": "source.unknown"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cannot infer media type"));
    }

    #[tokio::test]
    async fn inspect_returns_metadata_without_bytes() {
        let (store, blob_id) = seeded_store().await;
        let tool = BlobInspectTool::new(store);
        let output = tool
            .call(json!({"blob_id": blob_id.as_str()}))
            .await
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(output["blob_id"], blob_id.as_str());
        assert_eq!(output["media_type"], "image/png");
        assert_eq!(output["size_bytes"], png_bytes().len());
        assert!(output.get("data").is_none());
    }

    #[tokio::test]
    async fn tools_reject_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let (store, blob_id) = seeded_store().await;
        let save = BlobSaveFileTool::new(temp.path().to_path_buf(), store.clone());
        let save_err = save
            .call(json!({
                "blob_id": blob_id.as_str(),
                "path": "../escape.png"
            }))
            .await
            .unwrap_err();
        assert!(save_err.to_string().contains("escapes the project root"));

        let load = BlobLoadFileTool::new(temp.path().to_path_buf(), store);
        let load_err = load
            .call(json!({"path": "../escape.png"}))
            .await
            .unwrap_err();
        assert!(load_err.to_string().contains("escapes the project root"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tools_reject_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), temp.path().join("link")).unwrap();
        tokio::fs::write(outside.path().join("source.png"), png_bytes())
            .await
            .unwrap();

        let (store, blob_id) = seeded_store().await;
        let save = BlobSaveFileTool::new(temp.path().to_path_buf(), store.clone());
        let save_err = save
            .call(json!({
                "blob_id": blob_id.as_str(),
                "path": "link/sub/out.png"
            }))
            .await
            .unwrap_err();
        assert!(save_err.to_string().contains("symlink detected"));
        assert!(
            !outside.path().join("sub").exists(),
            "save must reject a symlink escape before creating deeper directories"
        );

        let load = BlobLoadFileTool::new(temp.path().to_path_buf(), store);
        let load_err = load
            .call(json!({"path": "link/source.png"}))
            .await
            .unwrap_err();
        assert!(load_err.to_string().contains("symlink detected"));
    }

    #[tokio::test]
    async fn load_file_rejects_oversized_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.bin");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_LOAD_FILE_SIZE + 1).unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let tool = BlobLoadFileTool::new(temp.path().to_path_buf(), store);
        let err = tool
            .call(json!({"path": "large.bin", "media_type": "application/octet-stream"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exceeds maximum"));
    }
}
