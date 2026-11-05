//! User allow/deny rules and their TOML persistence.
//!
//! Rules match a tool name ([`ToolPattern`]) and optionally a glob over the
//! action's canonical subject ([`ArgPattern`]): the command string for `shell`,
//! the display path for file tools, pretty JSON for everything else.
//!
//! Three files are layered: `~/.rkat/tui/tui.toml` (global, see
//! [`crate::bootstrap::GlobalDirs`]), `<realm>/tui.toml`
//! (per-workspace, the default write target) and `<project>/.rkat/tui.toml`
//! (committed with the repository, therefore untrusted: it may only add deny
//! rules and `[auto]` settings, may not loosen `trust_project_builds`, and
//! its endpoints are ignored). The realm file is fully applied only when
//! the host trust record says rkat-tui wrote it or the user trusted it
//! ([`crate::approval::trust`]); otherwise only its deny rules apply. The command-line overlay ([`crate::overlay`]) is a
//! fourth layer between the realm and project files. Besides rules and
//! `[auto]`, the file holds `[endpoints.<name>]` (see [`crate::endpoints`])
//! and `[pricing."<model id>"]` price overrides (see [`crate::pricing`]), and
//! `[confinement] extra_read`/`extra_write` sandbox grants (see
//! [`crate::bootstrap::confinement`]); like endpoints and prices, those are
//! honoured from the user's own layers only.

use crate::approval::trust::{TrustStatus, Untrusted};
use crate::bootstrap::confinement::ConfinementSection;
use crate::ui::compaction::{CompactMode, CompactionPolicy};
use globset::{Glob, GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// File name of every rules layer.
pub const RULES_FILE_NAME: &str = "tui.toml";

/// Errors from rule parsing and persistence.
#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    /// A rules file could not be written.
    #[error("cannot write {}: {source}", path.display())]
    Write {
        /// The rules file.
        path: PathBuf,
        /// Why.
        #[source]
        source: crate::paths::AtomicWriteError,
    },
    /// A glob pattern failed to compile.
    #[error("invalid pattern `{pattern}`: {source}")]
    Pattern {
        /// The offending pattern.
        pattern: String,
        /// Underlying globset error.
        #[source]
        source: globset::Error,
    },
    /// A tool pattern was empty.
    #[error("tool pattern is empty")]
    EmptyTool,
    /// Reading or writing a rules file failed.
    #[error("{path}: {source}")]
    Io {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A rules file is not valid TOML for [`RulesFile`].
    #[error("{path}: {source}")]
    Parse {
        /// File path.
        path: PathBuf,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },
    /// Serializing a rules file failed.
    #[error("failed to serialize rules: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// A rule entry set both `arg` and `literal`.
    #[error("rule for `{tool}` sets both `arg` and `literal`")]
    ArgAndLiteral {
        /// The rule's tool pattern.
        tool: String,
    },
}

/// Whether a rule allows or denies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Allow without review.
    Allow,
    /// Deny outright.
    Deny,
}

/// Which layer a rule came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOrigin {
    /// `~/.rkat/tui.toml`.
    Global,
    /// `<realm>/tui.toml`.
    Realm,
    /// `<project>/.rkat/tui.toml`.
    Project,
    /// Added during this process (not yet persisted).
    Session,
    /// From the command line (`--set`, `--allow`, `--deny`, …); never persisted.
    Cli,
}

impl RuleOrigin {
    /// Lower-case label (`global`, `realm`, `project`, `session`, `cli`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Realm => "realm",
            Self::Project => "project",
            Self::Session => "session",
            Self::Cli => "cli",
        }
    }
}

/// Tool-name matcher: an exact name or a glob such as `mcp__github__*`.
#[derive(Clone)]
pub struct ToolPattern {
    raw: String,
    glob: Option<GlobMatcher>,
}

impl ToolPattern {
    /// Parse a tool pattern.
    ///
    /// # Errors
    ///
    /// Returns [`RulesError`] for empty or invalid glob patterns.
    pub fn new(raw: impl Into<String>) -> Result<Self, RulesError> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            return Err(RulesError::EmptyTool);
        }
        let glob = if has_glob_meta(&raw) {
            Some(compile(&raw)?)
        } else {
            None
        };
        Ok(Self { raw, glob })
    }

    /// The pattern as written.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Whether the pattern is a plain tool name.
    pub fn is_exact(&self) -> bool {
        self.glob.is_none()
    }

    /// Whether `tool` matches.
    pub fn matches(&self, tool: &str) -> bool {
        match &self.glob {
            Some(glob) => glob.is_match(tool),
            None => self.raw == tool,
        }
    }
}

impl PartialEq for ToolPattern {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Eq for ToolPattern {}

impl fmt::Debug for ToolPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ToolPattern").field(&self.raw).finish()
    }
}

/// Matcher over the canonical action subject.
#[derive(Clone)]
pub enum ArgPattern {
    /// Glob (`*` also matches `/` and spaces).
    Glob {
        /// The pattern as written.
        raw: String,
        /// Compiled matcher.
        matcher: GlobMatcher,
    },
    /// The exact subject string (what "allow + add rule" mints).
    Literal(String),
}

impl ArgPattern {
    /// Parse a glob argument pattern.
    ///
    /// # Errors
    ///
    /// Returns [`RulesError::Pattern`] when the glob does not compile.
    pub fn new(raw: impl Into<String>) -> Result<Self, RulesError> {
        let raw = raw.into();
        let matcher = compile(&raw)?;
        Ok(Self::Glob { raw, matcher })
    }

    /// The pattern (or literal) as written.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Glob { raw, .. } => raw,
            Self::Literal(text) => text,
        }
    }

    /// Whether this is a [`Self::Literal`].
    pub fn is_literal(&self) -> bool {
        matches!(self, Self::Literal(_))
    }

    /// Whether `subject` matches.
    pub fn matches(&self, subject: &str) -> bool {
        match self {
            Self::Glob { matcher, .. } => matcher.is_match(subject),
            Self::Literal(text) => text == subject,
        }
    }
}

impl PartialEq for ArgPattern {
    fn eq(&self, other: &Self) -> bool {
        self.is_literal() == other.is_literal() && self.as_str() == other.as_str()
    }
}

impl Eq for ArgPattern {}

impl fmt::Debug for ArgPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Glob { raw, .. } => f.debug_tuple("Glob").field(raw).finish(),
            Self::Literal(text) => f.debug_tuple("Literal").field(text).finish(),
        }
    }
}

impl fmt::Display for ArgPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Glob { raw, .. } => f.write_str(raw),
            Self::Literal(text) => write!(f, "={text:?}"),
        }
    }
}

fn has_glob_meta(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

fn compile(pattern: &str) -> Result<GlobMatcher, RulesError> {
    let glob: Glob = GlobBuilder::new(pattern)
        .literal_separator(false)
        .backslash_escape(true)
        .build()
        .map_err(|source| RulesError::Pattern {
            pattern: pattern.to_string(),
            source,
        })?;
    Ok(glob.compile_matcher())
}

/// One allow or deny rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Tool-name matcher.
    pub tool: ToolPattern,
    /// Optional subject matcher; `None` matches every invocation of the tool.
    pub arg: Option<ArgPattern>,
    /// Allow or deny.
    pub kind: RuleKind,
    /// Layer the rule came from.
    pub origin: RuleOrigin,
}

impl Rule {
    /// Build a rule from raw patterns.
    ///
    /// # Errors
    ///
    /// Returns [`RulesError`] when a pattern is invalid.
    pub fn new(
        kind: RuleKind,
        tool: &str,
        arg: Option<&str>,
        origin: RuleOrigin,
    ) -> Result<Self, RulesError> {
        Ok(Self {
            tool: ToolPattern::new(tool)?,
            arg: arg.map(ArgPattern::new).transpose()?,
            kind,
            origin,
        })
    }

    /// A rule matching exactly one subject string (no glob semantics).
    ///
    /// # Errors
    ///
    /// Returns [`RulesError`] when the tool pattern is invalid.
    pub fn literal(
        kind: RuleKind,
        tool: &str,
        subject: impl Into<String>,
        origin: RuleOrigin,
    ) -> Result<Self, RulesError> {
        Ok(Self {
            tool: ToolPattern::new(tool)?,
            arg: Some(ArgPattern::Literal(subject.into())),
            kind,
            origin,
        })
    }

    fn from_entry(
        kind: RuleKind,
        entry: &RuleEntry,
        origin: RuleOrigin,
    ) -> Result<Self, RulesError> {
        match (&entry.arg, &entry.literal) {
            (Some(_), Some(_)) => Err(RulesError::ArgAndLiteral {
                tool: entry.tool.clone(),
            }),
            (None, Some(literal)) => Self::literal(kind, &entry.tool, literal.clone(), origin),
            (arg, None) => Self::new(kind, &entry.tool, arg.as_deref(), origin),
        }
    }

