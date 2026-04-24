//! Filesystem skill source.
//!
//! Post-V4: the filesystem source constructs `SkillKey` directly from the
//! configured `source_uuid` plus the on-disk directory slug. There is no
//! legacy "slash path" string parsing — the old `parse_legacy_as_key` path
//! is deleted. A filesystem skill is addressed solely by its
//! `(source_uuid, skill_name)` tuple.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillName, SkillScope,
    SkillSource, SourceHealthSnapshot, SourceHealthThresholds, SourceUuid, apply_filter,
};

use crate::parser::parse_skill_md;

/// Filesystem-backed skill source rooted at `root`.
pub struct FilesystemSkillSource {
    root: PathBuf,
    scope: SkillScope,
    source_uuid: SourceUuid,
    #[allow(dead_code)]
    thresholds: SourceHealthThresholds,
    #[allow(dead_code)]
    health: Arc<RwLock<SourceHealthSnapshot>>,
}

impl FilesystemSkillSource {
    pub fn new(root: impl Into<PathBuf>, scope: SkillScope) -> Self {
        Self::new_with_identity(
            root,
            scope,
            SourceUuid::builtin(),
            SourceHealthThresholds::default(),
        )
    }

    pub fn new_with_identity(
        root: impl Into<PathBuf>,
        scope: SkillScope,
        source_uuid: SourceUuid,
        thresholds: SourceHealthThresholds,
    ) -> Self {
        Self {
            root: root.into(),
            scope,
            source_uuid,
            thresholds,
            health: Arc::new(RwLock::new(SourceHealthSnapshot::default())),
        }
    }

    fn discover_skill_directories(base: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(base) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.join("SKILL.md").is_file() {
                    out.push(path);
                } else {
                    out.extend(Self::discover_skill_directories(&path));
                }
            }
        }
        out
    }

    fn skill_key_for_dir(&self, skill_dir: &Path) -> Option<SkillKey> {
        let file_name = skill_dir.file_name()?.to_str()?;
        let skill_name = SkillName::parse(file_name).ok()?;
        Some(SkillKey {
            source_uuid: self.source_uuid.clone(),
            skill_name,
        })
    }
}

impl SkillSource for FilesystemSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let mut all: Vec<SkillDescriptor> = Vec::new();
        for dir in Self::discover_skill_directories(&self.root) {
            let Some(key) = self.skill_key_for_dir(&dir) else {
                continue;
            };
            let content = match std::fs::read_to_string(dir.join("SKILL.md")) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let expected = key.skill_name.as_str().to_string();
            match parse_skill_md(key.clone(), self.scope, &content, Some(&expected)) {
                Ok(doc) => all.push(doc.descriptor),
                Err(err) => {
                    tracing::warn!(?dir, "skipping filesystem skill: {err}");
                }
            }
        }
        Ok(apply_filter(&all, filter))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        if key.source_uuid != self.source_uuid {
            return Err(SkillError::NotFound { key: key.clone() });
        }
        let dir = self.root.join(key.skill_name.as_str());
        if !dir.join("SKILL.md").is_file() {
            return Err(SkillError::NotFound { key: key.clone() });
        }
        let content = std::fs::read_to_string(dir.join("SKILL.md"))
            .map_err(|e| SkillError::Load(format!("read {}: {e}", dir.display()).into()))?;
        parse_skill_md(
            key.clone(),
            self.scope,
            &content,
            Some(key.skill_name.as_str()),
        )
    }

    async fn health_snapshot(&self) -> Result<SourceHealthSnapshot, SkillError> {
        Ok(self.health.read().map(|s| s.clone()).unwrap_or_default())
    }
}
