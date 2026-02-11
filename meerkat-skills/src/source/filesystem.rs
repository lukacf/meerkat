//! Filesystem skill source.
//!
//! Scans a directory tree recursively for `SKILL.md` files.
//! Derives namespaced skill IDs from relative paths.

use async_trait::async_trait;
use meerkat_core::skills::{
    SkillCollection, SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillScope,
    SkillSource, apply_filter,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Skill source that reads from a filesystem directory tree.
///
/// Scans recursively for subdirectories containing `SKILL.md` files.
/// Derives namespaced skill IDs from the relative path within the root.
///
/// Example: root = `.rkat/skills/`, file at `extraction/email/SKILL.md`
/// produces skill ID `"extraction/email"`.
pub struct FilesystemSkillSource {
    root: PathBuf,
    scope: SkillScope,
}

impl FilesystemSkillSource {
    pub fn new(root: PathBuf, scope: SkillScope) -> Self {
        Self { root, scope }
    }
}

/// Recursively find all `SKILL.md` files under `dir`.
/// Returns pairs of (relative_path_from_root, absolute_skill_md_path).
async fn find_skill_files(root: &Path, dir: &Path) -> Vec<(String, PathBuf)> {
    let mut results = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&current).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let is_dir = entry
                .file_type()
                .await
                .map(|ft| ft.is_dir())
                .unwrap_or(false);
            if !is_dir {
                continue;
            }
            let path = entry.path();

            let skill_file = path.join("SKILL.md");
            if tokio::fs::try_exists(&skill_file).await.unwrap_or(false) {
                // This directory has a SKILL.md — it's a skill
                if let Ok(relative) = path.strip_prefix(root) {
                    if let Some(rel_str) = relative.to_str() {
                        // Normalize path separators to /
                        let id = rel_str.replace(std::path::MAIN_SEPARATOR, "/");
                        results.push((id, skill_file));
                    } else {
                        tracing::warn!(
                            "Skipping non-UTF-8 skill directory: {}",
                            path.display()
                        );
                    }
                }
            } else {
                // No SKILL.md here — might contain subdirectories with skills
                stack.push(path);
            }
        }
    }

    results
}

/// Load collection descriptions from `COLLECTION.md` files.
/// Returns a map of collection path → description.
async fn load_collection_descriptions(root: &Path) -> BTreeMap<String, String> {
    let mut descriptions = BTreeMap::new();
    load_collection_descriptions_recursive(root, root, &mut descriptions).await;
    descriptions
}

async fn load_collection_descriptions_recursive(
    root: &Path,
    dir: &Path,
    descriptions: &mut BTreeMap<String, String>,
) {
    let collection_file = dir.join("COLLECTION.md");
    if tokio::fs::try_exists(&collection_file)
        .await
        .unwrap_or(false)
    {
        if let Ok(content) = tokio::fs::read_to_string(&collection_file).await {
            if let Ok(relative) = dir.strip_prefix(root) {
                if let Some(rel_str) = relative.to_str() {
                    let path = rel_str.replace(std::path::MAIN_SEPARATOR, "/");
                    if !path.is_empty() {
                        descriptions.insert(path, content.trim().to_string());
                    }
                }
            }
        }
    }

    // Recurse into subdirectories
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let is_dir = entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false);
        if is_dir {
            let path = entry.path();
            Box::pin(load_collection_descriptions_recursive(
                root,
                &path,
                descriptions,
            ))
            .await;
        }
    }
}

#[async_trait]
impl SkillSource for FilesystemSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        if !tokio::fs::try_exists(&self.root).await.unwrap_or(false) {
            return Ok(Vec::new());
        }

        let skill_files = find_skill_files(&self.root, &self.root).await;
        let mut descriptors = Vec::new();

        for (id_str, skill_file) in skill_files {
            let id = SkillId(id_str);

            match tokio::fs::read_to_string(&skill_file).await {
                Ok(content) => {
                    match crate::parser::parse_skill_md(id, self.scope, &content) {
                        Ok(doc) => descriptors.push(doc.descriptor),
                        Err(e) => {
                            tracing::warn!("Failed to parse {}: {e}", skill_file.display());
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read {}: {e}", skill_file.display());
                }
            }
        }

        Ok(apply_filter(&descriptors, filter))
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        let skill_dir = self.root.join(&id.0);
        let skill_file = skill_dir.join("SKILL.md");

        let content = tokio::fs::read_to_string(&skill_file)
            .await
            .map_err(|e| {
                SkillError::Load(
                    format!("failed to read {}: {e}", skill_file.display()).into(),
                )
            })?;

        crate::parser::parse_skill_md(id.clone(), self.scope, &content)
    }

    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
        // Get the default derived collections
        let all = self.list(&SkillFilter::default()).await?;
        let mut collections = meerkat_core::skills::derive_collections(&all);

        // Override descriptions from COLLECTION.md files
        let descriptions = load_collection_descriptions(&self.root).await;
        for collection in &mut collections {
            if let Some(desc) = descriptions.get(&collection.path) {
                collection.description = desc.clone();
            }
        }

        Ok(collections)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a SKILL.md file with valid frontmatter.
    async fn create_skill(root: &Path, rel_path: &str, name: &str) {
        let dir = root.join(rel_path);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let content = format!(
            "---\nname: {name}\ndescription: Test skill {name}\n---\n\n# {name}\n\nBody."
        );
        tokio::fs::write(dir.join("SKILL.md"), content)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_recursive_scan_nested_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        create_skill(&root, "extraction/email", "email").await;
        create_skill(&root, "extraction/fiction", "fiction").await;

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let skills = source.list(&SkillFilter::default()).await.unwrap();

        assert_eq!(skills.len(), 2);
        let ids: Vec<&str> = skills.iter().map(|s| s.id.0.as_str()).collect();
        assert!(ids.contains(&"extraction/email"));
        assert!(ids.contains(&"extraction/fiction"));
    }

    #[tokio::test]
    async fn test_recursive_scan_deep_nesting() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        create_skill(&root, "a/b/c", "deep-skill").await;

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let skills = source.list(&SkillFilter::default()).await.unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "a/b/c");
    }

    #[tokio::test]
    async fn test_collection_md_loading() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        // Create collection with COLLECTION.md
        let coll_dir = root.join("extraction");
        tokio::fs::create_dir_all(&coll_dir).await.unwrap();
        tokio::fs::write(
            coll_dir.join("COLLECTION.md"),
            "Entity and relationship extraction",
        )
        .await
        .unwrap();

        create_skill(&root, "extraction/email", "email").await;

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let collections = source.collections().await.unwrap();

        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].path, "extraction");
        assert_eq!(
            collections[0].description,
            "Entity and relationship extraction"
        );
    }

    #[tokio::test]
    async fn test_collection_md_missing_fallback() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        // No COLLECTION.md — should auto-generate description
        create_skill(&root, "extraction/email", "email").await;

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let collections = source.collections().await.unwrap();

        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].path, "extraction");
        assert_eq!(collections[0].description, "1 skill");
    }

    #[tokio::test]
    async fn test_root_level_skill() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        create_skill(&root, "my-skill", "my-skill").await;

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let skills = source.list(&SkillFilter::default()).await.unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "my-skill");
        assert!(skills[0].id.collection().is_none());
    }

    #[tokio::test]
    async fn test_list_with_collection_filter() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        create_skill(&root, "extraction/email", "email").await;
        create_skill(&root, "formatting/markdown", "markdown").await;

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let skills = source
            .list(&SkillFilter {
                collection: Some("extraction".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "extraction/email");
    }
}
