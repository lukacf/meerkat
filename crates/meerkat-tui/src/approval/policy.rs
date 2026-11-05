//! Tier classification: decides, without any LLM call, whether a tool call is
//! denied by a user rule (Tier 0), always allowed (Tier 1), an in-project edit
//! (Tier 2), or must be reviewed by the classifier (Tier 3).
//!
//! # The tiers and the OS confinement
//!
//! The tiers do not change with `--confinement`: the sandbox is defense in
//! depth under the gate, not a reason to review less. What the shell's
//! confinement ([`crate::bootstrap::confinement`]) already contains, for a
//! Tier-3 shell command the classifier allows (or wrongly allows):
//!
//! - **Writes outside the workspace** (`rm -rf ~/…`, editing `~/.bashrc`,
//!   `~/.ssh`, `/etc`): refused by the backend in `workspace` and `strict`
//!   mode, except in the temp dir, the Cargo home and the user cache dir,
//!   which the workspace mode grants.
//! - **Reading secrets outside the workspace** (`~/.ssh`, `~/.aws`, browser
//!   profiles, the credential store): not visible to the process at all;
//!   only the workspace, the granted extras and the platform baseline are.
//! - **Exfiltration and downloads** (`curl … | sh`, posting files to a
//!   remote host): contained only in `strict` mode, which denies IP network
//!   access; the default `workspace` mode keeps the network so `cargo fetch`
//!   works, so the classifier remains the only guard there.
//! - **Lingering processes** (`nohup`, daemons): every shell launch runs in
//!   durable process custody, so an interrupted or crashed run's processes
//!   are stopped, whatever the mode.
//!
//! Not contained by any mode: anything inside the workspace (deleting the
//! project, rewriting git history, editing `.rkat/` state; protected paths
//! keep Tier 2 off them, and on Linux the backend has no deny lists), and
//! everything the model does through the network in `workspace` mode.

use super::patch_targets::{ParseError, affected_paths};
use super::rules::{RuleId, RuleSet};
use super::shell;
use crate::memory::{MEMORY_FORGET, MEMORY_LIST, MEMORY_READ, MEMORY_SAVE, Scope, slug};
use crate::paths::ProjectRoot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Policy tier, first match wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Tier 0: a user deny rule matched.
    UserDeny,
    /// Tier 1: read-only builtin, in-project read, allowlisted shell, or narrow allow rule.
    AlwaysAllow,
    /// Tier 2: write/edit/patch where every target is inside the project and unprotected.
    InProjectEdit,
    /// Tier 3: needs classifier review.
    Classifier,
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UserDeny => "user deny rule",
            Self::AlwaysAllow => "always allow",
            Self::InProjectEdit => "in-project edit",
            Self::Classifier => "classifier",
        })
    }
}

/// Who made the final call on an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decider {
    /// Static policy (tiers 0–2, user rules, auto mode disabled).
    Policy,
    /// The LLM classifier (including cached allows).
    Classifier,
    /// The human, via escalation.
    Human,
}

/// Compact verdict record for the UI and the approval transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum VerdictSummary {
    /// The action was allowed.
    Allowed {
        /// Tier the action fell into.
        tier: Tier,
        /// Who allowed it.
        by: Decider,
    },
    /// The action was denied.
    Denied {
        /// Tier the action fell into.
        tier: Tier,
        /// Who denied it.
        by: Decider,
    },
}

impl VerdictSummary {
    /// Whether the action was allowed.
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed { .. })
    }
}

/// Builtins that only read state or manage the agent's own bookkeeping.
///
/// Every upstream builtin the TUI's sessions can be offered is either here
/// or in [`REVIEWED_BUILTINS`]; `upstream_builtins_are_all_classified`
/// pins the upstream families, and the `builtin_tiers` integration test
/// every tool a real session offers the model.
pub const READ_ONLY_BUILTINS: &[&str] = &[
    "task_create",
    "task_get",
    "task_list",
    "task_update",
    "datetime",
    "view_image",
    "wait",
    "blob_inspect",
    "browse_skills",
    "load_skill",
    "skill_list_resources",
    "skill_read_resource",
    "shell_jobs",
    "shell_job_status",
    "peers",
    // Native semantic memory (`AgentFactory::memory`): searches the
    // session's own compacted history.
    "memory_search",
    // Mob queries (see [`crate::mobs`]).
    "mob_list",
    "mob_list_members",
    "mob_check_member",
    "mob_profile_get",
    "mob_profile_list",
    "mob_profile_list_sources",
];

