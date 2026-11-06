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
use super::catalog::{ToolFamily, ToolOrigin};
use crate::memory::{NOTE_FORGET, NOTE_LIST, NOTE_READ, NOTE_SAVE, Scope, slug};
use crate::paths::ProjectRoot;
use crate::tools::{EDIT_FILE, GLOB, GREP, READ_FILE, WRITE_FILE};
use meerkat_core::ToolMutationClass;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use std::fmt;

/// Policy tier, first match wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Tier 0: a user deny rule matched.
    UserDeny,
    /// Tier 0: a file tool targets a sealed path (the gate's own `.rkat/`
    /// state, the realm and user config directories, or a git directory
    /// governing the project, inside it or not; see
    /// [`crate::paths::ResolvedPath::sealed`]); denied before any rule or
    /// classifier.
    Sealed,
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
            Self::Sealed => "sealed path",
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

/// Tools that hand work to a helper (a mob member, see [`crate::mobs`]; the
/// TUI's own `fork`, see [`crate::fork`]). Delegating does nothing by
/// itself: every call the helper makes passes the delegating session's
/// gate.
pub const DELEGATION_TOOLS: &[&str] = &["delegate", "fork_off", crate::fork::FORK];

/// What a tool call does, as far as the policy is concerned.
///
/// Built by [`Action::parse`] from the call's raw arguments, deserialized
/// once into the tool's typed argument struct. A name counts as a builtin,
/// file or note tool only when the call's [`ToolOrigin`] says the session's
/// tool of that name comes from there ([`ToolFamily`]); anything else, and
/// any argument that does not parse, is [`ActionKind::Other`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
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
        path: String,
    },
    /// `glob` / `grep`.
    Search {
        /// Search root (defaults to the project root).
        path: Option<String>,
        /// `glob`'s pattern, which can itself name a directory.
        pattern: Option<String>,
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
    /// `note_list` / `note_read`.
    NoteRead,
    /// `note_save` / `note_forget` with a valid scope.
    NoteWrite {
        /// Note scope.
        scope: Scope,
        /// Note name as supplied (rule subjects use its slug when valid).
        name: String,
    },
    /// Anything else, including foreign tools and malformed arguments.
    Other,
}

/// A parsed tool call.
#[derive(Debug, Clone)]
pub struct Action<'a> {
    /// Tool name.
    pub name: &'a str,
    /// Raw arguments, exactly as the model sent them.
    pub args: &'a RawValue,
    /// Implementation family of the session's tool of this name.
    pub family: ToolFamily,
    /// Policy-relevant interpretation.
    pub kind: ActionKind,
}

/// Typed arguments of `read_file`.
#[derive(Deserialize)]
struct PathArgs {
    path: String,
}

/// Typed arguments of `glob` / `grep` (only the fields the policy reads).
#[derive(Deserialize)]
struct SearchArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
}

/// Typed arguments of `apply_patch`.
#[derive(Deserialize)]
struct PatchArgs {
    patch: String,
}

/// Typed arguments of `note_save` / `note_forget`.
#[derive(Deserialize)]
struct NoteArgs {
    scope: Scope,
    name: String,
}

fn typed<T: serde::de::DeserializeOwned>(args: &RawValue) -> Option<T> {
    serde_json::from_str(args.get()).ok()
}

impl<'a> Action<'a> {
    /// Interpret a call of `name` with raw `args`, whose session tool comes
    /// from `origin` (`None`: the session does not offer it, or its
    /// definitions could not be read). Malformed arguments, foreign tools
    /// and a declared mutation class that contradicts a read-only family
    /// yield [`ActionKind::Other`], which always reaches the classifier.
    pub fn parse(name: &'a str, args: &'a RawValue, origin: Option<&ToolOrigin>) -> Self {
        let family = origin.map_or(ToolFamily::Foreign, ToolOrigin::family);
        let mutation = origin.map_or(ToolMutationClass::Unknown, |o| o.mutation);
        let kind = Self::kind(name, args, family, mutation);
        Self {
            name,
            args,
            family,
            kind,
        }
    }

