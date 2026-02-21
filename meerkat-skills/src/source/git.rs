//! Git-backed skill source.
//!
//! Clones/pulls a git repo into a local cache directory using `gix` (gitoxide),
//! then delegates to `FilesystemSkillSource` for parsing and collection derivation.
//!
//! Pure Rust — no runtime dependency on the `git` CLI binary.

use std::future::Future;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use meerkat_core::skills::{
    SkillCollection, SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId,
    SkillQuarantineDiagnostic, SkillScope, SkillSource, SourceHealthSnapshot,
    SourceHealthThresholds, SourceUuid,
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
    fn is_immutable(&self) -> bool {
        matches!(self, GitRef::Tag(_) | GitRef::Commit(_))
    }
}

/// Authentication for git operations.
#[derive(Debug, Clone)]
pub enum GitSkillAuth {
    /// HTTPS token (injected as Authorization header via gix credential callback).
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
    /// Stable source UUID used for diagnostics/identity propagation.
    pub source_uuid: SourceUuid,
    /// Health thresholds to use for delegated filesystem source classification.
    pub health_thresholds: SourceHealthThresholds,
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
            source_uuid: match SourceUuid::parse("00000000-0000-4000-8000-000000000201") {
                Ok(source_uuid) => source_uuid,
                Err(_) => unreachable!("hardcoded git source UUID must remain valid"),
            },
            health_thresholds: SourceHealthThresholds::default(),
        }
    }
}

/// Git-backed skill source.
///
/// Clones a git repo on first access using gitoxide, then delegates to
/// `FilesystemSkillSource`. For Branch refs, fetches on TTL expiry.
/// For Tag/Commit refs, no refresh after initial clone.
pub struct GitSkillSource {
    config: GitSkillConfig,
    inner: RwLock<Option<FilesystemSkillSource>>,
    last_sync: RwLock<Option<Instant>>,
    /// Serializes clone/fetch operations to prevent concurrent callers from
    /// triggering duplicate git operations.
    sync_mutex: tokio::sync::Mutex<()>,
}

impl GitSkillSource {
    pub fn new(config: GitSkillConfig) -> Self {
        Self {
            config,
            inner: RwLock::new(None),
            last_sync: RwLock::new(None),
            sync_mutex: tokio::sync::Mutex::new(()),
        }
    }

    /// Ensure the repo is cloned and up to date. Lazy init.
    async fn ensure_synced(&self) -> Result<(), SkillError> {
        // Fast path: check freshness without lock
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

        // Serialize concurrent sync operations
        let _sync_guard = self.sync_mutex.lock().await;

        // Re-check after acquiring lock (another caller may have synced)
        {
            let last = self.last_sync.read().await;
            if let Some(synced_at) = *last
                && (self.config.git_ref.is_immutable()
                    || synced_at.elapsed() < self.config.refresh_interval)
            {
                return Ok(());
            }
        }

        // Need to sync
        let repo_dir = &self.config.cache_dir;

        if repo_dir.join(".git").exists() {
            // Already cloned — fetch if branch
            if !self.config.git_ref.is_immutable()
                && let Err(e) = self.gix_fetch().await
            {
                // Fetch failed — serve stale if we have data
                let has_data = self.inner.read().await.is_some();
                if has_data {
                    tracing::warn!(
                        "Git fetch failed for '{}', serving stale cache: {e}",
                        self.config.repo_url
                    );
                    let mut last = self.last_sync.write().await;
                    *last = Some(Instant::now()); // Reset TTL to avoid hammering
                    return Ok(());
                }
                return Err(e);
            }
        } else {
            // First access — clone
            if let Err(e) = self.gix_clone().await {
                // Clean up partial clone directory to prevent stale .git
                let _ = tokio::fs::remove_dir_all(repo_dir).await;
                return Err(e);
            }
        }

        // Rebuild the inner FilesystemSkillSource
        let skills_dir = match &self.config.skills_root {
            Some(root) => {
                let joined = repo_dir.join(root);
                // Validate skills_root doesn't escape the repo directory
                let canonical = joined.canonicalize().unwrap_or_else(|_| joined.clone());
                let repo_canonical = repo_dir.canonicalize().unwrap_or_else(|_| repo_dir.clone());
                if !canonical.starts_with(&repo_canonical) {
                    return Err(SkillError::Load(
                        format!("skills_root '{}' escapes repository directory", root).into(),
                    ));
                }
                joined
            }
            None => repo_dir.clone(),
        };

        let source = FilesystemSkillSource::new_with_identity(
            skills_dir,
            SkillScope::Project,
            self.config.source_uuid.clone(),
            self.config.health_thresholds,
        );
        *self.inner.write().await = Some(source);
        *self.last_sync.write().await = Some(Instant::now());

        Ok(())
    }