    /// Whether this rule matches the tool and subjects.
    ///
    /// Deny rules match when **any** subject matches; allow rules require
    /// **every** subject to match (an `apply_patch` touching two files is
    /// allowed only when both are covered). For `shell`, the same applies per
    /// simple command of the command line (see [`super::shell_subject`]).
    pub fn matches(&self, tool: &str, subjects: &[String]) -> bool {
        if !self.tool.matches(tool) {
            return false;
        }
        let Some(arg) = &self.arg else {
            return true;
        };
        let shell = tool == "shell";
        match self.kind {
            RuleKind::Deny => subjects.iter().any(|s| {
                if shell {
                    super::shell_subject::deny_matches(arg, s)
                } else {
                    arg.matches(s)
                }
            }),
            RuleKind::Allow => {
                !subjects.is_empty()
                    && subjects.iter().all(|s| {
                        if shell {
                            super::shell_subject::allow_matches(arg, s)
                        } else {
                            arg.matches(s)
                        }
                    })
            }
        }
    }

    /// The serialized form.
    pub fn to_entry(&self) -> RuleEntry {
        let (arg, literal) = match &self.arg {
            Some(ArgPattern::Glob { raw, .. }) => (Some(raw.clone()), None),
            Some(ArgPattern::Literal(text)) => (None, Some(text.clone())),
            None => (None, None),
        };
        RuleEntry {
            tool: self.tool.as_str().to_string(),
            arg,
            literal,
        }
    }

    /// Why this rule is too broad to keep in auto mode, if it is.
    pub fn broad_reason(&self) -> Option<&'static str> {
        if self.kind != RuleKind::Allow {
            return None;
        }
        let tool = self.tool.as_str();
        if tool.trim_matches('*').is_empty() {
            return Some("matches every tool");
        }
        if let Some(rest) = tool.strip_prefix("mcp__")
            && !self.tool.is_exact()
        {
            let tool_part = rest.split_once("__").map_or("", |(_, t)| t);
            if tool_part.trim_matches('*').is_empty() {
                return Some("matches every tool of an MCP server");
            }
        }
        if !self.tool.matches("shell") {
            return None;
        }
        let Some(arg) = &self.arg else {
            return Some("allows every shell command");
        };
        broad_shell_arg(arg.as_str(), !arg.is_literal())
    }
}

/// Interpreters whose wildcard allow rules amount to arbitrary code execution.
const INTERPRETER_PREFIXES: &[&str] = &[
    "python", "node", "bash", "sh", "zsh", "ruby", "perl", "deno", "bun",
];

/// Task runners that execute project-defined scripts.
const RUNNER_PREFIXES: &[&[&str]] = &[
    &["npm", "run"],
    &["npm", "exec"],
    &["npx"],
    &["pnpm"],
    &["yarn"],
    &["cargo", "run"],
    &["make"],
];

fn broad_shell_arg(arg: &str, glob: bool) -> Option<&'static str> {
    let trimmed = arg.trim();
    if trimmed.is_empty() || (glob && trimmed.starts_with(['*', '?', '[', '{'])) {
        return Some("matches any shell command");
    }
    let mut words = trimmed.split_whitespace();
    let first = words.next().unwrap_or_default();
    let first_bare = first.trim_end_matches(['*', '?']);
    let wildcard = glob && has_glob_meta(trimmed);
    if wildcard
        && INTERPRETER_PREFIXES.iter().any(|p| {
            if *p == "python" {
                first_bare.starts_with(p)
            } else {
                first_bare == *p
            }
        })
    {
        return Some("runs an interpreter on arbitrary input");
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    for prefix in RUNNER_PREFIXES {
        let matches = prefix.iter().enumerate().all(|(i, p)| {
            tokens
                .get(i)
                .is_some_and(|t| t.trim_end_matches(['*', '?']) == *p)
        });
        if matches {
            return Some("runs project-defined scripts");
        }
    }
    None
}

/// Identifies a rule inside a [`RuleSet`]: a serial number assigned when
/// the rule is added, never reused and never shifted by removing other
/// rules, so an id shown in a denial still names the same rule later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleId {
    /// Which list.
    pub kind: RuleKind,
    /// Serial number (from 1, shared by both lists).
    pub serial: u64,
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            RuleKind::Allow => "allow",
            RuleKind::Deny => "deny",
        };
        write!(f, "{kind}#{}", self.serial)
    }
}

/// Merged rules from every layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSet {
    allow: Vec<(u64, Rule)>,
    deny: Vec<(u64, Rule)>,
    /// Last serial handed out.
    last: u64,
}

impl RuleSet {
    fn list(&self, kind: RuleKind) -> &[(u64, Rule)] {
        match kind {
            RuleKind::Allow => &self.allow,
            RuleKind::Deny => &self.deny,
        }
    }

    fn list_mut(&mut self, kind: RuleKind) -> &mut Vec<(u64, Rule)> {
        match kind {
            RuleKind::Allow => &mut self.allow,
            RuleKind::Deny => &mut self.deny,
        }
    }

    /// Allow rules with their ids, in order.
    pub fn allow(&self) -> impl ExactSizeIterator<Item = (RuleId, &Rule)> {
        self.with_ids(RuleKind::Allow)
    }

    /// Deny rules with their ids, in order.
    pub fn deny(&self) -> impl ExactSizeIterator<Item = (RuleId, &Rule)> {
        self.with_ids(RuleKind::Deny)
    }

    fn with_ids(&self, kind: RuleKind) -> impl ExactSizeIterator<Item = (RuleId, &Rule)> {
        self.list(kind)
            .iter()
            .map(move |(serial, rule)| (RuleId { kind, serial: *serial }, rule))
    }

    fn first_match(&self, kind: RuleKind, tool: &str, subjects: &[String]) -> Option<RuleId> {
        self.with_ids(kind)
            .find(|(_, r)| r.matches(tool, subjects))
            .map(|(id, _)| id)
    }

    /// First deny rule matching the action.
    pub fn matches_deny(&self, tool: &str, subjects: &[String]) -> Option<RuleId> {
        self.first_match(RuleKind::Deny, tool, subjects)
    }

    /// First allow rule matching the action.
    pub fn matches_allow(&self, tool: &str, subjects: &[String]) -> Option<RuleId> {
        self.first_match(RuleKind::Allow, tool, subjects)
    }

    /// Look up a rule by id.
    pub fn get(&self, id: RuleId) -> Option<&Rule> {
        self.list(id.kind)
            .iter()
            .find(|(serial, _)| *serial == id.serial)
            .map(|(_, rule)| rule)
    }

    /// Add a rule to the matching list (ignores exact duplicates). Returns
    /// the id of the rule (the existing one for a duplicate).
    pub fn push(&mut self, rule: Rule) -> RuleId {
        let kind = rule.kind;
        if let Some((serial, _)) = self
            .list(kind)
            .iter()
            .find(|(_, r)| r.tool == rule.tool && r.arg == rule.arg)
        {
            return RuleId {
                kind,
                serial: *serial,
            };
        }
        self.last += 1;
        let serial = self.last;
        self.list_mut(kind).push((serial, rule));
        RuleId { kind, serial }
    }

    /// Remove a rule by id.
    pub fn remove(&mut self, id: RuleId) -> Option<Rule> {
        let list = self.list_mut(id.kind);
        let index = list.iter().position(|(serial, _)| *serial == id.serial)?;
        Some(list.remove(index).1)
    }

    /// Drop allow rules too broad for auto mode; returns the dropped rules.
    pub fn sanitize_for_auto(&mut self) -> Vec<Rule> {
        let (keep, dropped): (Vec<(u64, Rule)>, Vec<(u64, Rule)>) =
            std::mem::take(&mut self.allow)
                .into_iter()
                .partition(|(_, r)| r.broad_reason().is_none());
        self.allow = keep;
        dropped.into_iter().map(|(_, r)| r).collect()
    }

    /// Rules from one layer, as a serializable file body (without `[auto]`).
    pub fn layer_entries(&self, origin: RuleOrigin) -> (Vec<RuleEntry>, Vec<RuleEntry>) {
        let pick = |rules: &[(u64, Rule)]| {
            rules
                .iter()
                .filter(|(_, r)| r.origin == origin)
                .map(|(_, r)| r.to_entry())
                .collect()
        };
        (pick(&self.allow), pick(&self.deny))
    }
}

/// Auto-mode settings after layering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoSettings {
    /// Whether auto mode is on; when off, Tier-3 actions are denied outright.
    pub enabled: bool,
    /// Classifier model override.
    pub classifier_model: Option<String>,
    /// Whether `cargo build|test|nextest|check|clippy` are Tier 1. Off by
    /// default: builds execute project code (`build.rs`, tests, proc macros).
    pub trust_project_builds: bool,
    /// What to do when the context fills up: warn (default) or run
    /// `/compact` automatically.
    pub compact: CompactMode,
    /// Context usage (percent of the window) at which to warn once.
    pub compact_warn_percent: u8,
    /// Context usage (percent of the window) at which to auto-compact, or to
    /// warn after every turn in warn mode. Never below `compact_warn_percent`
    /// after [`load_layered`].
    pub compact_auto_percent: u8,
    /// Whether the stage-2 classifier may read project files (read-only,
    /// bounded). On by default; off makes stage 2 decide from the attached
    /// context only, so file contents never reach the classifier.
    pub classifier_tools: bool,
}

impl Default for AutoSettings {
    fn default() -> Self {
        let compaction = CompactionPolicy::default();
        Self {
            enabled: true,
            classifier_model: None,
            trust_project_builds: false,
            compact: compaction.mode,
            compact_warn_percent: compaction.warn_percent,
            compact_auto_percent: compaction.auto_percent,
            classifier_tools: true,
        }
    }
}

