//! Present native authority for governed MCP server attachment.
//!
//! On a runtime with an installed native work host, an MCP server is attached
//! to a session (live), written to configuration (persisted) or connected for a
//! configured session only through a prepared, authenticated attachment control
//! owned by that host. These traits are the surface-facing view of such a
//! control: the surfaces ask it again at each effect boundary, after any wait,
//! and never infer permission from a saved `mcp.toml` row, a server name or an
//! earlier permit. Ungoverned runtimes do not use them.

use crate::mcp_config::{McpScope, McpServerConfig};

/// Why a governed attachment effect was refused. Local and typed: no variant
/// touches the session, its runs or another realm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpAttachmentRefusal {
    /// The current mandate, grant or account decision does not allow it.
    #[error("the attachment is not permitted")]
    Denied,
    /// The authority needed to decide is not available now.
    #[error("attachment authority is unavailable")]
    Unavailable,
    /// The prepared attachment no longer matches the current session,
    /// destination or executor attachment.
    #[error("the prepared attachment is no longer current")]
    NotCurrent,
    /// The current decision requires an operation review this control does
    /// not carry.
    #[error("the attachment requires a review")]
    ReviewRequired,
    /// The control does not cover this exact server descriptor or effect.
    #[error("the attachment control does not cover this server or effect")]
    Mismatch,
}

/// One prepared, authenticated attachment control for an exact server
/// descriptor. Each call rechecks the current mandate, grant/account decision,
/// review requirement and native attachment; a success is consumed by that one
/// effect and is not permission for another.
pub trait McpAttachmentAuthority: Send + Sync {
    /// The exact server descriptor this control was prepared for.
    fn server(&self) -> &McpServerConfig;

    /// Recheck at the live apply boundary, before the server is discovered,
    /// spawned or its tools become visible to the session.
    fn authorize_apply(&self) -> Result<(), McpAttachmentRefusal>;

    /// Recheck before the descriptor is written to configuration in `scope`.
    /// The current native configuration owner decides for the exact realm,
    /// descriptor and effect; a live-session permit alone is not enough.
    fn authorize_persist(&self, scope: McpScope) -> Result<(), McpAttachmentRefusal>;
}

/// Installed by a governed host composition: authorizes each configured
/// server for a fresh session before discovery, spawn or first use. The
/// configured row is configuration, never present authority.
pub trait ConfiguredMcpAttachmentAuthority: Send + Sync {
    fn authorize_configured(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal>;
}

/// The truthful outcome of one governed live MCP add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpAttachmentOutcome {
    /// The descriptor was written to configuration.
    pub persisted: bool,
    /// The add was staged for the session's next boundary.
    pub staged: bool,
}

/// One reviewed MCP server to attach to the current member live: the exact
/// descriptor, the native prepared control for it (never inferred from a
/// session id or an account review), and, separately, the configuration
/// owner to persist it under. The persisted write is its own effect, decided
/// by the control under the config lock; the live add is staged with the
/// control, which travels to the connection's physical entry.
pub struct LiveMcpAttachment {
    pub control: std::sync::Arc<dyn McpAttachmentAuthority>,
    pub server: McpServerConfig,
    pub persist: Option<crate::mcp_config::McpConfigMutationAuthority>,
}

impl std::fmt::Debug for LiveMcpAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveMcpAttachment")
            .field("server", &self.server.name)
            .field(
                "persist",
                &self.persist.as_ref().map(|persist| &persist.scope),
            )
            .finish_non_exhaustive()
    }
}

/// Why a live MCP attachment to the current member did not stage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveMcpAttachError {
    /// The control, the current member or the configuration owner refused.
    #[error("MCP attachment refused: {0}")]
    Refused(McpAttachmentRefusal),
    /// The member's turn boundary is held (this proves a held boundary, not
    /// necessarily an active turn): nothing was attached and nothing waited.
    /// `Busy` grants no permission, and whether to try again is for the
    /// caller's owner to decide; it is neither an instruction to retry nor a
    /// promise of automatic retry. Current-turn attachment is not
    /// implemented: an in-turn caller always gets `Busy`.
    #[error("the member's turn boundary is held; nothing was attached")]
    Busy,
    /// This member has no live MCP attachment surface (it was not built by a
    /// governed session-owned construction).
    #[error("this member has no live MCP attachment surface")]
    Unsupported,
    /// Writing the server to configuration failed.
    #[error("{0}")]
    Persist(String),
    /// Staging or applying the server failed (a persisted write is reported
    /// separately and is not undone by a later live failure).
    #[error("{0}")]
    Stage(String),
}

/// The live MCP attachment surface of one built member: its construction's
/// own router, which the member's actor mutates between turns.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait LiveMcpAttachmentSurface: Send + Sync {
    /// Stage `attachment` on this member's router and apply it now (the
    /// actor serializes this with turns). Reports the persisted and staged
    /// effects separately and returns the exact attempt to observe; it makes
    /// no connection-success claim.
    async fn attach(
        &self,
        attachment: LiveMcpAttachment,
    ) -> Result<LiveMcpAttachmentReceipt, LiveMcpAttachError>;
}

