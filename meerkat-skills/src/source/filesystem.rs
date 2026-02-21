//! Filesystem skill source.
//!
//! Scans a directory tree recursively for `SKILL.md` files.
//! Derives namespaced skill IDs from relative paths.

use meerkat_core::skills::{
    SkillCollection, SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId,
    SkillQuarantineDiagnostic, SkillScope, SkillSource, SourceHealthSnapshot, SourceHealthState,
    SourceHealthThresholds, SourceUuid, apply_filter, classify_source_health,
};
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

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
    source_uuid: SourceUuid,
    thresholds: SourceHealthThresholds,
    state: Arc<RwLock<FilesystemSourceState>>,
}

impl FilesystemSkillSource {
    pub fn new(root: PathBuf, scope: SkillScope) -> Self {
        let source_uuid = default_source_uuid(scope);
        Self::new_with_identity(root, scope, source_uuid, SourceHealthThresholds::default())
    }

    pub fn new_with_identity(
        root: PathBuf,
        scope: SkillScope,
        source_uuid: SourceUuid,
        thresholds: SourceHealthThresholds,
    ) -> Self {
        Self {
            root,
            scope,
            source_uuid,
            thresholds,
            state: Arc::new(RwLock::new(FilesystemSourceState::default())),
        }
    }
}

#[derive(Debug, Default)]
struct FilesystemSourceState {
    diagnostics: BTreeMap<String, SkillQuarantineDiagnostic>,
    invalid_count: u32,
    total_count: u32,
    failure_streak: u32,
    handshake_failed: bool,
}