    fn kind(
        name: &str,
        args: &RawValue,
        family: ToolFamily,
        mutation: ToolMutationClass,
    ) -> ActionKind {
        let declared_mutating = mutation == ToolMutationClass::Mutating;
        match (family, name) {
            (ToolFamily::Native, "shell") => {
                typed::<ShellArgs>(args).map_or(ActionKind::Other, |input| ActionKind::Shell {
                    command: input.command,
                    working_dir: input.working_dir,
                    background: input.background,
                })
            }
            (ToolFamily::Native, "apply_patch") => {
                match typed::<PatchArgs>(args).map(|a| patch_targets(&a.patch)) {
                    Some(Ok(paths)) if !paths.is_empty() => ActionKind::FileWrite { paths },
                    _ => ActionKind::Other,
                }
            }
            (ToolFamily::Native, n) if READ_ONLY_BUILTINS.contains(&n) && !declared_mutating => {
                ActionKind::ReadOnlyBuiltin
            }
            (ToolFamily::Native | ToolFamily::Fork, n) if DELEGATION_TOOLS.contains(&n) => {
                ActionKind::Delegation
            }
            (ToolFamily::Files, READ_FILE) if !declared_mutating => typed::<PathArgs>(args)
                .map_or(ActionKind::Other, |a| ActionKind::FileRead { path: a.path }),
            (ToolFamily::Files, GLOB | GREP) if !declared_mutating => typed::<SearchArgs>(args)
                .map_or(ActionKind::Other, |a| ActionKind::Search {
                    path: a.path,
                    pattern: if name == GLOB { a.pattern } else { None },
                }),
            (ToolFamily::Files, WRITE_FILE | EDIT_FILE) => typed::<PathArgs>(args)
                .map_or(ActionKind::Other, |a| ActionKind::FileWrite {
                    paths: vec![a.path],
                }),
            (ToolFamily::Notes, NOTE_LIST | NOTE_READ) if !declared_mutating => {
                ActionKind::NoteRead
            }
            (ToolFamily::Notes, NOTE_SAVE | NOTE_FORGET) => typed::<NoteArgs>(args)
                .map_or(ActionKind::Other, |a| ActionKind::NoteWrite {
                    scope: a.scope,
                    name: a.name,
                }),
            _ => ActionKind::Other,
        }
    }

    /// Canonical rule subjects: the command for `shell`, display paths for file
    /// tools, `<scope>/<slug>` for note writes, pretty JSON for everything else.
    /// Computed once per decision by the engine.
    pub fn subjects(&self, root: &ProjectRoot) -> Vec<String> {
        let display = |raw: &str| match root.resolve(raw) {
            Ok(resolved) => root.display_of(&resolved).to_string(),
            Err(_) => raw.to_string(),
        };
        match &self.kind {
            ActionKind::Shell { command, .. } => vec![command.clone()],
            ActionKind::FileRead { path } => vec![display(path)],
            ActionKind::Search { path, .. } => {
                vec![path.as_deref().map_or_else(|| ".".to_string(), display)]
            }
            ActionKind::FileWrite { paths } => paths.iter().map(|p| display(p)).collect(),
            ActionKind::NoteWrite { scope, name } => {
                let name = slug(name).unwrap_or_else(|_| name.clone());
                vec![format!("{scope}/{name}")]
            }
            ActionKind::ReadOnlyBuiltin
            | ActionKind::Delegation
            | ActionKind::NoteRead
            | ActionKind::Other => vec![pretty_args(self.args)],
        }
    }
}

/// Pretty JSON of raw arguments (their raw text when they are not JSON).
#[must_use]
pub fn pretty_args(args: &RawValue) -> String {
    serde_json::from_str::<Value>(args.get())
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| args.get().to_string())
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
/// sync with it (pinned against the live tool schema by the `drift_pins`
/// test). Unknown argument names are rejected (`deny_unknown_fields`): the
/// tool would ignore them, which makes their purpose unclear, so such calls
/// go to the classifier.
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
    classify_tier_with(action, &action.subjects(ctx.root), rules, ctx)
}

