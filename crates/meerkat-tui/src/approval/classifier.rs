//! The Tier-3 classifier seam.
//!
//! [`ActionClassifier`] is the only interface the approval engine uses; the
//! two-stage LLM implementation lives behind it (prompts and response parsing in
//! [`super::prompts`]). Errors never count as denials: the engine escalates to
//! the human instead.
//!
//! The two-stage `LlmClassifier` (fast one-word pass, then a reasoned pass
//! with a read-only project tool loop over [`crate::tools::ReadOnlyProjectTools`])
//! is ported in P2: it moves from the old `meerkat_client` streaming types to
//! `meerkat_llm_core::{LlmClient, LlmRequest, LlmEvent}`, whose request now
//! carries typed `ProviderTag` knobs (the old untyped low-reasoning params),
//! requires replay projection of the request messages, and emits new event
//! kinds (`AssistantOutput`, `ServerToolContent`, …). Its tests
//! (`tests/classifier_llm.rs`) move with it.

use super::transcript::ApprovalTranscript;
use async_trait::async_trait;
use meerkat_core::Provider;
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};

/// Facts about the environment shown to the classifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// Canonical project root (the trust boundary).
    pub project_root: PathBuf,
    /// Process working directory.
    pub cwd: PathBuf,
    /// `origin` remote URL, if any.
    pub git_remote: Option<String>,
    /// Default branch name, if known.
    pub default_branch: Option<String>,
    /// Operating system (`std::env::consts::OS`).
    pub os: String,
}

impl Environment {
    /// Environment without git facts.
    pub fn basic(project_root: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            cwd: cwd.into(),
            git_remote: None,
            default_branch: None,
            os: std::env::consts::OS.to_string(),
        }
    }

    /// Environment with the `origin` remote URL and default branch read from
    /// the repository's git metadata (best effort; anything missing or
    /// unreadable leaves the fields empty).
    ///
    /// The metadata is read from files rather than by running `git`, so
    /// building the classifier environment launches no process.
    pub async fn detect(project_root: &Path, cwd: &Path) -> Self {
        let mut env = Self::basic(project_root, cwd);
        let root = project_root.to_path_buf();
        let facts = tokio::task::spawn_blocking(move || GitFacts::read(&root))
            .await
            .unwrap_or_default();
        env.git_remote = facts.origin_url;
        env.default_branch = facts.default_branch;
        env
    }
}

/// Git facts read from a repository's metadata files.
#[derive(Debug, Default, PartialEq, Eq)]
struct GitFacts {
    origin_url: Option<String>,
    default_branch: Option<String>,
}

impl GitFacts {
    /// Read from the repository at `root` (its `.git` directory, or the
    /// `gitdir:` a `.git` file points to, and that directory's `commondir`).
    fn read(root: &Path) -> Self {
        let Some(common) = common_git_dir(root) else {
            return Self::default();
        };
        let origin_url = std::fs::read_to_string(common.join("config"))
            .ok()
            .and_then(|config| origin_url(&config));
        let default_branch = std::fs::read_to_string(common.join("refs/remotes/origin/HEAD"))
            .ok()
            .and_then(|head| {
                head.trim()
                    .strip_prefix("ref: refs/remotes/origin/")
                    .filter(|branch| !branch.is_empty())
                    .map(str::to_string)
            });
        Self {
            origin_url,
            default_branch,
        }
    }
}

/// The git directory holding `config` and `refs` for the work tree at `root`.
fn common_git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let target = PathBuf::from(pointer.trim().strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target
        } else {
            root.join(target)
        }
    };
    match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => {
            let common = PathBuf::from(common.trim());
            Some(if common.is_absolute() {
                common
            } else {
                git_dir.join(common)
            })
        }
        Err(_) => Some(git_dir),
    }
}

/// The `url` of `[remote "origin"]` in a git config file.
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

/// Everything the classifier is given. Deliberately excludes assistant prose,
/// reasoning and the agent's tool outputs; stage 2 may additionally read
/// project files itself through read-only project tools.
#[derive(Debug, Clone, Copy)]
pub struct ClassifierInput<'a> {
    /// Environment and trust boundary.
    pub environment: &'a Environment,
    /// Reasoning-blind transcript (user prompts and prior tool calls).
    pub transcript: &'a ApprovalTranscript,
    /// Tool under review.
    pub tool: &'a str,
    /// Its arguments.
    pub args: &'a Value,
    /// Content of an in-project script the action runs (≤ 8 KiB), if any.
    pub script_payload: Option<&'a str>,
    /// Project files the agent created or modified through file-edit tools
    /// during this run, as display paths, oldest first. Their content is
    /// agent-authored, so the classifier must not take it as evidence of
    /// user intent.
    pub agent_written: &'a [String],
}