    /// Clone the repository using gix (gitoxide).
    async fn gix_clone(&self) -> Result<(), SkillError> {
        let url = self.config.repo_url.clone();
        let cache_dir = self.config.cache_dir.clone();
        let git_ref = self.config.git_ref.clone();
        let depth = self.config.depth;
        let auth = self.config.auth.clone();

        tokio::task::spawn_blocking(move || {
            gix_clone_blocking(&url, &cache_dir, &git_ref, depth, auth.as_ref())
        })
        .await
        .map_err(|e| SkillError::Load(format!("clone task panicked: {e}").into()))?
    }

    /// Fetch latest from remote for branch refs using gix.
    async fn gix_fetch(&self) -> Result<(), SkillError> {
        let cache_dir = self.config.cache_dir.clone();
        let repo_url = self.config.repo_url.clone();

        tokio::task::spawn_blocking(move || gix_fetch_blocking(&cache_dir, &repo_url))
            .await
            .map_err(|e| SkillError::Load(format!("fetch task panicked: {e}").into()))?
    }
}

// ---------------------------------------------------------------------------
// Blocking gix operations (run inside spawn_blocking)
// ---------------------------------------------------------------------------

/// Clone a repository using gix. Blocking — must be called via spawn_blocking.
fn gix_clone_blocking(
    url: &str,
    dest: &Path,
    git_ref: &GitRef,
    depth: Option<usize>,
    auth: Option<&GitSkillAuth>,
) -> Result<(), SkillError> {
    let interrupt = AtomicBool::new(false);

    // Ensure the parent directory exists (git CLI creates it implicitly, gix does not)
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SkillError::Load(format!("cannot create cache dir: {e}").into()))?;
    }

    let mut prepare = gix::prepare_clone(url, dest).map_err(|e| {
        SkillError::Load(format!("gix clone prepare failed for '{url}': {e}").into())
    })?;

    // Set branch/tag ref (not applicable for commit — clone default then checkout)
    match git_ref {
        GitRef::Branch(name) | GitRef::Tag(name) => {
            prepare = prepare
                .with_ref_name(Some(name.as_str()))
                .map_err(|e| SkillError::Load(format!("invalid ref name '{name}': {e}").into()))?;
        }
        GitRef::Commit(_) => {
            // Clone default branch, checkout commit after
        }
    }

    // Shallow clone
    if !matches!(git_ref, GitRef::Commit(_))
        && let Some(d) = depth.and_then(NonZeroU32::try_from_u32)
    {
        prepare = prepare.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(d));
    }

    // Authentication: inject token via gix credential callback
    if let Some(GitSkillAuth::HttpsToken(token)) = auth {
        let token = token.clone();
        prepare = prepare.configure_connection(move |conn| {
            let t = token.clone();
            conn.set_credentials(move |action| match action {
                gix::credentials::helper::Action::Get(ctx) => {
                    Ok(Some(gix::credentials::protocol::Outcome {
                        identity: gix::sec::identity::Account {
                            username: "x-access-token".into(),
                            password: t.clone(),
                            oauth_refresh_token: None,
                        },
                        next: gix::credentials::helper::NextAction::from(ctx),
                    }))
                }
                _ => Ok(None),
            });
            Ok(())
        });
    }

    // Execute clone
    let (mut checkout, _outcome) = prepare
        .fetch_then_checkout(gix::progress::Discard, &interrupt)
        .map_err(|e| SkillError::Load(format!("gix fetch failed for '{url}': {e}").into()))?;

    // Checkout working tree
    let (_repo, _) = checkout
        .main_worktree(gix::progress::Discard, &interrupt)
        .map_err(|e| SkillError::Load(format!("gix checkout failed for '{url}': {e}").into()))?;

    // For commit refs, detach HEAD to the specific SHA and update worktree
    if let GitRef::Commit(sha) = git_ref {
        checkout_tree_at_rev(dest, sha, &interrupt)?;
    }

    Ok(())
}