impl AutoSettings {
    /// The UI's context-compaction policy, with `mode` (from
    /// `--auto-compact`/`--no-auto-compact`) overriding [`Self::compact`].
    #[must_use]
    pub fn compaction_policy(&self, mode: Option<CompactMode>) -> CompactionPolicy {
        CompactionPolicy {
            mode: mode.unwrap_or(self.compact),
            warn_percent: self.compact_warn_percent,
            auto_percent: self.compact_auto_percent,
        }
        .normalized()
        .0
    }
}

/// `[auto]` section of a rules file. Unset fields inherit from lower layers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoSection {
    /// See [`AutoSettings::enabled`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// See [`AutoSettings::classifier_model`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_model: Option<String>,
    /// See [`AutoSettings::trust_project_builds`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_project_builds: Option<bool>,
    /// See [`AutoSettings::compact`] (`"warn"` or `"auto"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact: Option<CompactMode>,
    /// See [`AutoSettings::compact_warn_percent`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_warn_percent: Option<u8>,
    /// See [`AutoSettings::compact_auto_percent`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_auto_percent: Option<u8>,
    /// See [`AutoSettings::classifier_tools`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier_tools: Option<bool>,
}

/// One `[[allow]]` / `[[deny]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleEntry {
    /// Tool name or glob.
    pub tool: String,
    /// Optional subject glob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<String>,
    /// Optional exact subject (mutually exclusive with `arg`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
}

/// One `[endpoints.<name>]` table: an extra LLM endpoint (see
/// [`crate::endpoints`]). Unset fields inherit from lower layers.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointSection {
    /// `anthropic`, `openai` or `gemini` (validated when endpoints are built).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<String>,
    /// Base URL (e.g. `http://localhost:4000` for LiteLLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Environment variable holding the API key: the only way to supply
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// An inline API key. Parsed only so that start-up can refuse it with a
    /// typed error naming `api_key_env`
    /// ([`crate::endpoints::EndpointError::InlineApiKey`]); never used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Tie-break when several endpoints list the same bare model id
    /// (higher wins; default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
}

impl fmt::Debug for EndpointSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EndpointSection")
            .field("dialect", &self.dialect)
            .field("base_url", &self.base_url)
            .field("api_key_env", &self.api_key_env)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("priority", &self.priority)
            .finish()
    }
}

impl EndpointSection {
    /// Overwrite every field `other` sets.
    pub fn merge_from(&mut self, other: &Self) {
        let Self {
            dialect,
            base_url,
            api_key_env,
            api_key,
            priority,
        } = other;
        if dialect.is_some() {
            self.dialect.clone_from(dialect);
        }
        if base_url.is_some() {
            self.base_url.clone_from(base_url);
        }
        if api_key_env.is_some() {
            self.api_key_env.clone_from(api_key_env);
        }
        if api_key.is_some() {
            self.api_key.clone_from(api_key);
        }
        if priority.is_some() {
            self.priority = *priority;
        }
    }
}

/// One `[pricing."<model id>"]` table: USD per million tokens for the cost
/// readout (see [`crate::pricing`]). Unset fields inherit from lower layers,
/// then from the fetched price lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingSection {
    /// Uncached input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    /// Output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    /// Input tokens read from the prompt cache (default: `input`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<f64>,
    /// Input tokens written to the prompt cache (default: `input`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

impl PricingSection {
    /// Overwrite every field `other` sets.
    pub fn merge_from(&mut self, other: &Self) {
        let Self {
            input,
            output,
            cached_input,
            cache_write,
        } = *other;
        self.input = input.or(self.input);
        self.output = output.or(self.output);
        self.cached_input = cached_input.or(self.cached_input);
        self.cache_write = cache_write.or(self.cache_write);
    }

    /// The first field that is not a finite, non-negative number, if any.
    #[must_use]
    pub fn invalid_field(&self) -> Option<&'static str> {
        [
            ("input", self.input),
            ("output", self.output),
            ("cached_input", self.cached_input),
            ("cache_write", self.cache_write),
        ]
        .into_iter()
        .find(|(_, value)| value.is_some_and(|v| !v.is_finite() || v < 0.0))
        .map(|(name, _)| name)
    }
}

/// An endpoint after layering, with the layer that first defined it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayeredEndpoint {
    /// Merged settings.
    pub section: EndpointSection,
    /// Layer that defined the endpoint (later layers may only refine it).
    pub origin: RuleOrigin,
}

/// On-disk rules file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    /// Auto-mode settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<AutoSection>,
    /// Allow rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<RuleEntry>,
    /// Deny rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<RuleEntry>,
    /// Extra LLM endpoints by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub endpoints: BTreeMap<String, EndpointSection>,
    /// Price overrides by model id (USD per million tokens).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pricing: BTreeMap<String, PricingSection>,
    /// Extra directories for the confined shell (user layers only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confinement: Option<ConfinementSection>,
}

impl RulesFile {
    /// Load a rules file; a missing file yields an empty one.
    ///
    /// # Errors
    ///
    /// Returns [`RulesError`] on I/O or parse failures.
    pub fn load(path: &Path) -> Result<Self, RulesError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| RulesError::Parse {
                path: path.to_path_buf(),
                source,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(RulesError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// The file's rules, tagged with `origin`.
    ///
    /// # Errors
    ///
    /// [`RulesError`] when a pattern is invalid.
    pub fn rules(&self, origin: RuleOrigin) -> Result<Vec<Rule>, RulesError> {
        let allow = self
            .allow
            .iter()
            .map(|e| Rule::from_entry(RuleKind::Allow, e, origin));
        let deny = self
            .deny
            .iter()
            .map(|e| Rule::from_entry(RuleKind::Deny, e, origin));
        allow.chain(deny).collect()
    }
}

/// Non-fatal observations from loading rules, shown to the user once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The project-level file contained allow rules, which are ignored.
    ProjectAllowIgnored {
        /// Project rules file.
        path: PathBuf,
        /// Number of ignored entries.
        count: usize,
    },
    /// The project-level file tried to enable `trust_project_builds`; only the
    /// user's own layers may turn it on.
    ProjectTrustIgnored {
        /// Project rules file.
        path: PathBuf,
    },
    /// The realm rules file is not trusted (see [`crate::approval::trust`]):
    /// only its deny rules apply.
    UntrustedRealmDenyOnly {
        /// Realm rules file.
        path: PathBuf,
        /// Why it is not trusted.
        reason: Untrusted,
        /// Ignored allow rules.
        allow: usize,
        /// Whether a non-empty `[auto]` section was ignored.
        auto: bool,
        /// Ignored endpoints.
        endpoints: usize,
        /// Ignored price overrides.
        pricing: usize,
        /// Ignored `[confinement]` grants (directories).
        confinement: usize,
    },
    /// A global rules file exists at the pre-`GlobalDirs` location and is no
    /// longer read.
    LegacyGlobalRulesIgnored {
        /// The old file (`<user config root>/.rkat/tui.toml`).
        path: PathBuf,
        /// Where the global rules file now lives.
        new_path: PathBuf,
    },
    /// A configured endpoint is invalid and was skipped.
    EndpointSkipped {
        /// Endpoint name.
        name: String,
        /// Why.
        reason: String,
    },
    /// The project file defines an endpoint. Project files never define
    /// endpoints (a cloned repository could route prompts and keys to its
    /// own server): it is ignored, never used as a default or for the
    /// classifier.
    ProjectEndpointIgnored {
        /// Project rules file.
        path: PathBuf,
        /// Endpoint name.
        name: String,
    },
    /// A file the user does not own (the project file, or an untrusted
    /// realm file) set `[pricing]`; only user layers may.
    PricingIgnored {
        /// The rules file.
        path: PathBuf,
        /// Number of ignored model entries.
        count: usize,
    },
    /// The project file sets `[confinement]` grants. Only the user's own
    /// layers may widen the shell's sandbox: they are ignored.
    ProjectConfinementIgnored {
        /// Project rules file.
        path: PathBuf,
        /// Number of ignored grants (directories).
        count: usize,
    },
    /// A `[pricing."<model>"]` entry has a negative or non-finite price.
    PricingInvalid {
        /// Model id.
        model: String,
        /// The offending field.
        field: &'static str,
    },
    /// The layered `compact_warn_percent`/`compact_auto_percent` were out of
    /// range (`1..=100`, auto ≥ warn) and were clamped.
    CompactPercentClamped {
        /// The configured `(warn, auto)` percentages.
        configured: (u8, u8),
        /// The `(warn, auto)` percentages in effect.
        applied: (u8, u8),
    },
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectAllowIgnored { path, count } => write!(
                f,
                "ignored {count} allow rule(s) in {}: project rules files may only add deny rules",
                path.display()
            ),
            Self::ProjectTrustIgnored { path } => write!(
                f,
                "ignored trust_project_builds=true in {}: a project file can only lower trust",
                path.display()
            ),
            Self::UntrustedRealmDenyOnly {
                path,
                reason,
                allow,
                auto,
                endpoints,
                pricing,
                confinement,
            } => {
                write!(
                    f,
                    "{} is not trusted ({reason}); only its deny rules apply (ignored {allow} allow rule(s), {endpoints} endpoint(s), {pricing} price override(s), {confinement} [confinement] grant(s)",
                    path.display()
                )?;
                if *auto {
                    f.write_str(" and its [auto] settings")?;
                }
                f.write_str("); review it and run /rules trust to use all of it")
            }
            Self::LegacyGlobalRulesIgnored { path, new_path } => write!(
                f,
                "{} is no longer read; move it to {}",
                path.display(),
                new_path.display()
            ),
            Self::EndpointSkipped { name, reason } => {
                write!(f, "endpoint `{name}` skipped: {reason}")
            }
            Self::ProjectEndpointIgnored { path, name } => write!(
                f,
                "ignored [endpoints.{name}] in {}: project files may not define endpoints; define it in your own tui.toml",
                path.display()
            ),
            Self::PricingIgnored { path, count } => write!(
                f,
                "ignored {count} [pricing] entr{} in {}: only your own tui.toml files set prices",
                if *count == 1 { "y" } else { "ies" },
                path.display()
            ),
            Self::ProjectConfinementIgnored { path, count } => write!(
                f,
                "ignored {count} [confinement] grant(s) in {}: project files may not widen the shell's sandbox; add them to your own tui.toml",
                path.display()
            ),
            Self::PricingInvalid { model, field } => write!(
                f,
                "ignored [pricing.\"{model}\"]: `{field}` must be a non-negative number"
            ),
            Self::CompactPercentClamped {
                configured: (warn, auto),
                applied: (applied_warn, applied_auto),
            } => write!(
                f,
                "[auto] compact_warn_percent = {warn}, compact_auto_percent = {auto} are out of range (1..=100, auto ≥ warn); using {applied_warn} and {applied_auto}"
            ),
        }
    }
}