impl FilesystemSourceState {
    fn health_snapshot(&self, thresholds: SourceHealthThresholds) -> SourceHealthSnapshot {
        let invalid_ratio = if self.total_count == 0 {
            0.0
        } else {
            self.invalid_count as f32 / self.total_count as f32
        };
        let state = classify_source_health(
            invalid_ratio,
            self.failure_streak,
            self.handshake_failed,
            thresholds,
        );
        SourceHealthSnapshot {
            state,
            invalid_ratio,
            invalid_count: self.invalid_count,
            total_count: self.total_count,
            failure_streak: self.failure_streak,
            handshake_failed: self.handshake_failed,
        }
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_error_code(error: &SkillError) -> (&'static str, &'static str) {
    match error {
        SkillError::Parse(_) => ("parse_error", "parse"),
        SkillError::Load(_) => ("load_error", "load"),
        SkillError::NotFound { .. } => ("not_found", "load"),
        SkillError::CapabilityUnavailable { .. } => ("capability_unavailable", "policy"),
        SkillError::Ambiguous { .. } => ("ambiguous_ref", "resolver"),
        SkillError::SourceUuidCollision { .. } => ("identity_collision", "identity"),
        SkillError::SourceUuidMutationWithoutLineage { .. } => ("identity_mutation", "identity"),
        SkillError::MissingSkillRemaps { .. } => ("identity_missing_remaps", "identity"),
        SkillError::RemapWithoutLineage { .. } => ("identity_remap", "identity"),
        SkillError::InvalidLegacySkillRefFormat { .. } => ("invalid_legacy_ref", "resolver"),
        SkillError::UnknownSkillAlias { .. } => ("unknown_alias", "resolver"),
        SkillError::RemapCycle { .. } => ("remap_cycle", "identity"),
    }
}

fn expected_slug_for_id(id: &SkillId) -> &str {
    id.skill_name()
}

fn record_failure(
    state: &mut FilesystemSourceState,
    source_uuid: &SourceUuid,
    id: SkillId,
    location: String,
    error: &SkillError,
) {
    let now = now_unix_secs();
    let (error_code, error_class) = parse_error_code(error);
    if let Some(existing) = state.diagnostics.get_mut(&id.0) {
        existing.last_seen_unix_secs = now;
        existing.message = error.to_string();
        existing.error_code = error_code.to_string();
        existing.error_class = error_class.to_string();
        existing.location = location;
    } else {
        state.diagnostics.insert(
            id.0.clone(),
            SkillQuarantineDiagnostic {
                source_uuid: source_uuid.clone(),
                skill_id: id,
                location,
                error_code: error_code.to_string(),
                error_class: error_class.to_string(),
                message: error.to_string(),
                first_seen_unix_secs: now,
                last_seen_unix_secs: now,
            },
        );
    }
}

fn default_source_uuid(scope: SkillScope) -> SourceUuid {
    let raw = match scope {
        SkillScope::Project => "00000000-0000-4000-8000-000000000101",
        SkillScope::User => "00000000-0000-4000-8000-000000000102",
        SkillScope::Builtin => "00000000-0000-4000-8000-000000000103",
    };
    SourceUuid::parse(raw).expect("default filesystem source UUID must be valid")
}

/// Recursively find all `SKILL.md` files under `dir`.
/// Returns pairs of (relative_path_from_root, absolute_skill_md_path).
/// Recursively find `SKILL.md` files under `dir`, returning `(id, path)` pairs.
///
/// Directories containing `SKILL.md` are treated as leaf skill directories —
/// their subdirectories are NOT recursed into. This means a skill cannot
/// contain nested skills. Only directories without `SKILL.md` are explored
/// further (they act as collection/namespace directories).
async fn find_skill_files(root: &Path, dir: &Path) -> Vec<(String, PathBuf)> {
    let mut results = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&current).await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("Failed to read skill directory {}: {e}", current.display());
                continue;
            }
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
                        tracing::warn!("Skipping non-UTF-8 skill directory: {}", path.display());
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
        match tokio::fs::read_to_string(&collection_file).await {
            Ok(content) => {
                if let Ok(relative) = dir.strip_prefix(root)
                    && let Some(rel_str) = relative.to_str()
                {
                    let path = rel_str.replace(std::path::MAIN_SEPARATOR, "/");
                    if !path.is_empty() {
                        descriptions.insert(path, content.trim().to_string());
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Failed to read {}: {e}", collection_file.display());
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

impl SkillSource for FilesystemSkillSource {
    fn list(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
        async move {
            if !tokio::fs::try_exists(&self.root).await.unwrap_or(false) {
                return Ok(Vec::new());
            }

            let skill_files = find_skill_files(&self.root, &self.root).await;
            let mut descriptors = Vec::new();
            let mut total_count: u32 = 0;
            let mut invalid_count: u32 = 0;
            let mut had_failures = false;
            let mut current_failures: HashSet<SkillId> = HashSet::new();

            for (id_str, skill_file) in skill_files {
                total_count += 1;
                let id = SkillId(id_str);

                match tokio::fs::read_to_string(&skill_file).await {
                    Ok(content) => match crate::parser::parse_skill_md(
                        id.clone(),
                        self.scope,
                        &content,
                        Some(expected_slug_for_id(&id)),
                    ) {
                        Ok(doc) => descriptors.push(doc.descriptor),
                        Err(e) => {
                            invalid_count += 1;
                            had_failures = true;
                            current_failures.insert(id.clone());
                            tracing::warn!("Failed to parse {}: {e}", skill_file.display());
                            if let Ok(mut guard) = self.state.write() {
                                record_failure(
                                    &mut guard,
                                    &self.source_uuid,
                                    id,
                                    skill_file.display().to_string(),
                                    &e,
                                );
                            }
                        }
                    },
                    Err(e) => {
                        invalid_count += 1;
                        had_failures = true;
                        current_failures.insert(id.clone());
                        tracing::warn!("Failed to read {}: {e}", skill_file.display());
                        if let Ok(mut guard) = self.state.write() {
                            let load_error = SkillError::Load(
                                format!("failed to read {}: {e}", skill_file.display()).into(),
                            );
                            record_failure(
                                &mut guard,
                                &self.source_uuid,
                                id,
                                skill_file.display().to_string(),
                                &load_error,
                            );
                        }
                    }
                }
            }

            if let Ok(mut guard) = self.state.write() {
                guard.total_count = total_count;
                guard.invalid_count = invalid_count;
                guard.failure_streak = if had_failures {
                    guard.failure_streak.saturating_add(1)
                } else {
                    0
                };
                guard
                    .diagnostics
                    .retain(|skill_id, _| current_failures.iter().any(|id| id.0 == *skill_id));

                let snapshot = guard.health_snapshot(self.thresholds);
                match snapshot.state {
                    SourceHealthState::Healthy => {}
                    SourceHealthState::Degraded => tracing::warn!(
                        root = %self.root.display(),
                        invalid_ratio = snapshot.invalid_ratio,
                        failure_streak = snapshot.failure_streak,
                        "filesystem skill source is degraded"
                    ),
                    SourceHealthState::Unhealthy => tracing::error!(
                        root = %self.root.display(),
                        invalid_ratio = snapshot.invalid_ratio,
                        failure_streak = snapshot.failure_streak,
                        "filesystem skill source is unhealthy; serving only valid skills"
                    ),
                }
            }

            Ok(apply_filter(&descriptors, filter))
        }
    }

    fn load(&self, id: &SkillId) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        async move {
            let skill_dir = self.root.join(&id.0);
            let canonical = skill_dir
                .canonicalize()
                .or_else(|_| {
                    skill_dir
                        .parent()
                        .map(|p| {
                            p.canonicalize()
                                .map(|c| c.join(skill_dir.file_name().unwrap_or_default()))
                        })
                        .unwrap_or(Err(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "cannot resolve",
                        )))
                })
                .unwrap_or_else(|_| skill_dir.clone());
            let root_canonical = self
                .root
                .canonicalize()
                .unwrap_or_else(|_| self.root.clone());
            if !canonical.starts_with(&root_canonical) {
                return Err(SkillError::Load(
                    format!("skill ID '{}' resolves outside skills root", id.0).into(),
                ));
            }
            let skill_file = skill_dir.join("SKILL.md");

            let content = tokio::fs::read_to_string(&skill_file).await.map_err(|e| {
                let error =
                    SkillError::Load(format!("failed to read {}: {e}", skill_file.display()).into());
                if let Ok(mut guard) = self.state.write() {
                    guard.failure_streak = guard.failure_streak.saturating_add(1);
                    record_failure(
                        &mut guard,
                        &self.source_uuid,
                        id.clone(),
                        skill_file.display().to_string(),
                        &error,
                    );
                    let snapshot = guard.health_snapshot(self.thresholds);
                    if snapshot.state == SourceHealthState::Unhealthy {
                        return SkillError::Load(
                            format!(
                                "source '{}' unhealthy during load failure (invalid_ratio={:.3}, streak={})",
                                self.root.display(),
                                snapshot.invalid_ratio,
                                snapshot.failure_streak
                            )
                            .into(),
                        );
                    }
                }
                error
            })?;

            match crate::parser::parse_skill_md(
                id.clone(),
                self.scope,
                &content,
                Some(expected_slug_for_id(id)),
            ) {
                Ok(doc) => {
                    if let Ok(mut guard) = self.state.write() {
                        guard.failure_streak = 0;
                        guard.diagnostics.remove(&id.0);
                    }
                    Ok(doc)
                }
                Err(error) => {
                    if let Ok(mut guard) = self.state.write() {
                        guard.failure_streak = guard.failure_streak.saturating_add(1);
                        record_failure(
                            &mut guard,
                            &self.source_uuid,
                            id.clone(),
                            skill_file.display().to_string(),
                            &error,
                        );
                        let snapshot = guard.health_snapshot(self.thresholds);
                        if snapshot.state == SourceHealthState::Unhealthy {
                            return Err(SkillError::Load(
                                format!(
                                    "source '{}' unhealthy during parse failure (invalid_ratio={:.3}, streak={})",
                                    self.root.display(),
                                    snapshot.invalid_ratio,
                                    snapshot.failure_streak
                                )
                                .into(),
                            ));
                        }
                    }
                    Err(error)
                }
            }
        }
    }

    fn collections(&self) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
        async move {
            let all = self.list(&SkillFilter::default()).await?;
            let mut collections = meerkat_core::skills::derive_collections(&all);
            let descriptions = load_collection_descriptions(&self.root).await;
            for collection in &mut collections {
                if let Some(desc) = descriptions.get(&collection.path) {
                    collection.description = desc.clone();
                }
            }
            Ok(collections)
        }
    }

    fn quarantined_diagnostics(
        &self,
    ) -> impl Future<Output = Result<Vec<SkillQuarantineDiagnostic>, SkillError>> + Send {
        async move {
            let guard = self
                .state
                .read()
                .map_err(|_| SkillError::Load("filesystem skill source lock poisoned".into()))?;
            Ok(guard.diagnostics.values().cloned().collect())
        }
    }

    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<SourceHealthSnapshot, SkillError>> + Send {
        async move {
            let guard = self
                .state
                .read()
                .map_err(|_| SkillError::Load("filesystem skill source lock poisoned".into()))?;
            Ok(guard.health_snapshot(self.thresholds))
        }
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
        let content =
            format!("---\nname: {name}\ndescription: Test skill {name}\n---\n\n# {name}\n\nBody.");
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

        create_skill(&root, "a/b/c", "c").await;

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let skills = source.list(&SkillFilter::default()).await.unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "a/b/c");
    }

    #[tokio::test]
    async fn test_invalid_skill_is_quarantined_but_valid_skills_are_listed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        create_skill(&root, "extraction/email", "email").await;
        tokio::fs::create_dir_all(root.join("broken/skill"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("broken/skill/SKILL.md"),
            "---\nname: wrong-name\ndescription: bad\n---\n\nbody",
        )
        .await
        .unwrap();

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "extraction/email");

        let diagnostics = source.quarantined_diagnostics().await.unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].skill_id.0, "broken/skill");
        assert_eq!(
            diagnostics[0].source_uuid.to_string(),
            "00000000-0000-4000-8000-000000000101"
        );
        assert_eq!(diagnostics[0].error_class, "parse");
        assert_eq!(diagnostics[0].error_code, "parse_error");
    }

    #[tokio::test]
    async fn test_quarantine_diagnostics_retain_first_seen_and_update_last_seen() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        tokio::fs::create_dir_all(root.join("broken/skill"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("broken/skill/SKILL.md"),
            "---\nname: wrong-name\ndescription: bad\n---\n\nbody",
        )
        .await
        .unwrap();

        let source = FilesystemSkillSource::new(root.clone(), SkillScope::Project);
        let _ = source.list(&SkillFilter::default()).await.unwrap();
        let first = source.quarantined_diagnostics().await.unwrap();
        assert_eq!(first.len(), 1);
        let first_diag = &first[0];
        assert_eq!(first_diag.skill_id.0, "broken/skill");
        assert_eq!(
            first_diag.source_uuid.to_string(),
            "00000000-0000-4000-8000-000000000101"
        );
        assert_eq!(first_diag.error_class, "parse");
        assert!(first_diag.location.ends_with("broken/skill/SKILL.md"));

        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        let _ = source.list(&SkillFilter::default()).await.unwrap();
        let second = source.quarantined_diagnostics().await.unwrap();
        assert_eq!(second.len(), 1);
        let second_diag = &second[0];
        assert_eq!(
            second_diag.first_seen_unix_secs,
            first_diag.first_seen_unix_secs
        );
        assert!(second_diag.last_seen_unix_secs > first_diag.last_seen_unix_secs);
    }

    #[tokio::test]
    async fn test_health_snapshot_transitions_with_invalid_ratio() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        create_skill(&root, "skills/alpha", "alpha").await;
        create_skill(&root, "skills/beta", "beta").await;
        tokio::fs::create_dir_all(root.join("skills/broken"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("skills/broken/SKILL.md"),
            "---\nname: not-broken\ndescription: bad\n---\n\nbody",
        )
        .await
        .unwrap();

        let source = FilesystemSkillSource::new(root, SkillScope::Project);
        let _ = source.list(&SkillFilter::default()).await.unwrap();
        let snapshot = source.health_snapshot().await.unwrap();

        assert_eq!(snapshot.total_count, 3);
        assert_eq!(snapshot.invalid_count, 1);
        assert_eq!(snapshot.state, SourceHealthState::Degraded);
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