/// Fetch latest from origin using gix. Blocking — must be called via spawn_blocking.
fn gix_fetch_blocking(repo_dir: &Path, repo_url: &str) -> Result<(), SkillError> {
    let interrupt = AtomicBool::new(false);

    let repo = gix::open(repo_dir)
        .map_err(|e| SkillError::Load(format!("gix open failed for '{repo_url}': {e}").into()))?;

    let remote = repo
        .find_default_remote(gix::remote::Direction::Fetch)
        .ok_or_else(|| SkillError::Load(format!("no fetch remote for '{repo_url}'").into()))?
        .map_err(|e| SkillError::Load(format!("bad remote for '{repo_url}': {e}").into()))?;

    let _outcome = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(|e| SkillError::Load(format!("connect failed for '{repo_url}': {e}").into()))?
        .prepare_fetch(gix::progress::Discard, Default::default())
        .map_err(|e| {
            SkillError::Load(format!("prepare fetch failed for '{repo_url}': {e}").into())
        })?
        .receive(gix::progress::Discard, &interrupt)
        .map_err(|e| SkillError::Load(format!("fetch failed for '{repo_url}': {e}").into()))?;

    // Fast-forward the local branch to match the remote tracking branch
    let head = repo
        .head()
        .map_err(|e| SkillError::Load(format!("cannot read HEAD: {e}").into()))?;

    if let Some(referent) = head.referent_name() {
        let referent_str = referent.as_bstr().to_string();
        let branch_name = referent_str
            .strip_prefix("refs/heads/")
            .unwrap_or(&referent_str)
            .to_string();
        let tracking_name = format!("refs/remotes/origin/{branch_name}");

        if let Ok(tracking_ref) = repo.find_reference(&tracking_name) {
            let target = tracking_ref.id();
            repo.reference(
                referent.as_bstr().to_string(),
                target,
                gix::refs::transaction::PreviousValue::Any,
                "skills fetch: fast-forward",
            )
            .map_err(|e| SkillError::Load(format!("fast-forward failed: {e}").into()))?;

            // Update the working tree to match the new HEAD
            let target_str = target.to_string();
            checkout_tree_at_rev(repo_dir, &target_str, &interrupt)?;
        }
    }

    Ok(())
}

/// Checkout the working tree to match a specific revision.
///
/// Uses `repo.objects.into_arc()` to convert the `Rc`-backed object store
/// into an `Arc`-backed one, satisfying the `Send` bound required by
/// `gix::worktree::state::checkout`.
fn checkout_tree_at_rev(
    repo_dir: &Path,
    rev: &str,
    interrupt: &AtomicBool,
) -> Result<(), SkillError> {
    let repo = gix::open(repo_dir)
        .map_err(|e| SkillError::Load(format!("gix open failed: {e}").into()))?;

    // Resolve rev → tree ID and build index state while borrowing repo,
    // then convert objects to Arc-backed for the checkout (which requires Send).
    let (tree_id, workdir) = {
        let oid = repo
            .rev_parse_single(rev)
            .map_err(|e| SkillError::Load(format!("rev '{rev}' not found: {e}").into()))?;
        let commit = oid
            .object()
            .map_err(|e| SkillError::Load(format!("cannot read '{rev}': {e}").into()))?
            .try_into_commit()
            .map_err(|e| SkillError::Load(format!("'{rev}' is not a commit: {e}").into()))?;
        let tree = commit
            .tree()
            .map_err(|e| SkillError::Load(format!("cannot read tree for '{rev}': {e}").into()))?;
        (tree.id, repo.workdir().unwrap_or(repo_dir).to_path_buf())
    };

    // Build index from tree (borrows repo.objects temporarily)
    let mut state = gix::index::State::from_tree(&tree_id, &repo.objects, Default::default())
        .map_err(|e| SkillError::Load(format!("index from tree failed: {e}").into()))?;

    // Now move repo.objects into Arc-backed form for Send (required by checkout)
    let objects = repo
        .objects
        .into_arc()
        .map_err(|e| SkillError::Load(format!("object store conversion failed: {e}").into()))?;

    gix::worktree::state::checkout(
        &mut state,
        workdir,
        objects,
        &gix::progress::Discard,
        &gix::progress::Discard,
        interrupt,
        gix::worktree::state::checkout::Options::default(),
    )
    .map_err(|e| SkillError::Load(format!("worktree checkout failed: {e}").into()))?;

    Ok(())
}

/// Helper to convert `Option<usize>` to `NonZeroU32` for shallow clone depth.
trait TryFromU32 {
    fn try_from_u32(v: usize) -> Option<NonZeroU32>;
}