/// The classifier's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifierVerdict {
    /// Allow the action.
    Allow,
    /// Block the action.
    Block {
        /// Block-rule id (e.g. `D1`), if the classifier named one.
        rule: Option<String>,
        /// Explanation passed back to the agent.
        reason: String,
    },
}

impl fmt::Display for ClassifierVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => f.write_str("ALLOW"),
            Self::Block {
                rule: Some(rule),
                reason,
            } => write!(f, "BLOCK [{rule}]: {reason}"),
            Self::Block { rule: None, reason } => write!(f, "BLOCK: {reason}"),
        }
    }
}

/// Classifier failures; never counted as denials.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClassifierError {
    /// A stage exceeded its time budget.
    #[error("classifier timed out")]
    Timeout,
    /// The provider call failed.
    #[error("classifier provider error: {0}")]
    Provider(String),
    /// The response could not be parsed.
    #[error("unparseable classifier response: {0}")]
    Unparseable(String),
}

/// Reviews Tier-3 actions.
#[async_trait]
pub trait ActionClassifier: Send + Sync {
    /// Decide whether the action may run.
    async fn classify(
        &self,
        input: ClassifierInput<'_>,
    ) -> Result<ClassifierVerdict, ClassifierError>;
}

/// Placeholder used until an LLM classifier is configured: every Tier-3 action
/// is escalated to the human (or denied when no human is available).
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableClassifier;

#[async_trait]
impl ActionClassifier for UnavailableClassifier {
    async fn classify(
        &self,
        _input: ClassifierInput<'_>,
    ) -> Result<ClassifierVerdict, ClassifierError> {
        Err(ClassifierError::Provider(
            "no classifier is configured".to_string(),
        ))
    }
}

/// Default classifier model for `provider` (`None` for self-hosted and
/// unknown providers).
#[must_use]
pub fn classifier_default_model(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Anthropic => Some("claude-sonnet-4-6"),
        Provider::OpenAI => Some("gpt-5.2"),
        Provider::Gemini => Some("gemini-3.1-flash-lite"),
        Provider::SelfHosted | Provider::Other => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unavailable_classifier_errors() {
        let env = Environment::basic("/p", "/p");
        let transcript = ApprovalTranscript::default();
        let args = serde_json::json!({});
        let input = ClassifierInput {
            environment: &env,
            transcript: &transcript,
            tool: "x",
            args: &args,
            script_payload: None,
            agent_written: &[],
        };
        assert!(matches!(
            UnavailableClassifier.classify(input).await,
            Err(ClassifierError::Provider(_))
        ));
    }

    #[test]
    fn git_facts_from_metadata_files() {
        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().join(".git");
        std::fs::create_dir_all(git.join("refs/remotes/origin")).unwrap();
        std::fs::write(
            git.join("config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://u/x\n[remote \"origin\"]\n\turl = git@host:me/x.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        )
        .unwrap();
        std::fs::write(
            git.join("refs/remotes/origin/HEAD"),
            "ref: refs/remotes/origin/main\n",
        )
        .unwrap();
        let facts = GitFacts::read(dir.path());
        assert_eq!(facts.origin_url.as_deref(), Some("git@host:me/x.git"));
        assert_eq!(facts.default_branch.as_deref(), Some("main"));

        // A linked work tree points at its git dir, which points at the common dir.
        let worktree = dir.path().join("wt");
        let wt_git = git.join("worktrees/wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", wt_git.display()),
        )
        .unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        assert_eq!(GitFacts::read(&worktree), facts);

        assert_eq!(
            GitFacts::read(&dir.path().join("nowhere")),
            GitFacts::default()
        );
    }

    #[test]
    fn verdict_display() {
        let v = ClassifierVerdict::Block {
            rule: Some("D1".into()),
            reason: "force push".into(),
        };
        assert_eq!(v.to_string(), "BLOCK [D1]: force push");
        assert_eq!(ClassifierVerdict::Allow.to_string(), "ALLOW");
    }
}
