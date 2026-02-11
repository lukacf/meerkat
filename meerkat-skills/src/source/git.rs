//! Git-backed skill source.
//!
//! Clones/pulls a git repo into a local cache directory, then delegates
//! to `FilesystemSkillSource` for parsing and collection derivation.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_core::skills::{
    SkillCollection, SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillScope,
    SkillSource,
};
use tokio::sync::RwLock;

use super::FilesystemSkillSource;

/// Git ref to track.
#[derive(Debug, Clone)]
pub enum GitRef {
    /// Track a branch (pulls latest on refresh). Default: "main".
    Branch(String),
    /// Pin to a tag (immutable — no refresh needed after initial clone).
    Tag(String),
    /// Pin to an exact commit SHA (immutable — no refresh needed).
    Commit(String),
}

impl Default for GitRef {
    fn default() -> Self {
        GitRef::Branch("main".into())
    }
}

impl GitRef {
    fn ref_str(&self) -> &str {
        match self {
            GitRef::Branch(s) | GitRef::Tag(s) | GitRef::Commit(s) => s,
        }
    }

    fn is_immutable(&self) -> bool {
        matches!(self, GitRef::Tag(_) | GitRef::Commit(_))
    }
}

/// Authentication for git operations.
#[derive(Debug, Clone)]
pub enum GitSkillAuth {
    /// HTTPS token (used as password with empty username).
    HttpsToken(String),
}

/// Configuration for a git skill source.
#[derive(Debug, Clone)]
pub struct GitSkillConfig {
    /// Remote repository URL.
    pub repo_url: String,
    /// Branch, tag, or commit SHA to track.
    pub git_ref: GitRef,
    /// Local directory for the clone.
    pub cache_dir: PathBuf,
    /// Subdirectory within the repo to scan for skills.
    pub skills_root: Option<String>,
    /// How often to pull for updates (for Branch refs).
    pub refresh_interval: Duration,
    /// Authentication for private repos.
    pub auth: Option<GitSkillAuth>,
    /// Shallow clone depth. Default: Some(1).
    pub depth: Option<usize>,
}

impl GitSkillConfig {
    pub fn new(repo_url: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            repo_url: repo_url.into(),
            git_ref: GitRef::default(),
            cache_dir: cache_dir.into(),
            skills_root: None,
            refresh_interval: Duration::from_secs(300),
            auth: None,
            depth: Some(1),
        }
    }
}

/// Git-backed skill source.
///
/// Clones a git repo on first access, then delegates to `FilesystemSkillSource`.
/// For Branch refs, pulls on TTL expiry. For Tag/Commit refs, no refresh.
pub struct GitSkillSource {
    config: GitSkillConfig,
    inner: RwLock<Option<FilesystemSkillSource>>,
    last_sync: RwLock<Option<Instant>>,
}

impl GitSkillSource {
    pub fn new(config: GitSkillConfig) -> Self {
        Self {
            config,
            inner: RwLock::new(None),
            last_sync: RwLock::new(None),
        }
    }

    /// Ensure the repo is cloned and up to date. Lazy init.
    async fn ensure_synced(&self) -> Result<(), SkillError> {
        // Check if we have a fresh clone
        {
            let last = self.last_sync.read().await;
            if let Some(synced_at) = *last {
                if self.config.git_ref.is_immutable() {
                    return Ok(()); // Immutable ref — never refresh
                }
                if synced_at.elapsed() < self.config.refresh_interval {
                    return Ok(()); // Still fresh
                }
            }
        }

        // Need to sync
        let repo_dir = &self.config.cache_dir;

        if repo_dir.join(".git").exists() {
            // Already cloned — pull if branch
            if !self.config.git_ref.is_immutable() {
                if let Err(e) = self.git_pull().await {
                    // Pull failed — serve stale if we have data
                    let has_data = self.inner.read().await.is_some();
                    if has_data {
                        tracing::warn!(
                            "Git pull failed for '{}', serving stale cache: {e}",
                            self.config.repo_url
                        );
                        let mut last = self.last_sync.write().await;
                        *last = Some(Instant::now()); // Reset TTL to avoid hammering
                        return Ok(());
                    }
                    return Err(e);
                }
            }
        } else {
            // First access — clone
            self.git_clone().await?;
        }

        // Rebuild the inner FilesystemSkillSource
        let skills_dir = match &self.config.skills_root {
            Some(root) => repo_dir.join(root),
            None => repo_dir.clone(),
        };

        let source = FilesystemSkillSource::new(skills_dir, SkillScope::Project);
        *self.inner.write().await = Some(source);
        *self.last_sync.write().await = Some(Instant::now());

        Ok(())
    }