impl TryFromU32 for NonZeroU32 {
    fn try_from_u32(v: usize) -> Option<NonZeroU32> {
        u32::try_from(v).ok().and_then(NonZeroU32::new)
    }
}

#[allow(clippy::manual_async_fn)]
impl SkillSource for GitSkillSource {
    fn list(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
        async move {
            self.ensure_synced().await?;
            let guard = self.inner.read().await;
            let source = guard
                .as_ref()
                .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
            source.list(filter).await
        }
    }

    fn load(&self, id: &SkillId) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        async move {
            self.ensure_synced().await?;
            let guard = self.inner.read().await;
            let source = guard
                .as_ref()
                .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
            source.load(id).await
        }
    }

    fn collections(&self) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
        async move {
            self.ensure_synced().await?;
            let guard = self.inner.read().await;
            let source = guard
                .as_ref()
                .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
            source.collections().await
        }
    }

    fn quarantined_diagnostics(
        &self,
    ) -> impl Future<Output = Result<Vec<SkillQuarantineDiagnostic>, SkillError>> + Send {
        async move {
            self.ensure_synced().await?;
            let guard = self.inner.read().await;
            let source = guard
                .as_ref()
                .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
            source.quarantined_diagnostics().await
        }
    }

    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<SourceHealthSnapshot, SkillError>> + Send {
        async move {
            self.ensure_synced().await?;
            let guard = self.inner.read().await;
            let source = guard
                .as_ref()
                .ok_or_else(|| SkillError::Load("Git source not initialized".into()))?;
            source.health_snapshot().await
        }
    }
}

// ===========================================================================
// Test helpers — use git CLI for fixture setup only (creating bare repos, pushing)
// ===========================================================================

/// Run a git CLI command (test fixtures only).
#[cfg(test)]
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

/// Run a git CLI command in a directory (test fixtures only).
#[cfg(test)]
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