/// What one live attachment did, and the exact attempt it started.
#[derive(Debug, Clone)]
pub struct LiveMcpAttachmentReceipt {
    pub outcome: McpAttachmentOutcome,
    pub attempt: McpAttachmentAttempt,
}

/// Where the exact attempt of one live attachment stands now, for one tool
/// of its server, from the attaching router's own accepted
/// outcome of that attempt. A local observation, never a permit: it extends
/// no authority, reserves nothing and proves no cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAttachmentReadiness {
    /// Staged or connecting; the router has accepted no outcome yet. A wait
    /// deadline returns `Pending`: not settled yet, not a failure.
    Pending,
    /// The attempt's connection is installed and is the server's current
    /// connection, and its published routes serve the tool.
    Usable { connection_generation: u64 },
    /// The attempt's connection is installed and current, but no published
    /// route serves the tool on it.
    ToolNotRouted { connection_generation: u64 },
    /// The router refused the attempt's apply boundary.
    Rejected { detail: String },
    /// The attempt's connection failed, or its install was refused.
    Failed { detail: String },
    /// A newer intent for the server replaced the attempt, the router
    /// refused its result as stale, or its connection is no longer the
    /// server's current one.
    Superseded,
    /// The attaching router is gone (taken for shutdown, or dropped). This
    /// is not proof that its shutdown, or any process close, finished.
    Retired,
    /// The member actor the attempt was attached through is no longer the
    /// current one.
    NotCurrent,
}

/// Observes one exact attachment attempt.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait McpAttachmentAttemptObserver: Send + Sync {
    /// The attempt's readiness for `tool` now, from one read of its
    /// resolution and, once installed, of the router's current connection
    /// and routes. It neither drives progress nor waits for the attempt, but
    /// it has no deadline: its reads can wait behind a writer of the router.
    /// Use `wait` for a deadline-bounded answer.
    async fn observe(&self, tool: &str) -> McpAttachmentReadiness;

    /// Wait at most `timeout` for the attempt to leave `Pending`, driving
    /// the attaching router's own progress, and return its readiness for
    /// `tool`. One deadline bounds every step, lock acquisition included.
    /// The timeout bounds the observation only: it cancels nothing, and
    /// `Pending` after it is a local not-ready answer.
    async fn wait(&self, tool: &str, timeout: std::time::Duration) -> McpAttachmentReadiness;

    /// The typed setup failure of this attempt, when it failed as a stdio
    /// server launch, handshake or tool discovery the transport could
    /// classify ([`McpStdioSetupFailure`]). Host and operator facing; it
    /// carries no server text. `None` when the attempt did not fail that way,
    /// has not resolved, or the observer cannot tell.
    async fn setup_failure(&self) -> Option<McpStdioSetupFailure> {
        None
    }

    /// The bounded, redacted tail of this exact attempt's stderr, while the
    /// attaching router still retains it ([`McpStdioStderrTail`]).
    ///
    /// SENSITIVE and host or operator only: see [`McpStdioStderrTail`]. It
    /// must never be placed in model context. `None` once the attempt is
    /// superseded or retired, the router is gone, or no tail was captured.
    async fn stderr_tail(&self) -> Option<McpStdioStderrTail> {
        None
    }
}

/// The stage of a stdio MCP server's setup that a failure happened in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpStdioSetupPhase {
    /// Before the server process existed: launch preparation and the spawn.
    Launch,
    /// The process existed and the initialize handshake had not completed.
    Initialize,
    /// The handshake completed and the server's tools were being listed and
    /// checked.
    ToolDiscovery,
}

/// How a stdio MCP server process ended, as its own exit (see
/// [`McpStdioSetupFailureClass::Closed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpStdioExit {
    /// It exited with this status code.
    Code(i32),
    /// It was terminated by this signal number (Unix only).
    Signal(i32),
}

/// What the transport established about a failed stdio MCP server setup. A
/// class is set only where the transport can prove it; anything else is
/// [`Self::Unknown`], never a guess. No variant carries server text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpStdioSetupFailureClass {
    /// The server process could not be started: the spawn itself failed, or
    /// the started process lacked its protocol pipes. Confinement and other
    /// typed refusals before the spawn keep their own typed errors and are
    /// not reported here.
    SpawnFailed,
    /// The server's side of the protocol pipes closed: its stdout ended, or
    /// a write to its stdin found the pipe broken. `exit` is the server's own
    /// exit status when the close of its process proved it exited before any
    /// escalation; `None` when that could not be established (for example on
    /// macOS, where the exit cannot be observed without reaping it).
    Closed { exit: Option<McpStdioExit> },
    /// The server answered with a JSON-RPC error; `code` is its error code
    /// (`-32601` is "method not found"). The error message is not carried.
    Rejected { code: i32 },
    /// The server answered, but not with a valid response: an unexpected
    /// result, a conflicting response id, or an unreadable or oversized
    /// frame.
    ProtocolError,
    /// The setup's deadline expired during the reported phase.
    TimedOut,
    /// The setup failed in a way the transport did not classify.
    Unknown,
}

