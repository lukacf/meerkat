//! Filesystem skill source.

use async_trait::async_trait;
use meerkat_core::skills::{SkillDescriptor, SkillDocument, SkillError, SkillId, SkillScope, SkillSource};
use std::path::PathBuf;

/// Skill source that reads from a filesystem directory.
///
/// Each skill is a subdirectory containing a `SKILL.md` file.
pub struct FilesystemSkillSource {
    root: PathBuf,
    scope: SkillScope,
}

impl FilesystemSkillSource {
    pub fn new(root: PathBuf, scope: SkillScope) -> Self {
        Self { root, scope }
    }
}

#[async_trait]
impl SkillSource for FilesystemSkillSource {
    async fn list(&self) -> Result<Vec<SkillDescriptor>, SkillError> {
        let mut descriptors = Vec::new();

        let mut entries = match tokio::fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(_) => return Ok(Vec::new()), // Directory doesn't exist — no skills
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_file = path.join("SKILL.md");
            if !tokio::fs::try_exists(&skill_file).await.unwrap_or(false) {
                continue;
            }

            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let id = SkillId(dir_name.to_string());

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

        Ok(descriptors)
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        let skill_dir = self.root.join(&id.0);
        let skill_file = skill_dir.join("SKILL.md");

        let content = tokio::fs::read_to_string(&skill_file)
            .await
            .map_err(|e| SkillError::Load(format!("failed to read {}: {e}", skill_file.display()).into()))?;

        crate::parser::parse_skill_md(id.clone(), self.scope, &content)
    }
}