/// Test support utilities for creating local bare repos.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// Create a local bare git repo with a sample skill and push initial commit.
    pub async fn init_test_repo(
        repo_dir: &std::path::Path,
        work_dir: &std::path::Path,
    ) -> Result<(), String> {
        let repo_dir_str = repo_dir.to_str().ok_or_else(|| {
            "failed to convert repo_dir to UTF-8 path for git command".to_string()
        })?;
        let work_dir_str = work_dir.to_str().ok_or_else(|| {
            "failed to convert work_dir to UTF-8 path for git command".to_string()
        })?;

        run_git(&["init".into(), "--bare".into(), repo_dir_str.into()])
            .await
            .map_err(|err| format!("git init failed: {err}"))?;

        run_git(&["clone".into(), repo_dir_str.into(), work_dir_str.into()])
            .await
            .map_err(|err| format!("git clone failed: {err}"))?;

        let skill_dir = work_dir.join("extraction/email");
        tokio::fs::create_dir_all(&skill_dir)
            .await
            .map_err(|err| format!("failed to create skill dir: {err}"))?;
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: email\ndescription: Extract from emails\n---\n\n# Email\n\nBody.",
        )
        .await
        .map_err(|err| format!("failed to write SKILL.md: {err}"))?;

        let coll_dir = work_dir.join("extraction");
        tokio::fs::write(coll_dir.join("COLLECTION.md"), "Entity extraction skills")
            .await
            .map_err(|err| format!("failed to write COLLECTION.md: {err}"))?;

        run_git_in_dir(work_dir, &["add".into(), "-A".into()])
            .await
            .map_err(|err| format!("git add failed: {err}"))?;
        run_git_in_dir(
            work_dir,
            &[
                "commit".into(),
                "-m".into(),
                "Initial skills".into(),
                "--author".into(),
                "Test <test@test.com>".into(),
            ],
        )
        .await
        .map_err(|err| format!("git commit failed: {err}"))?;
        run_git_in_dir(work_dir, &["push".into()])
            .await
            .map_err(|err| format!("git push failed: {err}"))?;

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a local bare git repo with skill files for testing.
    async fn create_test_repo(tmp: &Path) -> PathBuf {
        let repo_dir = tmp.join("test-repo");
        let work_dir = tmp.join("work");

        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        run_git(&[
            "init".into(),
            "--bare".into(),
            repo_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();

        run_git(&[
            "clone".into(),
            repo_dir.to_str().unwrap().into(),
            work_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();

        let skill_dir = work_dir.join("extraction/email");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: email\ndescription: Extract from emails\n---\n\n# Email\n\nBody.",
        )
        .await
        .unwrap();

        let coll_dir = work_dir.join("extraction");
        tokio::fs::write(coll_dir.join("COLLECTION.md"), "Entity extraction skills")
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
                "Initial skills".into(),
                "--author".into(),
                "Test <test@test.com>".into(),
            ],
        )
        .await
        .unwrap();
        run_git_in_dir(&work_dir, &["push".into()]).await.unwrap();

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
    async fn test_health_thresholds_and_diagnostics_wired_for_git_source() {
        let tmp = TempDir::new().unwrap();
        let repo_dir = tmp.path().join("fake-repo");
        tokio::fs::create_dir_all(repo_dir.join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(repo_dir.join("valid-skill"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(repo_dir.join("broken-skill"))
            .await
            .unwrap();
        tokio::fs::write(
            repo_dir.join("valid-skill/SKILL.md"),
            "---\nname: valid-skill\ndescription: valid\n---\n\nbody",
        )
        .await
        .unwrap();
        tokio::fs::write(
            repo_dir.join("broken-skill/SKILL.md"),
            "---\nname: wrong-name\ndescription: invalid\n---\n\nbody",
        )
        .await
        .unwrap();

        let source_uuid = SourceUuid::parse("00000000-0000-4000-8000-000000000208")
            .expect("valid git source uuid");
        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: "file:///unused".to_string(),
            cache_dir: repo_dir,
            git_ref: GitRef::Tag("v1.0".into()),
            health_thresholds: SourceHealthThresholds {
                degraded_invalid_ratio: 0.75,
                unhealthy_invalid_ratio: 0.95,
                degraded_failure_streak: 10,
                unhealthy_failure_streak: 20,
            },
            source_uuid: source_uuid.clone(),
            ..GitSkillConfig::new("", "")
        });

        let listed = source.list(&SkillFilter::default()).await.unwrap();
        assert!(listed.iter().any(|s| s.id.0 == "valid-skill"));

        let snapshot = source.health_snapshot().await.unwrap();
        assert_eq!(snapshot.invalid_count, 1);
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(
            snapshot.state,
            meerkat_core::skills::SourceHealthState::Healthy
        );

        let diagnostics = source.quarantined_diagnostics().await.unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].source_uuid, source_uuid);
    }

    #[tokio::test]
    #[ignore] // integration_real: spawns git CLI for fixture setup
    async fn test_integration_real_clone_on_first_access() {
        let tmp = TempDir::new().unwrap();
        let repo_path = create_test_repo(tmp.path()).await;
        let cache_dir = tmp.path().join("clone-cache");

        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: format!("file://{}", repo_path.display()),
            cache_dir: cache_dir.clone(),
            git_ref: GitRef::Branch("main".into()),
            depth: None,
            ..GitSkillConfig::new("", "")
        });

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(!skills.is_empty());
        assert!(cache_dir.exists());
    }

    #[tokio::test]
    #[ignore] // integration_real: spawns git CLI for fixture setup
    async fn test_integration_real_namespaced_ids() {
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
    #[ignore] // integration_real: spawns git CLI for fixture setup
    async fn test_integration_real_collection_md() {
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
    #[ignore] // integration_real: spawns git CLI for fixture setup
    async fn test_integration_real_tag_ref_no_refresh() {
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
            refresh_interval: Duration::from_millis(1),
            depth: None,
            ..GitSkillConfig::new("", "")
        });

        let skills1 = source.list(&SkillFilter::default()).await.unwrap();
        assert!(!skills1.is_empty());

        tokio::time::sleep(Duration::from_millis(10)).await;

        let skills2 = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills1.len(), skills2.len());
    }

    #[tokio::test]
    #[ignore] // integration_real: spawns git CLI for fixture setup
    async fn test_integration_real_skills_root_subdirectory() {
        let tmp = TempDir::new().unwrap();

        let repo_dir = tmp.path().join("subdir-repo");
        let work_dir = tmp.path().join("subdir-work");

        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        run_git(&[
            "init".into(),
            "--bare".into(),
            repo_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();
        run_git(&[
            "clone".into(),
            repo_dir.to_str().unwrap().into(),
            work_dir.to_str().unwrap().into(),
        ])
        .await
        .unwrap();

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
        run_git_in_dir(&work_dir, &["push".into()]).await.unwrap();

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