/// The typed failure of one stdio MCP server setup attempt: the exact server
/// and attempt, the phase it failed in, and what the transport established.
/// Host and operator facing; it carries no server text, so it contains no
/// server output, paths or secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpStdioSetupFailure {
    /// The server name.
    pub server: String,
    /// The attaching router's sequence for this exact attempt.
    pub attempt: u64,
    pub phase: McpStdioSetupPhase,
    pub class: McpStdioSetupFailureClass,
    /// Whether the router still retains this attempt's stderr tail
    /// ([`McpAttachmentAttemptObserver::stderr_tail`]).
    pub stderr_tail_retained: bool,
}

impl McpStdioSetupFailure {
    #[must_use]
    pub fn new(
        server: impl Into<String>,
        attempt: u64,
        phase: McpStdioSetupPhase,
        class: McpStdioSetupFailureClass,
        stderr_tail_retained: bool,
    ) -> Self {
        Self {
            server: server.into(),
            attempt,
            phase,
            class,
            stderr_tail_retained,
        }
    }
}

/// The bounded tail of one stdio MCP server setup attempt's stderr.
///
/// SENSITIVE. Server stderr is arbitrary process output: it can contain
/// secrets, host paths, headers and other host details. A redaction pass for
/// obvious secret shapes and for the attempt's own launch environment values
/// has run over `text`, but redaction is defence in depth only and does not
/// make arbitrary stderr safe. Show it only to the host or its operator,
/// under the host's current authority; never place it in model context,
/// events or messages that reach the model or untrusted clients. Its `Debug`
/// prints sizes only.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpStdioStderrTail {
    /// The newest retained bytes as lossy UTF-8, redacted, with control
    /// characters (C0, DEL and C1) other than line feed and tab removed, so
    /// it carries no terminal escape sequences. When earlier bytes were
    /// dropped it starts after the first line break past the cut (or after
    /// the first whitespace when the retained bytes hold no line break), so
    /// nothing cut at the edge is shown in part, and a private key cut at
    /// the edge is redacted up to its END marker.
    pub text: String,
    /// The bytes the server wrote to stderr while it was captured.
    pub total_bytes: u64,
    /// Whether earlier bytes were dropped because of the size bound.
    pub truncated: bool,
    /// Whether the capture reached the end of the stream (every writer
    /// closed it) rather than stopping at its bound after the process ended.
    pub complete: bool,
}

impl McpStdioStderrTail {
    #[must_use]
    pub fn new(text: String, total_bytes: u64, truncated: bool, complete: bool) -> Self {
        Self {
            text,
            total_bytes,
            truncated,
            complete,
        }
    }
}

impl std::fmt::Debug for McpStdioStderrTail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpStdioStderrTail")
            .field("text_bytes", &self.text.len())
            .field("total_bytes", &self.total_bytes)
            .field("truncated", &self.truncated)
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

/// The exact attempt one live attachment started on its member's router.
#[derive(Clone)]
pub struct McpAttachmentAttempt {
    server: String,
    observer: std::sync::Arc<dyn McpAttachmentAttemptObserver>,
}

impl McpAttachmentAttempt {
    pub fn new(
        server: impl Into<String>,
        observer: std::sync::Arc<dyn McpAttachmentAttemptObserver>,
    ) -> Self {
        Self {
            server: server.into(),
            observer,
        }
    }

    /// The attached server's name.
    pub fn server(&self) -> &str {
        &self.server
    }

    /// The same attempt observed through `wrap` (an owner that adds its own
    /// currentness check around every observation).
    #[must_use]
    pub fn observed_through(
        self,
        wrap: impl FnOnce(
            std::sync::Arc<dyn McpAttachmentAttemptObserver>,
        ) -> std::sync::Arc<dyn McpAttachmentAttemptObserver>,
    ) -> Self {
        Self {
            server: self.server,
            observer: wrap(self.observer),
        }
    }

    /// See [`McpAttachmentAttemptObserver::observe`].
    pub async fn observe(&self, tool: &str) -> McpAttachmentReadiness {
        self.observer.observe(tool).await
    }

    /// See [`McpAttachmentAttemptObserver::wait`].
    pub async fn wait(&self, tool: &str, timeout: std::time::Duration) -> McpAttachmentReadiness {
        self.observer.wait(tool, timeout).await
    }

    /// See [`McpAttachmentAttemptObserver::setup_failure`].
    pub async fn setup_failure(&self) -> Option<McpStdioSetupFailure> {
        self.observer.setup_failure().await
    }

    /// See [`McpAttachmentAttemptObserver::stderr_tail`]. SENSITIVE: host and
    /// operator only, never model context ([`McpStdioStderrTail`]).
    pub async fn stderr_tail(&self) -> Option<McpStdioStderrTail> {
        self.observer.stderr_tail().await
    }
}

impl std::fmt::Debug for McpAttachmentAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAttachmentAttempt")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}