    async fn git_clone(&self) -> Result<(), SkillError> {
        let mut args = vec![
            "clone".to_string(),
            "--single-branch".to_string(),
            "--branch".to_string(),
            self.config.git_ref.ref_str().to_string(),
        ];

        if let Some(depth) = self.config.depth {
            args.push("--depth".into());
            args.push(depth.to_string());
        }

        let url = self.auth_url();
        args.push(url);
        args.push(
            self.config
                .cache_dir
                .to_str()
                .ok_or_else(|| SkillError::Load("non-UTF-8 cache dir path".into()))?
                .to_string(),
        );

        run_git(&args).await.map_err(|e| {
            SkillError::Load(
                format!("git clone failed for '{}': {e}", self.config.repo_url).into(),
            )
        })
    }

    async fn git_pull(&self) -> Result<(), SkillError> {
        let args = vec!["pull".to_string(), "--ff-only".to_string()];

        run_git_in_dir(&self.config.cache_dir, &args).await.map_err(|e| {
            SkillError::Load(
                format!("git pull failed for '{}': {e}", self.config.repo_url).into(),
            )
        })
    }

    fn auth_url(&self) -> String {
        match &self.config.auth {
            Some(GitSkillAuth::HttpsToken(token)) => {
                // Insert token into URL: https://token@host/path
                if let Some(rest) = self.config.repo_url.strip_prefix("https://") {
                    format!("https://x-access-token:{token}@{rest}")
                } else {
                    self.config.repo_url.clone()
                }
            }
            None => self.config.repo_url.clone(),
        }
    }

}

/// Run a git command and check for success.
async fn run_git(args: &[String]) -> Result<(), String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to spawn git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git exited with {}: {stderr}", output.status));
    }
    Ok(())
}

/// Run a git command in a specific directory.
async fn run_git_in_dir(dir: &Path, args: &[String]) -> Result<(), String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to spawn git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git exited with {}: {stderr}", output.status));
    }
    Ok(())
}