/// Upstream builtins that act (or read outside the project) and so pass the
/// classifier unless a user rule decides them: the shell and its job
/// controls, file blobs, skill functions, web search, model switch, image
/// generation, comms sends, councils and objective conclusions. The
/// mutating `mob_*` tools need review too (any `mob_*` not in
/// [`READ_ONLY_BUILTINS`]).
pub const REVIEWED_BUILTINS: &[&str] = &[
    "shell",
    "shell_job_cancel",
    "monitor_start",
    "apply_patch",
    "blob_save_file",
    "blob_load_file",
    "skill_invoke_function",
    "web_search",
    "brain_swap",
    "generate_image",
    "send_message",
    "reply_to_peer",
    "send_request",
    "send_response",
    // Mob tools beyond `delegate`: a temporary council of helpers, and a
    // member concluding its kickoff objective.
    "council",
    "conclude_objective",
];

/// Tools that hand work to a helper (a mob member, see [`crate::mobs`]).
/// Delegating does nothing by itself: every call the helper makes passes
/// the delegating session's gate.
pub const DELEGATION_TOOLS: &[&str] = &["delegate", "fork_off"];

/// What a tool call does, as far as the policy is concerned.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionKind<'a> {
    /// `shell`, parsed with [`ShellArgs`] (a copy of the tool's own argument type).
    Shell {
        /// Command string.
        command: String,
        /// Optional working directory (relative to the project root).
        working_dir: Option<String>,
        /// Background execution requested.
        background: bool,
    },
    /// `read_file`.
    FileRead {
        /// Requested path.
        path: &'a str,
    },
    /// `glob` / `grep`.
    Search {
        /// Search root (defaults to the project root).
        path: Option<&'a str>,
        /// `glob`'s pattern, which can itself name a directory.
        pattern: Option<&'a str>,
    },
    /// `write_file` / `edit_file` / `apply_patch`.
    FileWrite {
        /// Every path the call may create, modify, delete or move to.
        paths: Vec<String>,
    },
    /// A builtin from [`READ_ONLY_BUILTINS`].
    ReadOnlyBuiltin,
    /// A tool from [`DELEGATION_TOOLS`].
    Delegation,
    /// `memory_list` / `memory_read`.
    MemoryRead,
    /// `memory_save` / `memory_forget` with a valid scope.
    MemoryWrite {
        /// Note scope.
        scope: Scope,
        /// Note name as supplied (rule subjects use its slug when valid).
        name: &'a str,
    },
    /// Anything else, including calls with malformed arguments.
    Other,
}

/// A parsed tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct Action<'a> {
    /// Tool name.
    pub name: &'a str,
    /// Raw arguments.
    pub args: &'a Value,
    /// Policy-relevant interpretation.
    pub kind: ActionKind<'a>,
}

impl<'a> Action<'a> {
    /// Interpret a tool call. Malformed arguments yield [`ActionKind::Other`],
    /// which always reaches the classifier.
    pub fn parse(name: &'a str, args: &'a Value) -> Self {
        let str_arg = |key: &str| args.get(key).and_then(Value::as_str);
        let kind = match name {
            "shell" => parse_shell(args).map_or(ActionKind::Other, |input| ActionKind::Shell {
                command: input.command,
                working_dir: input.working_dir,
                background: input.background,
            }),
            "read_file" => {
                str_arg("path").map_or(ActionKind::Other, |path| ActionKind::FileRead { path })
            }
            "glob" => ActionKind::Search {
                path: str_arg("path"),
                pattern: str_arg("pattern"),
            },
            "grep" => ActionKind::Search {
                path: str_arg("path"),
                pattern: None,
            },
            "write_file" | "edit_file" => {
                str_arg("path").map_or(ActionKind::Other, |path| ActionKind::FileWrite {
                    paths: vec![path.to_string()],
                })
            }
            "apply_patch" => match str_arg("patch").map(patch_targets) {
                Some(Ok(paths)) if !paths.is_empty() => ActionKind::FileWrite { paths },
                _ => ActionKind::Other,
            },
            MEMORY_LIST | MEMORY_READ => ActionKind::MemoryRead,
            MEMORY_SAVE | MEMORY_FORGET => {
                let scope = args.get("scope").map(Scope::deserialize);
                match (scope, str_arg("name")) {
                    (Some(Ok(scope)), Some(name)) => ActionKind::MemoryWrite { scope, name },
                    _ => ActionKind::Other,
                }
            }
            n if READ_ONLY_BUILTINS.contains(&n) => ActionKind::ReadOnlyBuiltin,
            n if DELEGATION_TOOLS.contains(&n) => ActionKind::Delegation,
            _ => ActionKind::Other,
        };
        Self { name, args, kind }
    }