/// A [`Notice::LegacyGlobalRulesIgnored`] when the old global rules file
/// `<user_config_root>/.rkat/tui.toml` exists.
///
/// `new_path` is the current global rules file; without one the notice is not
/// emitted (there is nowhere to point the user).
#[must_use]
pub fn legacy_global_rules_notice(
    user_config_root: Option<&Path>,
    new_path: Option<&Path>,
) -> Option<Notice> {
    let path = user_config_root?.join(".rkat").join(RULES_FILE_NAME);
    let new_path = new_path?;
    (path.is_file() && path != new_path).then(|| Notice::LegacyGlobalRulesIgnored {
        path,
        new_path: new_path.to_path_buf(),
    })
}

/// How far the realm rules file (`<realm>/tui.toml`) is trusted (decided
/// from the host trust record, see [`crate::approval::trust`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealmTrust {
    /// The file is what rkat-tui wrote or the user trusted: all of it applies.
    Owned,
    /// The file is not trusted: only its deny rules apply.
    DenyOnly(Untrusted),
}

impl RealmTrust {
    /// The trust a [`TrustStatus`] gives the realm rules file.
    #[must_use]
    pub fn from_status(status: TrustStatus) -> Self {
        match status {
            TrustStatus::Missing | TrustStatus::Trusted => Self::Owned,
            TrustStatus::Untrusted(reason) => Self::DenyOnly(reason),
        }
    }
}

/// Everything `tui.toml` configures, after layering.
#[derive(Debug, Clone, Default)]
pub struct Layered {
    /// Merged allow/deny rules.
    pub rules: RuleSet,
    /// Merged `[auto]` settings.
    pub settings: AutoSettings,
    /// Merged `[endpoints.<name>]` (not yet validated).
    pub endpoints: BTreeMap<String, LayeredEndpoint>,
    /// Merged `[pricing."<model>"]` overrides (invalid entries dropped).
    pub pricing: BTreeMap<String, PricingSection>,
    /// Merged `[confinement]` grants of the user layers (global, an owned
    /// realm file, the command-line overlay), in layer order; not yet
    /// validated (see [`crate::bootstrap::confinement::grants_with`]).
    pub confinement: ConfinementSection,
    /// Non-fatal observations to show once.
    pub notices: Vec<Notice>,
}

/// Load and merge the three rule layers.
///
/// Same as [`load_layered_with`] without a command-line layer, returning
/// only rules, settings and notices.
///
/// # Errors
///
/// Returns [`RulesError`] when a file cannot be read or parsed.
pub fn load_layered(
    realm_dir: &Path,
    global_rules_file: Option<&Path>,
    project_rkat_dir: &Path,
    realm_trust: RealmTrust,
) -> Result<(RuleSet, AutoSettings, Vec<Notice>), RulesError> {
    let layered = load_layered_with(
        realm_dir,
        global_rules_file,
        project_rkat_dir,
        realm_trust,
        None,
    )?;
    Ok((layered.rules, layered.settings, layered.notices))
}

/// Load and merge the rule layers: global, realm, the command-line overlay
/// `cli` (see [`crate::overlay`]), then the project file.
///
/// `global_rules_file` is the user-global rules file itself (typically
/// [`crate::bootstrap::GlobalDirs::rules_file`]); the realm and project layers
/// are `tui.toml` inside `realm_dir` and `project_rkat_dir`.
///
/// User layers (global → realm → cli): rules accumulate, `[auto]` fields and
/// endpoint fields replace those of earlier layers, endpoints merge by name.
/// `[confinement]` grants accumulate.
/// A [`RealmTrust::DenyOnly`] realm file contributes only deny rules (no allow
/// rules, `[auto]` settings, endpoints, prices or `[confinement]` grants).
/// The project layer contributes deny rules and `[auto]` settings the
/// command line did not set; its endpoints and `[confinement]` grants are
/// ignored with a notice; it can never turn
/// `trust_project_builds` or `classifier_tools` on, but may turn them off.
///
/// # Errors
///
/// Returns [`RulesError`] when a file cannot be read or parsed.
pub fn load_layered_with(
    realm_dir: &Path,
    global_rules_file: Option<&Path>,
    project_rkat_dir: &Path,
    realm_trust: RealmTrust,
    cli: Option<&RulesFile>,
) -> Result<Layered, RulesError> {
    let mut rules = RuleSet::default();
    let mut settings = AutoSettings::default();
    let mut notices = Vec::new();
    let mut endpoints: BTreeMap<String, LayeredEndpoint> = BTreeMap::new();
    let mut pricing: BTreeMap<String, PricingSection> = BTreeMap::new();
    let mut confinement = ConfinementSection::default();

    let mut user_layers: Vec<(Option<PathBuf>, RuleOrigin, RulesFile)> = Vec::new();
    if let Some(global) = global_rules_file {
        user_layers.push((
            Some(global.to_path_buf()),
            RuleOrigin::Global,
            RulesFile::load(global)?,
        ));
    }
    let realm_path = realm_dir.join(RULES_FILE_NAME);
    let realm_file = RulesFile::load(&realm_path)?;
    user_layers.push((Some(realm_path), RuleOrigin::Realm, realm_file));
    if let Some(cli) = cli {
        user_layers.push((None, RuleOrigin::Cli, cli.clone()));
    }

    for (path, origin, file) in user_layers {
        let untrusted = match (&realm_trust, origin) {
            (RealmTrust::DenyOnly(reason), RuleOrigin::Realm) => Some(reason.clone()),
            _ => None,
        };
        let path = path.unwrap_or_default();
        let mut ignored_allow = 0;
        for rule in file.rules(origin)? {
            if untrusted.is_some() && rule.kind == RuleKind::Allow {
                ignored_allow += 1;
            } else {
                rules.push(rule);
            }
        }
        if let Some(reason) = untrusted {
            let auto = file
                .auto
                .as_ref()
                .is_some_and(|auto| *auto != AutoSection::default());
            let grants = file.confinement.as_ref().map_or(0, ConfinementSection::len);
            if ignored_allow > 0
                || auto
                || !file.endpoints.is_empty()
                || !file.pricing.is_empty()
                || grants > 0
            {
                notices.push(Notice::UntrustedRealmDenyOnly {
                    path,
                    reason,
                    allow: ignored_allow,
                    auto,
                    endpoints: file.endpoints.len(),
                    pricing: file.pricing.len(),
                    confinement: grants,
                });
            }
            continue;
        }
        if let Some(section) = &file.confinement {
            confinement.extend_from(section);
        }
        for (model, section) in &file.pricing {
            pricing
                .entry(model.clone())
                .or_default()
                .merge_from(section);
        }
        for (name, section) in &file.endpoints {
            endpoints
                .entry(name.clone())
                .and_modify(|layered| layered.section.merge_from(section))
                .or_insert_with(|| LayeredEndpoint {
                    section: section.clone(),
                    origin,
                });
        }
        if let Some(auto) = &file.auto {
            apply_auto(&mut settings, auto);
        }
    }

    let project_path = project_rkat_dir.join(RULES_FILE_NAME);
    let project = RulesFile::load(&project_path)?;
    if !project.allow.is_empty() {
        notices.push(Notice::ProjectAllowIgnored {
            path: project_path.clone(),
            count: project.allow.len(),
        });
    }
    for entry in &project.deny {
        rules.push(Rule::from_entry(
            RuleKind::Deny,
            entry,
            RuleOrigin::Project,
        )?);
    }
    if !project.pricing.is_empty() {
        notices.push(Notice::PricingIgnored {
            path: project_path.clone(),
            count: project.pricing.len(),
        });
    }
    pricing.retain(|model, section| match section.invalid_field() {
        Some(field) => {
            notices.push(Notice::PricingInvalid {
                model: model.clone(),
                field,
            });
            false
        }
        None => true,
    });
    // Nor do they widen the shell's sandbox.
    let project_grants = project
        .confinement
        .as_ref()
        .map_or(0, ConfinementSection::len);
    if project_grants > 0 {
        notices.push(Notice::ProjectConfinementIgnored {
            path: project_path.clone(),
            count: project_grants,
        });
    }
    // Project files never define endpoints, not even new ones: a cloned
    // repository could route prompts and credentials to its own server.
    for name in project.endpoints.keys() {
        notices.push(Notice::ProjectEndpointIgnored {
            path: project_path.clone(),
            name: name.clone(),
        });
    }
    if let Some(mut auto) = project.auto {
        if auto.trust_project_builds == Some(true) {
            auto.trust_project_builds = None;
            notices.push(Notice::ProjectTrustIgnored { path: project_path });
        }
        // File contents a cloned repository wrote must not reach the
        // classifier against the user's choice: the project may only turn
        // the classifier's file reads off.
        if auto.classifier_tools == Some(true) {
            auto.classifier_tools = None;
        }
        // The command line wins over the project file, except that the
        // project may always lower `trust_project_builds` and turn off
        // `classifier_tools`.
        if let Some(cli_auto) = cli.and_then(|c| c.auto.as_ref()) {
            let lower_trust = auto.trust_project_builds;
            let tools_off = auto.classifier_tools;
            auto = project_auto_without(auto, cli_auto);
            if lower_trust == Some(false) {
                auto.trust_project_builds = Some(false);
            }
            if tools_off == Some(false) {
                auto.classifier_tools = Some(false);
            }
        }
        apply_auto(&mut settings, &auto);
    }
    let (policy, clamped) = CompactionPolicy {
        mode: settings.compact,
        warn_percent: settings.compact_warn_percent,
        auto_percent: settings.compact_auto_percent,
    }
    .normalized();
    if clamped {
        notices.push(Notice::CompactPercentClamped {
            configured: (settings.compact_warn_percent, settings.compact_auto_percent),
            applied: (policy.warn_percent, policy.auto_percent),
        });
        settings.compact_warn_percent = policy.warn_percent;
        settings.compact_auto_percent = policy.auto_percent;
    }
    Ok(Layered {
        rules,
        settings,
        endpoints,
        pricing,
        confinement,
        notices,
    })
}

