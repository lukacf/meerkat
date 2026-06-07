//! Typed per-request system-prompt policy.
//!
//! This type is the canonical owner of the per-request system-prompt decision
//! on [`AgentBuildConfig`](crate::AgentBuildConfig). It is compiled on every
//! target (including `wasm32`, which has no filesystem prompt assembly) because
//! `AgentBuildConfig` is always available; the filesystem-backed
//! `assemble_system_prompt` (non-wasm only) consumes it on non-wasm targets.

/// Typed per-request system-prompt policy.
///
/// **Dogma §10:** inherit, set, and disable are three distinct facts that an
/// overloaded `Option<String>` cannot express — `None` collapses "inherit
/// config/AGENTS/default" together with "no opinion", and there is no way to
/// say "suppress every prompt source". This mirrors the `Inherit`/`Set`/`Clear`
/// shape of `TurnMetadataOverride` and the `Inherit`/`Enable`/`Disable` shape of
/// `ToolCategoryOverride`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SystemPromptOverride {
    /// No per-request opinion: fall through to the config-file override, the
    /// config inline override, then the default prompt + AGENTS.md files.
    #[default]
    Inherit,
    /// Explicit per-request prompt. Wins outright, skipping the config and
    /// AGENTS.md sources (dispatcher/tool/extra sections are still appended).
    Set(String),
    /// Explicitly suppress *every* prompt source: no config override, no
    /// AGENTS.md, no default prompt. Only the appended sections
    /// (extra/config-tool/dispatcher) remain.
    Disable,
}

impl SystemPromptOverride {
    /// Construct from the wire-boundary `Option<String>` that surfaces still
    /// carry (`CreateSessionRequest.system_prompt`). `Some(s)` is an explicit
    /// per-request prompt (`Set`); `None` is `Inherit`.
    ///
    /// `Disable` is not yet expressible at that wire boundary (the wire field
    /// is `Option<String>`), so it can only be constructed directly by in-crate
    /// callers building an `AgentBuildConfig`.
    #[must_use]
    pub fn from_wire_option(value: Option<String>) -> Self {
        match value {
            Some(prompt) => Self::Set(prompt),
            None => Self::Inherit,
        }
    }

    /// Project back to the persisted `Option<String>` build-state field.
    ///
    /// `Set` persists the prompt; `Inherit` persists `None`. `Disable` also
    /// projects to `None` today because the persisted `SessionBuildState`
    /// shape is `Option<String>` and cannot yet carry the suppression fact —
    /// see the lane note on row #337.
    #[must_use]
    pub fn to_persisted_option(&self) -> Option<String> {
        match self {
            Self::Set(prompt) => Some(prompt.clone()),
            Self::Inherit | Self::Disable => None,
        }
    }

    /// Whether this override carries an explicit per-request decision (either a
    /// `Set` prompt or an explicit `Disable`). Used to decide whether the
    /// prompt must be (re)assembled even for a resumed session.
    #[must_use]
    pub fn is_explicit(&self) -> bool {
        !matches!(self, Self::Inherit)
    }

    /// The explicit per-request prompt text, if any (`Set` only).
    #[must_use]
    pub fn as_set_prompt(&self) -> Option<&str> {
        match self {
            Self::Set(prompt) => Some(prompt.as_str()),
            Self::Inherit | Self::Disable => None,
        }
    }
}