    /// Canonical rule subjects: the command for `shell`, display paths for file
    /// tools, `<scope>/<slug>` for memory writes, pretty JSON for everything else.
    pub fn subjects(&self, root: &ProjectRoot) -> Vec<String> {
        let display = |raw: &str| match root.resolve(raw) {
            Ok(resolved) => root.display_of(&resolved).to_string(),
            Err(_) => raw.to_string(),
        };
        match &self.kind {
            ActionKind::Shell { command, .. } => vec![command.clone()],
            ActionKind::FileRead { path } => vec![display(path)],
            ActionKind::Search { path, .. } => vec![path.map_or_else(|| ".".to_string(), display)],
            ActionKind::FileWrite { paths } => paths.iter().map(|p| display(p)).collect(),
            ActionKind::MemoryWrite { scope, name } => {
                let name = slug(name).unwrap_or_else(|_| (*name).to_string());
                vec![format!("{scope}/{name}")]
            }
            ActionKind::ReadOnlyBuiltin
            | ActionKind::Delegation
            | ActionKind::MemoryRead
            | ActionKind::Other => {
                vec![serde_json::to_string_pretty(self.args).unwrap_or_default()]
            }
        }
    }
}

/// Target paths named by an `apply_patch` body, from the tool's own parser.
///
/// # Errors
///
/// Returns the parser's error when the patch is malformed (the action then
/// needs classifier review).
pub fn patch_targets(patch: &str) -> Result<Vec<String>, ParseError> {
    Ok(affected_paths(patch)?
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// Arguments of upstream's `shell` builtin.
///
/// **Local copy, to be upstreamed.** Mirrors the private `ShellInput` of
/// `crates/meerkat-tools/src/builtin/shell/tool.rs`; keep the field set in
/// sync with it. Unknown argument names are rejected
/// (`deny_unknown_fields`): the tool would ignore them, which makes their
/// purpose unclear, so such calls go to the classifier.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellArgs {
    /// The command to execute.
    pub command: String,
    /// Working directory (relative to the project root).
    pub working_dir: Option<String>,
    /// Timeout in seconds.
    pub timeout_secs: Option<u64>,
    /// Run as a background job.
    #[serde(default)]
    pub background: bool,
}

/// Parse shell arguments; anything the tool itself would refuse, or any
/// unknown argument name, yields `None`.
fn parse_shell(args: &Value) -> Option<ShellArgs> {
    ShellArgs::deserialize(args).ok()
}

/// Environment for tier classification.
#[derive(Debug, Clone, Copy)]
pub struct PolicyContext<'a> {
    /// Canonical project root.
    pub root: &'a ProjectRoot,
    /// Whether `cargo build|test|nextest|check|clippy` are Tier 1.
    pub trust_project_builds: bool,
    /// Whether the shell tool runs a POSIX shell (Tier-1 shell analysis is off otherwise).
    pub posix_shell: bool,
}

/// Result of tier classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierDecision {
    /// Tier.
    pub tier: Tier,
    /// Human-readable explanation.
    pub detail: String,
    /// The user rule that decided, if any.
    pub rule: Option<RuleId>,
}

impl TierDecision {
    fn new(tier: Tier, detail: impl Into<String>) -> Self {
        Self {
            tier,
            detail: detail.into(),
            rule: None,
        }
    }
}

/// Classify an action into a tier.
pub fn classify_tier(
    action: &Action<'_>,
    rules: &RuleSet,
    ctx: &PolicyContext<'_>,
) -> TierDecision {
    let subjects = action.subjects(ctx.root);
    if let Some(id) = rules.matches_deny(action.name, &subjects) {
        return TierDecision {
            tier: Tier::UserDeny,
            detail: format!("matches user rule {id}"),
            rule: Some(id),
        };
    }

    let fallback = match builtin_tier(action, ctx) {
        Ok(decision) => return decision,
        Err(reason) => reason,
    };

    if let Some(id) = rules.matches_allow(action.name, &subjects) {
        return TierDecision {
            tier: Tier::AlwaysAllow,
            detail: format!("matches user rule {id}"),
            rule: Some(id),
        };
    }
    TierDecision::new(Tier::Classifier, fallback)
}