/// `project` with every field `cli` sets cleared.
fn project_auto_without(project: AutoSection, cli: &AutoSection) -> AutoSection {
    fn keep<T>(value: Option<T>, overridden: bool) -> Option<T> {
        if overridden { None } else { value }
    }
    AutoSection {
        enabled: keep(project.enabled, cli.enabled.is_some()),
        classifier_model: keep(project.classifier_model, cli.classifier_model.is_some()),
        trust_project_builds: keep(
            project.trust_project_builds,
            cli.trust_project_builds.is_some(),
        ),
        compact: keep(project.compact, cli.compact.is_some()),
        compact_warn_percent: keep(
            project.compact_warn_percent,
            cli.compact_warn_percent.is_some(),
        ),
        compact_auto_percent: keep(
            project.compact_auto_percent,
            cli.compact_auto_percent.is_some(),
        ),
        classifier_tools: keep(project.classifier_tools, cli.classifier_tools.is_some()),
    }
}

fn apply_auto(settings: &mut AutoSettings, auto: &AutoSection) {
    if let Some(enabled) = auto.enabled {
        settings.enabled = enabled;
    }
    if let Some(model) = &auto.classifier_model {
        settings.classifier_model = Some(model.clone());
    }
    if let Some(trust) = auto.trust_project_builds {
        settings.trust_project_builds = trust;
    }
    if let Some(mode) = auto.compact {
        settings.compact = mode;
    }
    if let Some(percent) = auto.compact_warn_percent {
        settings.compact_warn_percent = percent;
    }
    if let Some(percent) = auto.compact_auto_percent {
        settings.compact_auto_percent = percent;
    }
    if let Some(tools) = auto.classifier_tools {
        settings.classifier_tools = tools;
    }
}

