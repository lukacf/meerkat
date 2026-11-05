//! Error type for the `rkat-tui` surface.

use std::path::PathBuf;

/// Top-level error for the TUI surface.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// The working directory or an explicit root could not be resolved.
    #[error("failed to resolve project root from {path}: {source}")]
    ProjectRoot {
        /// Path that was being resolved.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A generic I/O failure with context.
    #[error("{context}: {source}")]
    Io {
        /// What was being attempted.
        context: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Realm selection flags were invalid (e.g. `--realm` with `--isolated`).
    #[error("invalid realm selection: {0}")]
    Realm(#[from] meerkat_core::RuntimeBootstrapError),
    /// The realm config could not be loaded or env overrides were invalid.
    #[error("{context}: {source}")]
    Config {
        /// What was being attempted.
        context: &'static str,
        /// Underlying config error.
        #[source]
        source: meerkat_core::ConfigError,
    },
    /// The realm persistence backend could not be opened.
    #[error("failed to open realm persistence: {0}")]
    Persistence(#[from] meerkat::PersistenceError),
    /// A session store operation failed.
    #[error("session store error: {0}")]
    Store(#[from] meerkat_store::StoreError),
    /// Listing or reading the realm's sessions failed.
    #[error("session store error: {0}")]
    SessionStore(#[from] meerkat::SessionStoreError),
    /// The LLM client could not be constructed.
    #[error("failed to build LLM client: {0}")]
    LlmClient(#[from] meerkat_llm_core::FactoryError),
    /// A session service operation failed.
    #[error("session error: {0}")]
    Session(#[from] meerkat_core::service::SessionError),
    /// A session control operation (system-context append) failed.
    #[error("session control error: {0}")]
    SessionControl(#[from] meerkat_core::service::SessionControlError),
    /// A session reference (`last`, `~N`, id, or prefix) did not resolve.
    #[error("cannot resolve session `{reference}`: {reason}")]
    SessionRef {
        /// The reference supplied by the user.
        reference: String,
        /// Why it did not resolve.
        reason: String,
    },
    /// The effective MCP set (realm servers, project and user `mcp.toml`)
    /// could not be composed: a file cannot be read or parsed, or a realm
    /// server and a file server share a name with different definitions.
    #[error("MCP config error: {0}")]
    McpConfig(#[from] meerkat_core::mcp_config::EffectiveMcpServersError),
    /// No endpoint/model could be selected, an endpoint is unusable, or the
    /// endpoint projection into the realm config failed.
    #[error(transparent)]
    Endpoint(#[from] crate::endpoints::EndpointError),
    /// The session's external tools (file and note tools) could not be
    /// composed into one gateway.
    #[error("cannot compose the file and note tools: {0}")]
    ToolGateway(#[source] meerkat_core::error::ToolError),
    /// The user-global config directory and the realm directory are the
    /// same directory, or one holds the other (`--workspace ~` with the
    /// default `~/.rkat/tui`): their `tui.toml`, trust records and notes
    /// would collide.
    #[error(
        "the config dir {} and the realm dir {} {relation}; their tui.toml, trust records and notes would collide; pass --config-dir <other dir> to keep the user-global files elsewhere",
        .config.display(),
        .realm.display()
    )]
    ConfigDirCollision {
        /// The resolved user-global config directory.
        config: PathBuf,
        /// The realm directory.
        realm: PathBuf,
        /// How they collide (`are the same directory`, …).
        relation: &'static str,
    },
    /// stdin or stdout is not a terminal; the TUI needs one.
    #[error(
        "stdin and stdout must be a terminal; rkat-tui is interactive (use `rkat run` for scripts)"
    )]
    NotATerminal,
    /// The runtime-backed session service could not be composed.
    #[error("failed to compose the session runtime: {0}")]
    Composition(#[from] meerkat::surface::RuntimeBackedCompositionError),
    /// A session could not be made live on the runtime.
    #[error("failed to open the session on the runtime: {0}")]
    Materialize(#[from] meerkat::surface::SurfaceRuntimeMaterializeError),
    /// The session runtime refused or failed an operation.
    #[error("session runtime error: {0}")]
    Runtime(#[from] meerkat_runtime::RuntimeDriverError),
    /// The session's event stream could not be opened.
    #[error("cannot follow the session's events: {0}")]
    EventStream(#[from] meerkat_core::comms::StreamError),
    /// The runtime refused an input.
    #[error(transparent)]
    Admission(#[from] crate::session::AdmissionError),
    /// The terminal UI failed.
    #[error(transparent)]
    Ui(#[from] crate::ui::UiError),
    /// A command-line rule or setting flag is invalid.
    #[error(transparent)]
    Overlay(#[from] crate::overlay::OverlayError),
    /// The approval rules or the escalation ledger could not be loaded.
    #[error("approval setup failed: {0}")]
    Approval(#[from] crate::approval::ApprovalError),
    /// The selected process confinement cannot be installed on this host,
    /// or the command line asks for less than the realm's `[confinement]`.
    #[error("process confinement: {0}")]
    Confinement(#[from] crate::bootstrap::confinement::ConfinementError),
}

impl TuiError {
    /// Wrap an I/O error with a short description of what was being attempted.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Process exit code for this error.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::ProjectRoot { .. }
            | Self::Realm(_)
            | Self::Config { .. }
            | Self::SessionRef { .. }
            | Self::Endpoint(_)
            | Self::McpConfig(_)
            | Self::Overlay(_)
            | Self::Approval(_)
            | Self::Confinement(_)
            | Self::ConfigDirCollision { .. }
            | Self::NotATerminal => 2,
            Self::Io { .. }
            | Self::Persistence(_)
            | Self::Store(_)
            | Self::SessionStore(_)
            | Self::LlmClient(_)
            | Self::Session(_)
            | Self::SessionControl(_)
            | Self::ToolGateway(_)
            | Self::Composition(_)
            | Self::Materialize(_)
            | Self::Runtime(_)
            | Self::EventStream(_)
            | Self::Admission(_)
            | Self::Ui(_) => 1,
        }
    }
}