/// Tiers 1–2 without user rules. `Err` carries why the action needs review.
fn builtin_tier(action: &Action<'_>, ctx: &PolicyContext<'_>) -> Result<TierDecision, String> {
    match &action.kind {
        ActionKind::ReadOnlyBuiltin => {
            Ok(TierDecision::new(Tier::AlwaysAllow, "read-only builtin"))
        }
        ActionKind::Delegation => Ok(TierDecision::new(
            Tier::AlwaysAllow,
            "delegation (the helper's own calls pass this gate)",
        )),
        ActionKind::MemoryRead => Ok(TierDecision::new(Tier::AlwaysAllow, "memory read")),
        ActionKind::MemoryWrite { scope, .. } => {
            if *scope == Scope::Workspace {
                Ok(TierDecision::new(
                    Tier::InProjectEdit,
                    "workspace memory note",
                ))
            } else {
                Err("user-level memory persists into every project's system prompt".to_string())
            }
        }
        ActionKind::FileRead { path } => {
            require_inside(ctx.root, path)?;
            Ok(TierDecision::new(
                Tier::AlwaysAllow,
                "read inside the project",
            ))
        }
        ActionKind::Search { path, pattern } => {
            if let Some(path) = path {
                require_inside(ctx.root, path)?;
            }
            if let Some(pattern) = pattern
                && escapes_root(pattern)
            {
                return Err(format!(
                    "glob pattern `{pattern}` reaches outside the project"
                ));
            }
            Ok(TierDecision::new(
                Tier::AlwaysAllow,
                "search inside the project",
            ))
        }
        ActionKind::FileWrite { paths } => {
            for path in paths {
                let resolved = ctx
                    .root
                    .resolve(path)
                    .map_err(|e| format!("path `{path}`: {e}"))?;
                if !resolved.inside() {
                    return Err(format!("`{path}` is outside the project"));
                }
                if resolved.protected {
                    return Err(format!("`{path}` is a protected path"));
                }
            }
            Ok(TierDecision::new(
                Tier::InProjectEdit,
                "edit inside the project",
            ))
        }
        ActionKind::Shell {
            command,
            working_dir,
            background,
        } => {
            if *background {
                return Err("background shell command".to_string());
            }
            shell::allowlisted_in(command, working_dir.as_deref(), ctx).map_err(|e| e.0)?;
            Ok(TierDecision::new(
                Tier::AlwaysAllow,
                "read-only shell command",
            ))
        }
        ActionKind::Other => Err(format!("`{}` requires review", action.name)),
    }
}

fn require_inside(root: &ProjectRoot, path: &str) -> Result<(), String> {
    let resolved = root
        .resolve(path)
        .map_err(|e| format!("path `{path}`: {e}"))?;
    if resolved.inside() {
        Ok(())
    } else {
        Err(format!("`{path}` is outside the project"))
    }
}