/// Atomically write a rules file (temp file in the same directory + rename).
///
/// # Errors
///
/// Returns [`RulesError`] on serialization or I/O failures.
pub fn save_realm(path: &Path, file: &RulesFile) -> Result<(), RulesError> {
    let text = toml::to_string_pretty(file)?;
    // Realm files can hold endpoint keys: owner-only, never through a symlink.
    let trusted = path.parent().and_then(Path::parent);
    crate::paths::write_atomic(
        path,
        text.as_bytes(),
        crate::paths::FileMode::Private,
        trusted,
    )
    .map_err(|source| RulesError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// A human-readable summary of a rules file (what `/rules trust` shows
/// before trusting it).
#[must_use]
pub fn summarize(path: &Path, file: &RulesFile) -> String {
    use std::fmt::Write as _;
    let mut out = format!("{}:", path.display());
    let describe = |entry: &RuleEntry| match (&entry.arg, &entry.literal) {
        (Some(arg), _) => format!("{} {arg}", entry.tool),
        (None, Some(literal)) => format!("{} ={literal:?}", entry.tool),
        (None, None) => entry.tool.clone(),
    };
    for entry in &file.allow {
        let _ = write!(out, "\n  allow {}", describe(entry));
    }
    for entry in &file.deny {
        let _ = write!(out, "\n  deny {}", describe(entry));
    }
    if file.auto.as_ref().is_some_and(|a| *a != AutoSection::default()) {
        out.push_str("\n  [auto] settings");
    }
    for name in file.endpoints.keys() {
        let _ = write!(out, "\n  endpoint {name}");
    }
    if !file.pricing.is_empty() {
        let _ = write!(out, "\n  {} price override(s)", file.pricing.len());
    }
    out
}

/// Rewrite the rule lists of the realm file from the realm/session rules in
/// `rules`, preserving its `[auto]` section.
///
/// # Errors
///
/// Returns [`RulesError`] on I/O, parse or serialization failures.
pub fn persist_realm_rules(path: &Path, rules: &RuleSet) -> Result<(), RulesError> {
    let mut file = RulesFile::load(path)?;
    let (mut allow, mut deny) = rules.layer_entries(RuleOrigin::Realm);
    let (session_allow, session_deny) = rules.layer_entries(RuleOrigin::Session);
    allow.extend(session_allow);
    deny.extend(session_deny);
    file.allow = allow;
    file.deny = deny;
    save_realm(path, &file)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn allow(tool: &str, arg: Option<&str>) -> Rule {
        Rule::new(RuleKind::Allow, tool, arg, RuleOrigin::Realm).unwrap()
    }

    #[test]
    fn tool_patterns() {
        let exact = ToolPattern::new("shell").unwrap();
        assert!(exact.is_exact());
        assert!(exact.matches("shell"));
        assert!(!exact.matches("shell_jobs"));
        let glob = ToolPattern::new("mcp__github__*").unwrap();
        assert!(glob.matches("mcp__github__create_issue"));
        assert!(!glob.matches("mcp__gitlab__x"));
        assert!(ToolPattern::new("  ").is_err());
    }

    #[test]
    fn arg_patterns_span_separators() {
        let p = ArgPattern::new("cargo nextest run*").unwrap();
        assert!(p.matches("cargo nextest run -p meerkat-tui --lib"));
        assert!(!p.matches("cargo run"));
        let path = ArgPattern::new("docs/**").unwrap();
        assert!(path.matches("docs/a/b.md"));
    }

    #[test]
    fn allow_requires_all_subjects_deny_any() {
        let a = allow("apply_patch", Some("src/*"));
        let subjects = vec!["src/a.rs".to_string(), "README.md".to_string()];
        assert!(!a.matches("apply_patch", &subjects));
        let d = Rule::new(
            RuleKind::Deny,
            "apply_patch",
            Some("src/*"),
            RuleOrigin::Realm,
        )
        .unwrap();
        assert!(d.matches("apply_patch", &subjects));
    }

    #[test]
    fn ruleset_ids_and_matching() {
        let mut set = RuleSet::default();
        set.push(
            Rule::new(
                RuleKind::Deny,
                "shell",
                Some("git push*"),
                RuleOrigin::Global,
            )
            .unwrap(),
        );
        set.push(allow("shell", Some("make lint")));
        set.push(allow("shell", Some("make lint")));
        assert_eq!(set.allow.len(), 1, "duplicates are ignored");
        let id = set
            .matches_deny("shell", &["git push --force".into()])
            .unwrap();
        assert_eq!(id.to_string(), "deny#1");
        assert!(set.get(id).is_some());
        assert!(set.matches_deny("shell", &["git status".into()]).is_none());
        assert!(set.remove(id).is_some());
        assert!(set.deny.is_empty());
    }

    #[test]
    fn sanitizer_drops_broad_rules() {
        let mut set = RuleSet::default();
        let broad = [
            allow("shell", None),
            allow("shell", Some("*")),
            allow("shell", Some("python*")),
            allow("shell", Some("python3 *")),
            allow("shell", Some("node *")),
            allow("shell", Some("bash *")),
            allow("shell", Some("sh -c *")),
            allow("shell", Some("ruby *")),
            allow("shell", Some("perl *")),
            allow("shell", Some("deno *")),
            allow("shell", Some("bun *")),
            allow("shell", Some("npm run *")),
            allow("shell", Some("npx prettier")),
            allow("shell", Some("pnpm test")),
            allow("shell", Some("yarn *")),
            allow("shell", Some("cargo run")),
            allow("shell", Some("make")),
            allow("shell", Some("make *")),
            allow("mcp__*", None),
            allow("mcp__github__*", None),
            allow("*", None),
            allow("sh*", None),
        ];
        let narrow = [
            allow("shell", Some("cargo nextest run*")),
            allow("shell", Some("python3 scripts/gen.py")),
            allow("shell", Some("npm test")),
            allow("mcp__github__list_issues", None),
            allow("read_file", Some("/opt/docs/**")),
        ];
        for r in broad.iter().chain(narrow.iter()) {
            set.push(r.clone());
        }
        set.push(Rule::new(RuleKind::Deny, "shell", None, RuleOrigin::Realm).unwrap());
        let dropped = set.sanitize_for_auto();
        assert_eq!(dropped.len(), broad.len(), "dropped: {dropped:#?}");
        assert_eq!(set.allow.len(), narrow.len());
        assert_eq!(set.deny.len(), 1, "deny rules are never sanitized");
    }

    #[test]
    fn rules_file_round_trip() {
        let file = RulesFile {
            auto: Some(AutoSection {
                enabled: Some(true),
                classifier_model: Some("claude-sonnet-4-6".into()),
                trust_project_builds: Some(false),
                ..AutoSection::default()
            }),
            allow: vec![
                RuleEntry {
                    tool: "shell".into(),
                    arg: Some("cargo nextest run*".into()),
                    literal: None,
                },
                RuleEntry {
                    tool: "shell".into(),
                    arg: None,
                    literal: Some("make lint [*]".into()),
                },
            ],
            deny: vec![RuleEntry {
                tool: "mcp__prod__*".into(),
                arg: None,
                literal: None,
            }],
            endpoints: BTreeMap::from([(
                "litellm".to_string(),
                EndpointSection {
                    dialect: Some("openai".into()),
                    base_url: Some("http://localhost:4000".into()),
                    api_key_env: Some("LITELLM_KEY".into()),
                    api_key: None,
                    priority: Some(2),
                },
            )]),
            pricing: BTreeMap::from([(
                "gpt-5.4".to_string(),
                PricingSection {
                    input: Some(1.25),
                    output: Some(10.0),
                    cached_input: None,
                    cache_write: None,
                },
            )]),
            confinement: Some(ConfinementSection {
                extra_read: vec![PathBuf::from("/data")],
                extra_write: vec![PathBuf::from("/scratch")],
            }),
        };
        let text = toml::to_string_pretty(&file).unwrap();
        let back: RulesFile = toml::from_str(&text).unwrap();
        assert_eq!(back, file);
        let parsed: RulesFile = toml::from_str(
            "[auto]\nenabled = true\n\n[[allow]]\ntool = \"shell\"\narg = \"cargo nextest run*\"\n",
        )
        .unwrap();
        assert_eq!(parsed.allow.len(), 1);
        assert!(toml::from_str::<RulesFile>("[[allow]]\ntool=\"x\"\nbogus=1\n").is_err());
        let rules = file.rules(RuleOrigin::Realm).unwrap();
        assert_eq!(
            rules[1].arg,
            Some(ArgPattern::Literal("make lint [*]".into()))
        );
        assert_eq!(rules[1].to_entry(), file.allow[1]);
        let both: RulesFile =
            toml::from_str("[[allow]]\ntool=\"x\"\narg=\"a\"\nliteral=\"b\"\n").unwrap();
        assert!(matches!(
            both.rules(RuleOrigin::Realm),
            Err(RulesError::ArgAndLiteral { .. })
        ));
    }

    #[test]
    fn layered_loading() {
        let tmp = tempfile::tempdir().unwrap();
        let realm = tmp.path().join("realm");
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&realm).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            home.join(RULES_FILE_NAME),
            "[auto]\ntrust_project_builds = false\n[[deny]]\ntool = \"shell\"\narg = \"git push*\"\n",
        )
        .unwrap();
        std::fs::write(
            realm.join(RULES_FILE_NAME),
            "[auto]\nclassifier_model = \"gpt-5.2\"\n[[allow]]\ntool = \"shell\"\narg = \"make lint\"\n",
        )
        .unwrap();
        std::fs::write(
            project.join(RULES_FILE_NAME),
            "[auto]\ntrust_project_builds = true\nenabled = false\n[[allow]]\ntool = \"shell\"\n[[deny]]\ntool = \"agent_spawn\"\n",
        )
        .unwrap();
        let (rules, settings, notices) = load_layered(
            &realm,
            Some(&home.join(RULES_FILE_NAME)),
            &project,
            RealmTrust::Owned,
        )
        .unwrap();
        assert_eq!(rules.allow.len(), 1);
        assert_eq!(rules.allow[0].1.origin, RuleOrigin::Realm);
        assert_eq!(rules.deny.len(), 2);
        assert!(!settings.trust_project_builds);
        assert!(!settings.enabled);
        assert_eq!(settings.classifier_model.as_deref(), Some("gpt-5.2"));
        assert_eq!(notices.len(), 2);
        assert!(
            notices
                .iter()
                .any(|n| matches!(n, Notice::ProjectAllowIgnored { count: 1, .. }))
        );
    }

    #[test]
    fn global_rules_come_from_the_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("cfg").join("custom.toml");
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        std::fs::write(
            &global,
            "[[allow]]\ntool = \"shell\"\narg = \"make test\"\n[[deny]]\ntool = \"shell\"\narg = \"rm *\"\n",
        )
        .unwrap();
        let (rules, _, notices) = load_layered(
            &tmp.path().join("realm"),
            Some(&global),
            tmp.path(),
            RealmTrust::Owned,
        )
        .unwrap();
        assert_eq!(rules.allow.len(), 1);
        assert_eq!(rules.allow[0].1.origin, RuleOrigin::Global);
        assert_eq!(rules.deny.len(), 1);
        assert_eq!(rules.deny[0].1.origin, RuleOrigin::Global);
        assert!(notices.is_empty());
        // A missing global file is not an error.
        let (rules, _, _) = load_layered(
            &tmp.path().join("realm"),
            Some(&tmp.path().join("missing.toml")),
            tmp.path(),
            RealmTrust::Owned,
        )
        .unwrap();
        assert_eq!(rules, RuleSet::default());
    }

    #[test]
    fn legacy_global_rules_notice_only_when_old_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let new_path = tmp.path().join(".config/rkat-tui/tui.toml");
        assert_eq!(
            legacy_global_rules_notice(Some(tmp.path()), Some(&new_path)),
            None
        );
        let old = tmp.path().join(".rkat").join(RULES_FILE_NAME);
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "").unwrap();
        let notice = legacy_global_rules_notice(Some(tmp.path()), Some(&new_path)).unwrap();
        assert_eq!(
            notice,
            Notice::LegacyGlobalRulesIgnored {
                path: old.clone(),
                new_path: new_path.clone()
            }
        );
        assert_eq!(
            notice.to_string(),
            format!(
                "{} is no longer read; move it to {}",
                old.display(),
                new_path.display()
            )
        );
        assert_eq!(legacy_global_rules_notice(None, Some(&new_path)), None);
        assert_eq!(legacy_global_rules_notice(Some(tmp.path()), None), None);
    }

    #[test]
    fn missing_files_give_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let (rules, settings, notices) = load_layered(
            tmp.path(),
            None,
            &tmp.path().join("nope"),
            RealmTrust::Owned,
        )
        .unwrap();
        assert_eq!(rules, RuleSet::default());
        assert_eq!(settings, AutoSettings::default());
        assert!(notices.is_empty());
    }

    #[test]
    fn invalid_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(RULES_FILE_NAME),
            "[[allow]]\ntool = \"[\"\n",
        )
        .unwrap();
        assert!(matches!(
            load_layered(tmp.path(), None, tmp.path(), RealmTrust::Owned),
            Err(RulesError::Pattern { .. })
        ));
        std::fs::write(tmp.path().join(RULES_FILE_NAME), "not toml [").unwrap();
        assert!(matches!(
            load_layered(tmp.path(), None, tmp.path(), RealmTrust::Owned),
            Err(RulesError::Parse { .. })
        ));
    }

    #[test]
    fn save_and_persist_preserve_auto_section() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("deep/realm").join(RULES_FILE_NAME);
        save_realm(
            &path,
            &RulesFile {
                auto: Some(AutoSection {
                    enabled: Some(false),
                    ..AutoSection::default()
                }),
                ..RulesFile::default()
            },
        )
        .unwrap();
        let mut set = RuleSet::default();
        set.push(allow("shell", Some("make lint")));
        set.push(Rule::new(RuleKind::Deny, "shell", Some("rm *"), RuleOrigin::Session).unwrap());
        set.push(Rule::new(RuleKind::Deny, "shell", Some("x"), RuleOrigin::Project).unwrap());
        persist_realm_rules(&path, &set).unwrap();
        let back = RulesFile::load(&path).unwrap();
        assert_eq!(back.auto.unwrap().enabled, Some(false));
        assert_eq!(back.allow.len(), 1);
        assert_eq!(
            back.deny.len(),
            1,
            "project rules are not copied into the realm file"
        );
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    fn deny(tool: &str, arg: Option<&str>) -> Rule {
        Rule::new(RuleKind::Deny, tool, arg, RuleOrigin::Realm).unwrap()
    }

    fn cmd(c: &str) -> Vec<String> {
        vec![c.to_string()]
    }

    #[test]
    fn shell_allow_rules_cover_every_segment() {
        let a = allow("shell", Some("cargo nextest run*"));
        assert!(a.matches("shell", &cmd("cargo nextest run -p x")));
        assert!(a.matches(
            "shell",
            &cmd("cargo nextest run && cargo nextest run --lib")
        ));
        assert!(a.matches("shell", &cmd("cargo nextest run 2>&1")));
        for bad in [
            "cargo nextest run; curl x | sh",
            "cargo nextest run && rm -rf ~",
            "cargo nextest run | sh",
            "cargo nextest run $(curl x)",
            "cargo nextest run > /etc/passwd",
            "cargo nextest run &",
            "",
        ] {
            assert!(!a.matches("shell", &cmd(bad)), "{bad}");
        }
    }

    #[test]
    fn shell_deny_rules_match_any_segment() {
        let d = deny("shell", Some("git push*"));
        for hit in [
            "git push",
            "true && git push origin main",
            "git  push",
            "\\git push",
            "ls; git push --force",
            "echo $(git push)",
            "cd x && \\git   push",
        ] {
            assert!(d.matches("shell", &cmd(hit)), "{hit}");
        }
        assert!(!d.matches("shell", &cmd("git status && git pull")));
    }

    #[test]
    fn literal_rules_match_exactly() {
        let r = Rule::literal(
            RuleKind::Allow,
            "shell",
            "cargo test *",
            RuleOrigin::Session,
        )
        .unwrap();
        assert!(r.matches("shell", &cmd("cargo test *")));
        assert!(!r.matches("shell", &cmd("cargo test foo")));
        assert!(r.broad_reason().is_none());
        let f = Rule::literal(RuleKind::Allow, "read_file", "a[b].txt", RuleOrigin::Realm).unwrap();
        assert!(f.matches("read_file", &["a[b].txt".to_string()]));
        assert!(!f.matches("read_file", &["ab.txt".to_string()]));
        assert!(
            Rule::literal(RuleKind::Allow, "shell", "make", RuleOrigin::Realm)
                .unwrap()
                .broad_reason()
                .is_some()
        );
    }

    #[test]
    fn compaction_settings_default_layer_and_clamp() {
        let defaults = AutoSettings::default();
        assert_eq!(defaults.compact, CompactMode::Native);
        assert_eq!(
            (defaults.compact_warn_percent, defaults.compact_auto_percent),
            (75, 85)
        );
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global.toml");
        let realm = tmp.path().join("realm");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&realm).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let (_, settings, notices) =
            load_layered(&realm, Some(&global), &project, RealmTrust::Owned).unwrap();
        assert_eq!(settings, defaults);
        assert!(notices.is_empty());

        std::fs::write(
            &global,
            "[auto]\ncompact = \"auto\"\ncompact_warn_percent = 60\ncompact_auto_percent = 70\n",
        )
        .unwrap();
        std::fs::write(
            realm.join(RULES_FILE_NAME),
            "[auto]\ncompact_auto_percent = 90\n",
        )
        .unwrap();
        std::fs::write(
            project.join(RULES_FILE_NAME),
            "[auto]\ncompact = \"warn\"\n",
        )
        .unwrap();
        let (_, settings, notices) =
            load_layered(&realm, Some(&global), &project, RealmTrust::Owned).unwrap();
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(
            settings.compact,
            CompactMode::Warn,
            "the project layer may set it"
        );
        assert_eq!(
            (settings.compact_warn_percent, settings.compact_auto_percent),
            (60, 90)
        );
        let policy = settings.compaction_policy(None);
        assert_eq!(policy.mode, CompactMode::Warn);
        assert_eq!((policy.warn_percent, policy.auto_percent), (60, 90));
        assert_eq!(
            settings.compaction_policy(Some(CompactMode::Auto)).mode,
            CompactMode::Auto,
            "--auto-compact overrides the file"
        );

        // auto below warn is clamped up, with a notice.
        std::fs::write(
            project.join(RULES_FILE_NAME),
            "[auto]\ncompact_warn_percent = 95\ncompact_auto_percent = 80\n",
        )
        .unwrap();
        let (_, settings, notices) =
            load_layered(&realm, Some(&global), &project, RealmTrust::Owned).unwrap();
        assert_eq!(
            (settings.compact_warn_percent, settings.compact_auto_percent),
            (95, 95)
        );
        assert_eq!(
            notices,
            vec![Notice::CompactPercentClamped {
                configured: (95, 80),
                applied: (95, 95),
            }]
        );

        std::fs::write(
            project.join(RULES_FILE_NAME),
            "[auto]\ncompact = \"always\"\n",
        )
        .unwrap();
        assert!(load_layered(&realm, Some(&global), &project, RealmTrust::Owned).is_err());
        std::fs::write(
            project.join(RULES_FILE_NAME),
            "[auto]\ncompact_warn_percent = 300\n",
        )
        .unwrap();
        assert!(
            load_layered(&realm, Some(&global), &project, RealmTrust::Owned).is_err(),
            "u8 overflow is a parse error"
        );
    }

    #[test]
    fn trust_defaults_off_and_project_can_only_lower_it() {
        assert!(!AutoSettings::default().trust_project_builds);
        let tmp = tempfile::tempdir().unwrap();
        let realm = tmp.path().join("realm");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&realm).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join(RULES_FILE_NAME),
            "[auto]\ntrust_project_builds = true\n",
        )
        .unwrap();
        let (_, settings, notices) =
            load_layered(&realm, None, &project, RealmTrust::Owned).unwrap();
        assert!(!settings.trust_project_builds);
        assert!(matches!(notices[..], [Notice::ProjectTrustIgnored { .. }]));

        std::fs::write(
            realm.join(RULES_FILE_NAME),
            "[auto]\ntrust_project_builds = true\n",
        )
        .unwrap();
        std::fs::write(
            project.join(RULES_FILE_NAME),
            "[auto]\ntrust_project_builds = false\n",
        )
        .unwrap();
        let (_, settings, _) = load_layered(&realm, None, &project, RealmTrust::Owned).unwrap();
        assert!(!settings.trust_project_builds, "project lowers trust");
        std::fs::remove_file(project.join(RULES_FILE_NAME)).unwrap();
        let (_, settings, _) = load_layered(&realm, None, &project, RealmTrust::Owned).unwrap();
        assert!(
            settings.trust_project_builds,
            "the user's realm may raise it"
        );
    }

    #[test]
    fn untrusted_realm_contributes_only_deny_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let realm = tmp.path().join("realm");
        std::fs::create_dir_all(&realm).unwrap();
        std::fs::write(
            realm.join(RULES_FILE_NAME),
            "[auto]\ntrust_project_builds = true\nenabled = true\n[[allow]]\ntool = \"shell\"\narg = \"make lint\"\n[[deny]]\ntool = \"shell\"\narg = \"rm *\"\n",
        )
        .unwrap();
        let (rules, settings, notices) = load_layered(
            &realm,
            None,
            &tmp.path().join("p"),
            RealmTrust::DenyOnly(Untrusted::NotRecorded),
        )
        .unwrap();
        assert!(rules.allow.is_empty());
        assert_eq!(rules.deny.len(), 1);
        assert!(!settings.trust_project_builds);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices.iter().any(|n| matches!(
            n,
            Notice::UntrustedRealmDenyOnly {
                allow: 1,
                auto: true,
                ..
            }
        )));
        assert!(notices[0].to_string().contains("/rules trust"));
    }

    fn write(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(RULES_FILE_NAME), text).unwrap();
    }

    fn cli(text: &str) -> RulesFile {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn classifier_tools_default_on_and_project_can_only_turn_them_off() {
        let tmp = tempfile::tempdir().unwrap();
        let (realm, project) = (tmp.path().join("realm"), tmp.path().join("p"));
        assert!(AutoSettings::default().classifier_tools);
        // The project may turn the classifier's file reads off, even over
        // the command line...
        write(&realm, "");
        write(&project, "[auto]\nclassifier_tools = false\n");
        let overlay = cli("[auto]\nclassifier_tools = true\n");
        let layered =
            load_layered_with(&realm, None, &project, RealmTrust::Owned, Some(&overlay)).unwrap();
        assert!(!layered.settings.classifier_tools);
        // ...but never back on once a user layer turned them off.
        write(&realm, "[auto]\nclassifier_tools = false\n");
        write(&project, "[auto]\nclassifier_tools = true\n");
        let layered = load_layered_with(&realm, None, &project, RealmTrust::Owned, None).unwrap();
        assert!(!layered.settings.classifier_tools);
        // The command line may turn them off.
        write(&realm, "");
        let overlay = cli("[auto]\nclassifier_tools = false\n");
        let layered =
            load_layered_with(&realm, None, &project, RealmTrust::Owned, Some(&overlay)).unwrap();
        assert!(!layered.settings.classifier_tools);
    }

    #[test]
    fn cli_layer_beats_realm_but_project_deny_and_lowering_still_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let (realm, project) = (tmp.path().join("realm"), tmp.path().join("p"));
        write(
            &realm,
            "[auto]\nenabled = true\ncompact = \"warn\"\nclassifier_model = \"realm-model\"\n[[allow]]\ntool = \"read_file\"\n",
        );
        write(
            &project,
            "[auto]\ncompact = \"warn\"\ncompact_warn_percent = 50\ntrust_project_builds = false\n[[deny]]\ntool = \"shell\"\narg = \"git push*\"\n",
        );
        let overlay = cli(
            "[auto]\nenabled = false\ncompact = \"auto\"\ntrust_project_builds = true\n[[allow]]\ntool = \"shell\"\narg = \"cargo nextest run*\"\n[[deny]]\ntool = \"shell\"\narg = \"git push*\"\n",
        );
        let layered =
            load_layered_with(&realm, None, &project, RealmTrust::Owned, Some(&overlay)).unwrap();
        let s = &layered.settings;
        assert!(!s.enabled, "cli beats realm");
        assert_eq!(s.compact, CompactMode::Auto, "cli beats project");
        assert_eq!(s.classifier_model.as_deref(), Some("realm-model"));
        assert_eq!(
            s.compact_warn_percent, 50,
            "project fills what cli left unset"
        );
        assert!(!s.trust_project_builds, "project may lower trust");
        let allow: Vec<(RuleOrigin, &str)> = layered
            .rules
            .allow()
            .map(|(_, r)| (r.origin, r.tool.as_str()))
            .collect();
        assert_eq!(
            allow,
            vec![(RuleOrigin::Realm, "read_file"), (RuleOrigin::Cli, "shell")]
        );
        // The project's deny duplicates the cli's: one rule, the cli's.
        assert_eq!(layered.rules.deny.len(), 1);
        assert!(
            layered
                .rules
                .matches_deny("shell", &["git push origin".to_string()])
                .is_some()
        );
        // Without the cli deny, the project's still applies.
        let layered = load_layered_with(
            &realm,
            None,
            &project,
            RealmTrust::Owned,
            Some(&cli("[[allow]]\ntool = \"shell\"\narg = \"git push*\"\n")),
        )
        .unwrap();
        assert_eq!(layered.rules.deny[0].1.origin, RuleOrigin::Project);
        assert!(
            layered
                .rules
                .matches_deny("shell", &["git push".to_string()])
                .is_some()
        );
    }

    #[test]
    fn pricing_layers_merge_by_field_and_only_user_layers_count() {
        let tmp = tempfile::tempdir().unwrap();
        let (realm, project) = (tmp.path().join("realm"), tmp.path().join("p"));
        let global = tmp.path().join("global").join(RULES_FILE_NAME);
        write(
            global.parent().unwrap(),
            "[pricing.\"claude-opus-4-6\"]\ninput = 5\noutput = 25\ncached_input = 0.5\n",
        );
        write(
            &realm,
            "[pricing.\"claude-opus-4-6\"]\noutput = 30\n[pricing.bad]\ninput = -1\noutput = 1\n",
        );
        write(&project, "[pricing.\"gpt-5.4\"]\ninput = 0\noutput = 0\n");
        let overlay = cli("[pricing.\"claude-opus-4-6\"]\ninput = 15\n");
        let layered = load_layered_with(
            &realm,
            Some(&global),
            &project,
            RealmTrust::Owned,
            Some(&overlay),
        )
        .unwrap();
        assert_eq!(
            layered.pricing.get("claude-opus-4-6"),
            Some(&PricingSection {
                input: Some(15.0),
                output: Some(30.0),
                cached_input: Some(0.5),
                cache_write: None,
            })
        );
        assert!(
            !layered.pricing.contains_key("gpt-5.4"),
            "project prices ignored"
        );
        assert!(
            !layered.pricing.contains_key("bad"),
            "negative price dropped"
        );
        let texts: Vec<String> = layered.notices.iter().map(ToString::to_string).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("ignored 1 [pricing] entry")),
            "{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("`input` must be")),
            "{texts:?}"
        );
        // An untrusted realm file's prices are ignored too.
        let layered = load_layered_with(
            &realm,
            None,
            &project,
            RealmTrust::DenyOnly(Untrusted::Changed),
            None,
        )
        .unwrap();
        assert!(layered.pricing.is_empty());
    }

    #[test]
    fn cli_rules_are_never_persisted() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(RULES_FILE_NAME);
        let mut set = RuleSet::default();
        set.push(Rule::new(RuleKind::Allow, "shell", Some("ls*"), RuleOrigin::Cli).unwrap());
        set.push(Rule::new(RuleKind::Allow, "read_file", None, RuleOrigin::Realm).unwrap());
        persist_realm_rules(&path, &set).unwrap();
        let file = RulesFile::load(&path).unwrap();
        assert_eq!(file.allow.len(), 1);
        assert_eq!(file.allow[0].tool, "read_file");
    }

    #[test]
    fn endpoints_layer_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let (realm, project) = (tmp.path().join("realm"), tmp.path().join("p"));
        let global = tmp.path().join("global").join(RULES_FILE_NAME);
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        std::fs::write(
            &global,
            "[endpoints.litellm]\ndialect = \"openai\"\nbase_url = \"http://global\"\napi_key_env = \"LITELLM_KEY\"\n",
        )
        .unwrap();
        write(
            &realm,
            "[endpoints.litellm]\nbase_url = \"http://realm\"\npriority = 2\n",
        );
        write(
            &project,
            "[endpoints.litellm]\nbase_url = \"http://evil\"\n[endpoints.team]\ndialect = \"anthropic\"\napi_key = \"k\"\n",
        );
        let overlay = cli("[endpoints.litellm]\nbase_url = \"http://cli\"\n");
        let layered = load_layered_with(
            &realm,
            Some(&global),
            &project,
            RealmTrust::Owned,
            Some(&overlay),
        )
        .unwrap();
        let litellm = &layered.endpoints["litellm"];
        assert_eq!(litellm.origin, RuleOrigin::Global);
        assert_eq!(litellm.section.dialect.as_deref(), Some("openai"));
        assert_eq!(litellm.section.base_url.as_deref(), Some("http://cli"));
        assert_eq!(litellm.section.api_key_env.as_deref(), Some("LITELLM_KEY"));
        assert_eq!(litellm.section.priority, Some(2));
        // Project endpoints are never used, new or not.
        assert!(!layered.endpoints.contains_key("team"));
        for name in ["litellm", "team"] {
            assert!(
                layered.notices.iter().any(
                    |n| matches!(n, Notice::ProjectEndpointIgnored { name: n, .. } if n == name)
                ),
                "{name}"
            );
        }

        // An untrusted realm contributes no endpoints, and the project's
        // do not fill in.
        let layered = load_layered_with(
            &realm,
            None,
            &project,
            RealmTrust::DenyOnly(Untrusted::NotRecorded),
            None,
        )
        .unwrap();
        assert!(layered.endpoints.is_empty());
        assert!(layered.notices.iter().any(|n| matches!(
            n,
            Notice::UntrustedRealmDenyOnly { endpoints: 1, .. }
        )));
    }

    #[test]
    fn confinement_grants_come_from_user_layers_only() {
        let tmp = tempfile::tempdir().unwrap();
        let (realm, project) = (tmp.path().join("realm"), tmp.path().join("p"));
        let global = tmp.path().join("global").join(RULES_FILE_NAME);
        write(
            global.parent().unwrap(),
            "[confinement]\nextra_read = [\"/g/read\"]\nextra_write = [\"/g/write\"]\n",
        );
        write(&realm, "[confinement]\nextra_read = [\"/r/read\"]\n");
        write(
            &project,
            "[confinement]\nextra_read = [\"/p/a\"]\nextra_write = [\"/p/b\"]\n",
        );
        let overlay = cli("[confinement]\nextra_write = [\"/c/write\"]\n");
        let layered = load_layered_with(
            &realm,
            Some(&global),
            &project,
            RealmTrust::Owned,
            Some(&overlay),
        )
        .unwrap();
        // Global, owned realm and command-line overlay accumulate in order;
        // the project's are ignored with a notice.
        assert_eq!(
            layered.confinement,
            ConfinementSection {
                extra_read: vec![PathBuf::from("/g/read"), PathBuf::from("/r/read")],
                extra_write: vec![PathBuf::from("/g/write"), PathBuf::from("/c/write")],
            }
        );
        assert!(layered.notices.iter().any(|n| matches!(
            n,
            Notice::ProjectConfinementIgnored { count: 2, .. }
        )));
        let texts: Vec<String> = layered.notices.iter().map(ToString::to_string).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("ignored 2 [confinement] grant(s)")),
            "{texts:?}"
        );

        // An untrusted realm file's grants are ignored with its notice.
        let layered = load_layered_with(
            &realm,
            Some(&global),
            &project,
            RealmTrust::DenyOnly(Untrusted::Changed),
            None,
        )
        .unwrap();
        assert_eq!(layered.confinement.extra_read, vec![PathBuf::from("/g/read")]);
        let notice = layered
            .notices
            .iter()
            .find(|n| matches!(n, Notice::UntrustedRealmDenyOnly { confinement: 1, .. }))
            .unwrap();
        assert!(notice.to_string().contains("1 [confinement] grant(s)"), "{notice}");

        // Unknown keys in the table are a parse error.
        write(&realm, "[confinement]\nextra = [\"/x\"]\n");
        assert!(load_layered_with(&realm, None, &project, RealmTrust::Owned, None).is_err());
    }
}
