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
    classify_source_health,
};

use crate::parser::parse_skill_md;

/// Filesystem-backed skill source rooted at `root`.
pub struct FilesystemSkillSource {
    root: PathBuf,
    scope: SkillScope,
    source_uuid: SourceUuid,
    thresholds: SourceHealthThresholds,
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

    /// Record the latest scan's invalid/total counts into the typed
    /// source-health snapshot, classified via the shared thresholds. A lock
    /// poison is non-fatal for diagnostics, so it is left to surface on the
    /// next successful write.
    fn record_health(&self, invalid_count: u32, total_count: u32) {
        let invalid_ratio = if total_count == 0 {
            0.0
        } else {
            invalid_count as f32 / total_count as f32
        };
        let snapshot = SourceHealthSnapshot {
            state: classify_source_health(invalid_ratio, 0, false, self.thresholds),
            invalid_ratio,
            invalid_count,
            total_count,
            failure_streak: 0,
            handshake_failed: false,
        };
        if let Ok(mut health) = self.health.write() {
            *health = snapshot;
        }
    }
}

impl SkillSource for FilesystemSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let mut all: Vec<SkillDescriptor> = Vec::new();
        let mut invalid_count: u32 = 0;
        let mut total_count: u32 = 0;
        for dir in Self::discover_skill_directories(&self.root) {
            total_count = total_count.saturating_add(1);
            let Some(key) = self.skill_key_for_dir(&dir) else {
                // Directory name is not a valid skill slug — record it as
                // invalid source-health rather than silently dropping it.
                invalid_count = invalid_count.saturating_add(1);
                tracing::warn!(?dir, "skipping filesystem skill: invalid directory slug");
                continue;
            };
            let content = match std::fs::read_to_string(dir.join("SKILL.md")) {
                Ok(s) => s,
                Err(err) => {
                    invalid_count = invalid_count.saturating_add(1);
                    tracing::warn!(?dir, "skipping unreadable filesystem skill: {err}");
                    continue;
                }
            };
            let expected = key.skill_name.as_str().to_string();
            match parse_skill_md(key.clone(), self.scope, &content, Some(&expected)) {
                Ok(doc) => all.push(doc.descriptor),
                Err(err) => {
                    invalid_count = invalid_count.saturating_add(1);
                    tracing::warn!(?dir, "skipping filesystem skill: {err}");
                }
            }
        }

        self.record_health(invalid_count, total_count);
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::SourceHealthState;

    fn write_skill(root: &std::path::Path, dir: &str, contents: &str) {
        let skill_dir = root.join(dir);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();
    }

    #[tokio::test]
    async fn list_records_invalid_skill_in_source_health() {
        // Row #220: a directory with an invalid SKILL.md must surface in typed
        // source-health (invalid_count==1, non-Healthy) rather than being
        // silently filtered out of the inventory with a Default snapshot.
        let tmp = tempfile::tempdir().unwrap();
        let source_uuid = SourceUuid::builtin();

        // One valid skill, one with broken frontmatter (missing delimiter).
        write_skill(
            tmp.path(),
            "alpha",
            "---\nname: alpha\ndescription: valid alpha\n---\nbody\n",
        );
        write_skill(tmp.path(), "broken", "no frontmatter here at all");

        let source = FilesystemSkillSource::new_with_identity(
            tmp.path().to_path_buf(),
            SkillScope::Project,
            source_uuid,
            SourceHealthThresholds::default(),
        );

        let listed = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(listed.len(), 1, "only the valid skill is listed");
        assert_eq!(listed[0].key.skill_name.as_str(), "alpha");

        let health = source.health_snapshot().await.unwrap();
        assert_eq!(health.invalid_count, 1, "the broken SKILL.md must count");
        assert_eq!(health.total_count, 2);
        assert_ne!(
            health.state,
            SourceHealthState::Healthy,
            "an invalid skill must degrade source health, not stay Default-Healthy"
        );
    }
}