fn escapes_root(pattern: &str) -> bool {
    pattern.starts_with('/')
        || pattern.starts_with('~')
        || pattern.split(['/', '\\']).any(|c| c == "..")
        || std::path::Path::new(pattern).is_absolute()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::approval::rules::{Rule, RuleKind, RuleOrigin};
    use serde_json::json;

    fn project() -> (tempfile::TempDir, ProjectRoot) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let root = ProjectRoot::new(dir.path()).unwrap();
        (dir, root)
    }

    fn tier(root: &ProjectRoot, rules: &RuleSet, trust: bool, name: &str, args: &Value) -> Tier {
        let ctx = PolicyContext {
            root,
            trust_project_builds: trust,
            posix_shell: true,
        };
        classify_tier(&Action::parse(name, args), rules, &ctx).tier
    }

    #[test]
    fn parse_patch_targets() {
        let patch = "*** Begin Patch\n*** Add File: a.txt\n+x\n*** Update File: src/b.rs\n*** Move to: src/c.rs\n@@\n-x\n+y\n*** Delete File: old.rs\n*** End Patch\n";
        assert_eq!(
            patch_targets(patch).unwrap(),
            ["a.txt", "src/b.rs", "src/c.rs", "old.rs"]
        );
        assert!(
            patch_targets("*** Begin Patch\n*** End Patch")
                .unwrap()
                .is_empty()
        );
        assert!(patch_targets("garbage").is_err());
    }

    #[test]
    fn indented_patch_header_is_a_target() {
        let (_dir, root) = project();
        let args = json!({
            "patch": "*** Begin Patch\n*** Add File: src/ok.rs\n+x\n  *** Add File: .git/config\n+[core]\n*** End Patch"
        });
        let action = Action::parse("apply_patch", &args);
        assert_eq!(
            action.kind,
            ActionKind::FileWrite {
                paths: vec!["src/ok.rs".into(), ".git/config".into()]
            }
        );
        assert_eq!(
            tier(&root, &RuleSet::default(), false, "apply_patch", &args),
            Tier::Classifier
        );
    }

    #[test]
    fn delegation_and_mob_queries_need_no_review_but_mob_changes_do() {
        let (_dir, root) = project();
        let task = json!({"member_id": "scout", "task": "rm -rf /", "result_label": "r", "max_text_bytes": 10});
        assert_eq!(
            Action::parse("delegate", &task).kind,
            ActionKind::Delegation
        );
        assert_eq!(
            tier(&root, &RuleSet::default(), false, "delegate", &task),
            Tier::AlwaysAllow
        );
        assert_eq!(
            tier(
                &root,
                &RuleSet::default(),
                false,
                "mob_list_members",
                &json!({})
            ),
            Tier::AlwaysAllow
        );
        assert_eq!(
            tier(
                &root,
                &RuleSet::default(),
                false,
                "mob_destroy",
                &json!({"mob_id": "x"})
            ),
            Tier::Classifier
        );
    }

    #[test]
    fn shell_args_use_the_tool_type() {
        let args = json!({"command": "ls", "working_dir": "src", "timeout_secs": 5});
        assert_eq!(
            Action::parse("shell", &args).kind,
            ActionKind::Shell {
                command: "ls".into(),
                working_dir: Some("src".into()),
                background: false
            }
        );
        for bad in [
            json!({"command": "ls", "cwd": "/"}),
            json!({"command": "ls", "background": "yes"}),
            json!({"command": 1}),
            json!("ls"),
        ] {
            assert_eq!(
                Action::parse("shell", &bad).kind,
                ActionKind::Other,
                "{bad}"
            );
        }
    }

    #[test]
    fn upstream_builtins_are_all_classified() {
        use meerkat_tools::builtin::{
            BRAIN_SWAP_TOOL_NAMES, COMMS_TOOL_NAMES, SHELL_TOOL_NAMES, SKILL_TOOL_NAMES,
            UTILITY_TOOL_NAMES, WEB_SEARCH_TOOL_NAMES,
        };
        let upstream = [
            SHELL_TOOL_NAMES,
            COMMS_TOOL_NAMES,
            UTILITY_TOOL_NAMES,
            SKILL_TOOL_NAMES,
            WEB_SEARCH_TOOL_NAMES,
            BRAIN_SWAP_TOOL_NAMES,
            meerkat_tools::builtin::image_generation::TOOL_NAMES,
        ];
        for name in upstream.into_iter().flatten() {
            let read_only = READ_ONLY_BUILTINS.contains(name);
            let reviewed = REVIEWED_BUILTINS.contains(name);
            assert!(
                read_only != reviewed,
                "upstream builtin `{name}` must be in exactly one of READ_ONLY_BUILTINS and REVIEWED_BUILTINS"
            );
        }
        let (_dir, root) = project();
        let ctx = PolicyContext {
            root: &root,
            trust_project_builds: true,
            posix_shell: true,
        };
        let rules = RuleSet::default();
        for name in READ_ONLY_BUILTINS {
            let args = json!({});
            let tier = classify_tier(&Action::parse(name, &args), &rules, &ctx).tier;
            assert_eq!(tier, Tier::AlwaysAllow, "{name}");
        }
        for name in REVIEWED_BUILTINS {
            let args = json!({});
            let tier = classify_tier(&Action::parse(name, &args), &rules, &ctx).tier;
            assert_eq!(tier, Tier::Classifier, "{name}");
        }
    }

    #[test]
    fn tier_table() {
        let (dir, root) = project();
        let outside = dir.path().parent().unwrap().join("x.txt");
        let outside = outside.to_str().unwrap();
        let rules = RuleSet::default();
        let cases: Vec<(&str, Value, bool, Tier)> = vec![
            ("datetime", json!({}), true, Tier::AlwaysAllow),
            ("task_list", json!({}), true, Tier::AlwaysAllow),
            (
                "shell_job_status",
                json!({"job_id": "1"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "read_file",
                json!({"path": "src/main.rs"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "read_file",
                json!({"path": ".env"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "read_file",
                json!({"path": outside}),
                true,
                Tier::Classifier,
            ),
            (
                "read_file",
                json!({"path": "~/.ssh/id_rsa"}),
                true,
                Tier::Classifier,
            ),
            ("read_file", json!({}), true, Tier::Classifier),
            (
                "glob",
                json!({"pattern": "**/*.rs"}),
                true,
                Tier::AlwaysAllow,
            ),
            ("glob", json!({"pattern": "../**"}), true, Tier::Classifier),
            ("glob", json!({"pattern": "/etc/*"}), true, Tier::Classifier),
            (
                "glob",
                json!({"pattern": "*", "path": "/etc"}),
                true,
                Tier::Classifier,
            ),
            (
                "grep",
                json!({"pattern": "x", "path": "src"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "grep",
                json!({"pattern": "x", "path": "/"}),
                true,
                Tier::Classifier,
            ),
            (
                "write_file",
                json!({"path": "src/new.rs", "content": ""}),
                true,
                Tier::InProjectEdit,
            ),
            (
                "edit_file",
                json!({"path": "src/main.rs"}),
                true,
                Tier::InProjectEdit,
            ),
            (
                "edit_file",
                json!({"path": ".rkat/config.toml"}),
                true,
                Tier::Classifier,
            ),
            (
                "write_file",
                json!({"path": ".git/hooks/pre-commit"}),
                true,
                Tier::Classifier,
            ),
            (
                "write_file",
                json!({"path": ".env"}),
                true,
                Tier::Classifier,
            ),
            (
                "write_file",
                json!({"path": "tui.toml"}),
                true,
                Tier::Classifier,
            ),
            (
                "write_file",
                json!({"path": outside}),
                true,
                Tier::Classifier,
            ),
            (
                "apply_patch",
                json!({"patch": "*** Begin Patch\n*** Add File: src/x.rs\n+a\n*** End Patch"}),
                true,
                Tier::InProjectEdit,
            ),
            (
                "apply_patch",
                json!({"patch": "*** Begin Patch\n*** Update File: src/main.rs\n*** Move to: ../out.rs\n*** End Patch"}),
                true,
                Tier::Classifier,
            ),
            (
                "apply_patch",
                json!({"patch": "garbage"}),
                true,
                Tier::Classifier,
            ),
            (
                "shell",
                json!({"command": "ls -la"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "shell",
                json!({"command": "ls", "background": true}),
                true,
                Tier::Classifier,
            ),
            (
                "shell",
                json!({"command": "ls", "working_dir": "/"}),
                true,
                Tier::Classifier,
            ),
            (
                "shell",
                json!({"command": "cargo test"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "shell",
                json!({"command": "cargo test"}),
                false,
                Tier::Classifier,
            ),
            (
                "shell",
                json!({"command": "rm -rf /"}),
                true,
                Tier::Classifier,
            ),
            ("shell", json!({}), true, Tier::Classifier),
            (
                "agent_spawn",
                json!({"prompt": "x"}),
                true,
                Tier::Classifier,
            ),
            (
                "mcp__github__create_issue",
                json!({}),
                true,
                Tier::Classifier,
            ),
            ("unknown_tool", json!({}), true, Tier::Classifier),
            ("memory_list", json!({}), true, Tier::AlwaysAllow),
            (
                "memory_read",
                json!({"scope": "user", "name": "x"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "memory_search",
                json!({"query": "x"}),
                true,
                Tier::AlwaysAllow,
            ),
            (
                "memory_save",
                json!({"scope": "workspace", "name": "x", "content": "y"}),
                true,
                Tier::InProjectEdit,
            ),
            (
                "memory_forget",
                json!({"scope": "workspace", "name": "x"}),
                true,
                Tier::InProjectEdit,
            ),
            (
                "memory_save",
                json!({"scope": "user", "name": "x", "content": "y"}),
                true,
                Tier::Classifier,
            ),
            (
                "memory_forget",
                json!({"scope": "user", "name": "x"}),
                true,
                Tier::Classifier,
            ),
            (
                "memory_save",
                json!({"scope": "global", "name": "x", "content": "y"}),
                true,
                Tier::Classifier,
            ),
            (
                "memory_save",
                json!({"scope": "workspace"}),
                true,
                Tier::Classifier,
            ),
        ];
        for (name, args, trust, expected) in &cases {
            assert_eq!(
                tier(&root, &rules, *trust, name, args),
                *expected,
                "{name} {args}"
            );
        }
    }

    #[test]
    fn user_rules_take_precedence() {
        let (_dir, root) = project();
        let mut rules = RuleSet::default();
        rules.push(
            Rule::new(
                RuleKind::Deny,
                "read_file",
                Some("src/*"),
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        rules.push(
            Rule::new(
                RuleKind::Deny,
                "shell",
                Some("git push*"),
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        rules.push(
            Rule::new(
                RuleKind::Allow,
                "shell",
                Some("make lint"),
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        rules.push(
            Rule::new(
                RuleKind::Allow,
                "mcp__docs__search",
                None,
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        assert_eq!(
            tier(
                &root,
                &rules,
                true,
                "read_file",
                &json!({"path": "src/main.rs"})
            ),
            Tier::UserDeny
        );
        assert_eq!(
            tier(
                &root,
                &rules,
                true,
                "shell",
                &json!({"command": "git push --force"})
            ),
            Tier::UserDeny
        );
        assert_eq!(
            tier(
                &root,
                &rules,
                true,
                "shell",
                &json!({"command": "make lint"})
            ),
            Tier::AlwaysAllow
        );
        assert_eq!(
            tier(&root, &rules, true, "mcp__docs__search", &json!({"q": "x"})),
            Tier::AlwaysAllow
        );
        let ctx = PolicyContext {
            root: &root,
            trust_project_builds: true,
            posix_shell: true,
        };
        let args = json!({"command": "make lint"});
        let decision = classify_tier(&Action::parse("shell", &args), &rules, &ctx);
        assert_eq!(
            decision.rule.map(|r| r.to_string()).as_deref(),
            Some("allow#1")
        );
    }

    #[test]
    fn subjects_use_display_paths() {
        let (dir, root) = project();
        let abs = dir.path().join("src/main.rs");
        let args = json!({"path": abs.to_str().unwrap()});
        let action = Action::parse("read_file", &args);
        assert_eq!(action.subjects(&root), ["src/main.rs"]);
        let args = json!({"x": 1});
        assert_eq!(
            Action::parse("mcp__a__b", &args).subjects(&root),
            ["{\n  \"x\": 1\n}"]
        );
    }

    #[test]
    fn memory_writes_parse_and_match_rules() {
        let (_dir, root) = project();
        let args = json!({"scope": "user", "name": "Prefers Rust!", "content": "x"});
        let action = Action::parse("memory_save", &args);
        assert_eq!(
            action.kind,
            ActionKind::MemoryWrite {
                scope: Scope::User,
                name: "Prefers Rust!"
            }
        );
        assert_eq!(action.subjects(&root), ["user/prefers-rust"]);
        let ctx = PolicyContext {
            root: &root,
            trust_project_builds: true,
            posix_shell: true,
        };
        let decision = classify_tier(&action, &RuleSet::default(), &ctx);
        assert_eq!(decision.tier, Tier::Classifier);
        assert_eq!(
            decision.detail,
            "user-level memory persists into every project's system prompt"
        );
        let ws = json!({"scope": "workspace", "name": "x", "content": "y"});
        let decision = classify_tier(
            &Action::parse("memory_save", &ws),
            &RuleSet::default(),
            &ctx,
        );
        assert_eq!(decision.detail, "workspace memory note");

        let mut rules = RuleSet::default();
        rules.push(
            Rule::new(
                RuleKind::Allow,
                "memory_save",
                Some("user/*"),
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        assert_eq!(
            tier(&root, &rules, true, "memory_save", &args),
            Tier::AlwaysAllow
        );
        let mut rules = RuleSet::default();
        rules.push(
            Rule::new(
                RuleKind::Deny,
                "memory_save",
                Some("workspace/*"),
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        assert_eq!(
            tier(&root, &rules, true, "memory_save", &ws),
            Tier::UserDeny
        );
    }
}