#[async_trait]
impl SkillSource for GitSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        self.ensure_synced().await?;
        let guard = self.inner.read().await;
        let source = guard
            .as_ref()
            .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
        source.list(filter).await
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        self.ensure_synced().await?;
        let guard = self.inner.read().await;
        let source = guard
            .as_ref()
            .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
        source.load(id).await
    }

    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
        self.ensure_synced().await?;
        let guard = self.inner.read().await;
        let source = guard
            .as_ref()
            .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
        source.collections().await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a local bare git repo with skill files for testing.
    async fn create_test_repo(tmp: &Path) -> PathBuf {
        let repo_dir = tmp.join("test-repo");
        let work_dir = tmp.join("work");

        // Init bare repo
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        run_git(&[
            "init".into(),
            "--bare".into(),
            repo_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();

        // Clone it to a work dir, add files, push
        run_git(&[
            "clone".into(),
            repo_dir.to_str().unwrap().into(),
            work_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();

        // Create skill files
        let skill_dir = work_dir.join("extraction/email");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: email\ndescription: Extract from emails\n---\n\n# Email\n\nBody.",
        )
        .await
        .unwrap();

        // Create COLLECTION.md
        let coll_dir = work_dir.join("extraction");
        tokio::fs::write(
            coll_dir.join("COLLECTION.md"),
            "Entity extraction skills",
        )
        .await
        .unwrap();

        // Git add, commit, push
        run_git_in_dir(&work_dir, &["add".into(), "-A".into()])
            .await
            .unwrap();
        run_git_in_dir(
            &work_dir,
            &[
                "commit".into(),
                "-m".into(),
                "Initial skills".into(),
                "--author".into(),
                "Test <test@test.com>".into(),
            ],
        )
        .await
        .unwrap();
        run_git_in_dir(&work_dir, &["push".into()])
            .await
            .unwrap();

        repo_dir
    }

    #[tokio::test]
    async fn test_lazy_no_clone_on_construction() {
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().join("cache");

        let _source = GitSkillSource::new(GitSkillConfig::new(
            "https://nonexistent.example.com/repo.git",
            &cache_dir,
        ));

        // Cache dir should not exist — no clone on construction
        assert!(!cache_dir.exists());
    }

    #[tokio::test]
    async fn test_clone_failure_returns_error() {
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().join("cache");

        let source = GitSkillSource::new(GitSkillConfig::new(
            "https://nonexistent.example.com/repo.git",
            &cache_dir,
        ));

        let result = source.list(&SkillFilter::default()).await;
        assert!(matches!(result, Err(SkillError::Load(_))));
    }

    #[tokio::test]
    async fn test_clone_on_first_access() {
        let tmp = TempDir::new().unwrap();
        let repo_path = create_test_repo(tmp.path()).await;
        let cache_dir = tmp.path().join("clone-cache");

        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: format!("file://{}", repo_path.display()),
            cache_dir: cache_dir.clone(),
            git_ref: GitRef::Branch("main".into()),
            depth: None, // file:// protocol doesn't support shallow clone well
            ..GitSkillConfig::new("", "")
        });

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(!skills.is_empty());
        assert!(cache_dir.exists()); // Clone happened
    }

    #[tokio::test]
    async fn test_namespaced_ids_from_repo_structure() {
        let tmp = TempDir::new().unwrap();
        let repo_path = create_test_repo(tmp.path()).await;
        let cache_dir = tmp.path().join("ns-cache");

        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: format!("file://{}", repo_path.display()),
            cache_dir,
            git_ref: GitRef::Branch("main".into()),
            depth: None,
            ..GitSkillConfig::new("", "")
        });

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(skills.iter().any(|s| s.id.0 == "extraction/email"));
    }

    #[tokio::test]
    async fn test_collection_md_from_repo() {
        let tmp = TempDir::new().unwrap();
        let repo_path = create_test_repo(tmp.path()).await;
        let cache_dir = tmp.path().join("coll-cache");

        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: format!("file://{}", repo_path.display()),
            cache_dir,
            git_ref: GitRef::Branch("main".into()),
            depth: None,
            ..GitSkillConfig::new("", "")
        });

        let collections = source.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].path, "extraction");
        assert_eq!(collections[0].description, "Entity extraction skills");
    }

    #[tokio::test]
    async fn test_tag_ref_no_refresh() {
        let tmp = TempDir::new().unwrap();
        let repo_path = create_test_repo(tmp.path()).await;

        // Create a tag in the repo
        let work_dir = tmp.path().join("tag-work");
        run_git(&[
            "clone".into(),
            repo_path.to_str().unwrap().into(),
            work_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();
        run_git_in_dir(&work_dir, &["tag".into(), "v1.0".into()])
            .await
            .unwrap();
        run_git_in_dir(&work_dir, &["push".into(), "--tags".into()])
            .await
            .unwrap();

        let cache_dir = tmp.path().join("tag-cache");
        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: format!("file://{}", repo_path.display()),
            cache_dir,
            git_ref: GitRef::Tag("v1.0".into()),
            refresh_interval: Duration::from_millis(1), // Very short — but immutable, so no refresh
            depth: None,
            ..GitSkillConfig::new("", "")
        });

        // First access — clones
        let skills1 = source.list(&SkillFilter::default()).await.unwrap();
        assert!(!skills1.is_empty());

        // Wait past "refresh interval"
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Second access — should NOT pull (tag is immutable)
        let skills2 = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills1.len(), skills2.len());
    }

    #[tokio::test]
    async fn test_skills_root_subdirectory() {
        let tmp = TempDir::new().unwrap();

        // Create a repo with skills in a subdirectory
        let repo_dir = tmp.path().join("subdir-repo");
        let work_dir = tmp.path().join("subdir-work");

        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        run_git(&["init".into(), "--bare".into(), repo_dir.to_str().unwrap().into()])
            .await
            .unwrap();
        run_git(&[
            "clone".into(),
            repo_dir.to_str().unwrap().into(),
            work_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();

        // Skills in a subdirectory
        let skill_dir = work_dir.join("my-skills/test-skill");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test\n---\n\nBody.",
        )
        .await
        .unwrap();

        run_git_in_dir(&work_dir, &["add".into(), "-A".into()])
            .await
            .unwrap();
        run_git_in_dir(
            &work_dir,
            &[
                "commit".into(),
                "-m".into(),
                "Add skills".into(),
                "--author".into(),
                "Test <test@test.com>".into(),
            ],
        )
        .await
        .unwrap();
        run_git_in_dir(&work_dir, &["push".into()])
            .await
            .unwrap();

        let cache_dir = tmp.path().join("subdir-cache");
        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: format!("file://{}", repo_dir.display()),
            cache_dir,
            git_ref: GitRef::Branch("main".into()),
            skills_root: Some("my-skills".into()),
            depth: None,
            ..GitSkillConfig::new("", "")
        });

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "test-skill");
    }
}