/// [`classify_tier`] with the action's subjects already computed
/// ([`Action::subjects`]), so a decision computes them once.
pub fn classify_tier_with(
    action: &Action<'_>,
    subjects: &[String],
    rules: &RuleSet,
    ctx: &PolicyContext<'_>,
) -> TierDecision {
    if let Some(id) = rules.matches_deny(action.name, subjects) {
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

    if let Some(id) = rules.matches_allow(action.name, subjects) {
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
        ActionKind::NoteRead => Ok(TierDecision::new(Tier::AlwaysAllow, "note read")),
        ActionKind::NoteWrite { scope, .. } => {
            if *scope == Scope::Workspace {
                Ok(TierDecision::new(Tier::InProjectEdit, "workspace note"))
            } else {
                Err("user-level notes persist into every project's system prompt".to_string())
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
            if let Some(path) = path.as_deref() {
                require_inside(ctx.root, path)?;
            }
            if let Some(pattern) = pattern.as_deref()
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
            // Every target is checked for a seal before any other verdict:
            // an earlier outside or protected target must not hide a later
            // sealed one behind the classifier or an allow rule.
            for path in paths {
                if ctx.root.resolve(path).is_ok_and(|resolved| resolved.sealed) {
                    return Ok(TierDecision::new(
                        Tier::Sealed,
                        format!(
                            "`{path}` is sealed: file tools never write the gate's own state \
                             (`.rkat/`, the realm and user config directories) or the git directories"
                        ),
                    ));
                }
            }
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
        ActionKind::Other if action.family == ToolFamily::Foreign => Err(format!(
            "`{}` is not a tool the TUI or the engine provides",
            action.name
        )),
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

    /// The origin a real TUI session gives `name` (test fixture: the
    /// production gate reads origins from the session's live tool
    /// definitions).
    fn origin(name: &str) -> Option<ToolOrigin> {
        use crate::memory::{NOTES_SOURCE, NOTE_TOOL_NAMES};
        use crate::tools::{FILE_TOOL_NAMES, FILE_TOOLS_SOURCE};
        use meerkat_core::ToolSourceKind;
        if FILE_TOOL_NAMES.contains(&name) {
            Some(ToolOrigin::tui(FILE_TOOLS_SOURCE, name))
        } else if NOTE_TOOL_NAMES.contains(&name) {
            Some(ToolOrigin::tui(NOTES_SOURCE, name))
        } else if name == crate::fork::FORK {
            Some(ToolOrigin::tui(crate::fork::FORK_SOURCE, name))
        } else if let Some(rest) = name.strip_prefix("mcp__") {
            Some(ToolOrigin::mcp(rest.split("__").next().unwrap_or_default()))
        } else if matches!(name, "unknown_tool" | "agent_spawn") {
            None
        } else {
            Some(ToolOrigin::native(ToolSourceKind::Builtin))
        }
    }

    fn raw(args: &Value) -> Box<RawValue> {
        serde_json::value::to_raw_value(args).unwrap()
    }

    fn parse<'a>(name: &'a str, args: &'a RawValue) -> Action<'a> {
        Action::parse(name, args, origin(name).as_ref())
    }

    fn tier(root: &ProjectRoot, rules: &RuleSet, trust: bool, name: &str, args: &Value) -> Tier {
        let ctx = PolicyContext {
            root,
            trust_project_builds: trust,
            posix_shell: true,
        };
        classify_tier(&parse(name, &raw(args)), rules, &ctx).tier
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
        let raw_args = raw(&args);
        let action = parse("apply_patch", &raw_args);
        assert_eq!(
            action.kind,
            ActionKind::FileWrite {
                paths: vec!["src/ok.rs".into(), ".git/config".into()]
            }
        );
        assert_eq!(
            tier(&root, &RuleSet::default(), false, "apply_patch", &args),
            Tier::Sealed
        );
    }

    /// A sealed target later in a patch seals it, whatever comes before it
    /// (a protected `build.rs`, an outside `/tmp` path) and under an allow
    /// rule for the tool.
    #[test]
    fn a_later_sealed_patch_target_seals_the_patch() {
        let (_dir, root) = project();
        let mut rules = RuleSet::default();
        rules.push(Rule::new(RuleKind::Allow, "apply_patch", Some("*"), RuleOrigin::Realm).unwrap());
        for patch in [
            "*** Begin Patch\n*** Add File: build.rs\n+x\n*** Update File: .git/config\n@@\n-a\n+b\n*** End Patch",
            "*** Begin Patch\n*** Add File: /tmp/x\n+x\n*** Update File: .rkat/config.toml\n@@\n-a\n+b\n*** End Patch",
        ] {
            let args = json!({ "patch": patch });
            for set in [&RuleSet::default(), &rules] {
                assert_eq!(tier(&root, set, false, "apply_patch", &args), Tier::Sealed, "{patch}");
            }
        }
    }

    #[test]
    fn delegation_and_mob_queries_need_no_review_but_mob_changes_do() {
        let (_dir, root) = project();
        let task = json!({"member_id": "scout", "task": "rm -rf /", "result_label": "r", "max_text_bytes": 10});
        assert_eq!(
            parse("delegate", &raw(&task)).kind,
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
    fn fork_is_tiered_like_delegate_only_from_the_tuis_own_surface() {
        use crate::fork::{FORK, FORK_SOURCE};

        let (_dir, root) = project();
        let task = json!({"task": "rm -rf /", "expected_output": "done"});
        let delegate = json!({"member_id": "scout", "task": "rm -rf /", "result_label": "r", "max_text_bytes": 10});
        assert_eq!(parse(FORK, &raw(&task)).kind, ActionKind::Delegation);
        assert_eq!(
            tier(&root, &RuleSet::default(), false, FORK, &task),
            tier(&root, &RuleSet::default(), false, "delegate", &delegate),
        );
        assert_eq!(
            tier(&root, &RuleSet::default(), false, FORK, &task),
            Tier::AlwaysAllow
        );
        // Another surface's `fork` is a foreign tool.
        let args = raw(&task);
        let foreign = ToolOrigin::mcp("evil");
        assert_eq!(
            Action::parse(FORK, &args, Some(&foreign)).kind,
            ActionKind::Other
        );
        let callback = ToolOrigin::tui("someone-else", FORK);
        assert_ne!(callback.family(), ToolFamily::Fork);
        assert_eq!(ToolOrigin::tui(FORK_SOURCE, FORK).family(), ToolFamily::Fork);
    }

    #[test]
    fn shell_args_use_the_tool_type() {
        let args = json!({"command": "ls", "working_dir": "src", "timeout_secs": 5});
        assert_eq!(
            parse("shell", &raw(&args)).kind,
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
                parse("shell", &raw(&bad)).kind,
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
            let tier = classify_tier(&parse(name, &raw(&args)), &rules, &ctx).tier;
            assert_eq!(tier, Tier::AlwaysAllow, "{name}");
        }
        for name in REVIEWED_BUILTINS {
            let args = json!({});
            let tier = classify_tier(&parse(name, &raw(&args)), &rules, &ctx).tier;
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
                Tier::Sealed,
            ),
            (
                "write_file",
                json!({"path": ".git/hooks/pre-commit"}),
                true,
                Tier::Sealed,
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
            ("note_list", json!({}), true, Tier::AlwaysAllow),
            (
                "note_read",
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
                "note_save",
                json!({"scope": "workspace", "name": "x", "content": "y"}),
                true,
                Tier::InProjectEdit,
            ),
            (
                "note_forget",
                json!({"scope": "workspace", "name": "x"}),
                true,
                Tier::InProjectEdit,
            ),
            (
                "note_save",
                json!({"scope": "user", "name": "x", "content": "y"}),
                true,
                Tier::Classifier,
            ),
            (
                "note_forget",
                json!({"scope": "user", "name": "x"}),
                true,
                Tier::Classifier,
            ),
            (
                "note_save",
                json!({"scope": "global", "name": "x", "content": "y"}),
                true,
                Tier::Classifier,
            ),
            (
                "note_save",
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
        let decision = classify_tier(&parse("shell", &raw(&args)), &rules, &ctx);
        assert_eq!(
            decision.rule.map(|r| r.to_string()).as_deref(),
            Some("allow#3")
        );
    }

    #[test]
    fn subjects_use_display_paths() {
        let (dir, root) = project();
        let abs = dir.path().join("src/main.rs");
        let args = json!({"path": abs.to_str().unwrap()});
        let raw_args = raw(&args);
        let action = parse("read_file", &raw_args);
        assert_eq!(action.subjects(&root), ["src/main.rs"]);
        let args = json!({"x": 1});
        assert_eq!(
            parse("mcp__a__b", &raw(&args)).subjects(&root),
            ["{\n  \"x\": 1\n}"]
        );
    }

    #[test]
    fn note_writes_parse_and_match_rules() {
        let (_dir, root) = project();
        let args = json!({"scope": "user", "name": "Prefers Rust!", "content": "x"});
        let raw_args = raw(&args);
        let action = parse("note_save", &raw_args);
        assert_eq!(
            action.kind,
            ActionKind::NoteWrite {
                scope: Scope::User,
                name: "Prefers Rust!".into()
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
            "user-level notes persist into every project's system prompt"
        );
        let ws = json!({"scope": "workspace", "name": "x", "content": "y"});
        let decision = classify_tier(
            &parse("note_save", &raw(&ws)),
            &RuleSet::default(),
            &ctx,
        );
        assert_eq!(decision.detail, "workspace note");

        let mut rules = RuleSet::default();
        rules.push(
            Rule::new(
                RuleKind::Allow,
                "note_save",
                Some("user/*"),
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        assert_eq!(
            tier(&root, &rules, true, "note_save", &args),
            Tier::AlwaysAllow
        );
        let mut rules = RuleSet::default();
        rules.push(
            Rule::new(
                RuleKind::Deny,
                "note_save",
                Some("workspace/*"),
                RuleOrigin::Realm,
            )
            .unwrap(),
        );
        assert_eq!(
            tier(&root, &rules, true, "note_save", &ws),
            Tier::UserDeny
        );
    }

    #[test]
    fn names_do_not_confer_tiers_without_provenance() {
        let (_dir, root) = project();
        let ctx = PolicyContext {
            root: &root,
            trust_project_builds: true,
            posix_shell: true,
        };
        let rules = RuleSet::default();
        let decide = |name: &str, args: &Value, origin: Option<ToolOrigin>| {
            let args = raw(args);
            let action = Action::parse(name, &args, origin.as_ref());
            (action.kind.clone(), classify_tier(&action, &rules, &ctx).tier)
        };
        // An MCP server's tools named like a builtin, a file tool and a note tool.
        for (name, args) in [
            ("task_list", json!({})),
            ("read_file", json!({"path": "src/main.rs"})),
            ("write_file", json!({"path": "src/x.rs", "content": ""})),
            ("note_save", json!({"scope": "workspace", "name": "x", "content": ""})),
            ("shell", json!({"command": "ls"})),
        ] {
            assert_eq!(
                decide(name, &args, Some(ToolOrigin::mcp("evil"))),
                (ActionKind::Other, Tier::Classifier),
                "{name}"
            );
            // A tool the session does not offer at all.
            assert_eq!(
                decide(name, &args, None),
                (ActionKind::Other, Tier::Classifier),
                "{name}"
            );
        }
        // A foreign callback surface reusing the TUI's tool name.
        let other = ToolOrigin {
            provenance: Some(crate::tools::provenance("someone-else")),
            mutation: ToolMutationClass::ReadOnly,
        };
        assert_eq!(
            decide("read_file", &json!({"path": "src/main.rs"}), Some(other)).1,
            Tier::Classifier
        );
        // A builtin read-only name whose declaration says it mutates.
        let mutating = ToolOrigin {
            mutation: ToolMutationClass::Mutating,
            ..ToolOrigin::native(meerkat_core::ToolSourceKind::Builtin)
        };
        assert_eq!(
            decide("task_list", &json!({}), Some(mutating)).1,
            Tier::Classifier
        );
        // Unparsable arguments never become a string payload of a known kind.
        let garbage = serde_json::value::RawValue::from_string("[1,2".into());
        assert!(garbage.is_err(), "the transport only delivers JSON");
        assert_eq!(
            decide("read_file", &json!("src/main.rs"), origin("read_file")),
            (ActionKind::Other, Tier::Classifier)
        );
    }
}
