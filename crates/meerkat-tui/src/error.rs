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
    /// The provider could not be determined or is not supported.
    #[error("{0}")]
    Provider(String),
    /// No API key was found for the selected provider.
    #[error(
        "no API key for provider `{provider}`: set it in the realm config or the provider's environment variable"
    )]
    MissingApiKey {
        /// Canonical provider name.
        provider: &'static str,
    },
    /// The LLM client could not be constructed.
    #[error("failed to build LLM client: {0}")]
    LlmClient(#[from] meerkat_llm_core::FactoryError),
    /// A session service operation failed.
    #[error("session error: {0}")]
    Session(#[from] meerkat_core::service::SessionError),
    /// A session reference (`last`, `~N`, id, or prefix) did not resolve.
    #[error("cannot resolve session `{reference}`: {reason}")]
    SessionRef {
        /// The reference supplied by the user.
        reference: String,
        /// Why it did not resolve.
        reason: String,
    },
    /// An MCP config file (`.rkat/mcp.toml`) could not be read or parsed.
    #[error("MCP config error: {0}")]
    McpConfig(#[from] meerkat_core::McpConfigError),
    /// No endpoint/model could be selected, or an endpoint is unusable.
    #[error("{0}")]
    Endpoint(String),
    /// MCP servers could not be started.
    #[error("MCP: {0}")]
    Mcp(String),
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
    #[error("the runtime refused the input: {0}")]
    InputRejected(String),
    /// The terminal UI failed.
    #[error(transparent)]
    Ui(#[from] crate::ui::UiError),
    /// A command-line rule or setting flag is invalid.
    #[error(transparent)]
    Overlay(#[from] crate::overlay::OverlayError),
    /// The approval rules or the escalation ledger could not be loaded.
    #[error("approval setup failed: {0}")]
    Approval(#[from] crate::approval::ApprovalError),
    /// The selected process confinement cannot be installed on this host.
    #[error("process confinement: {0}")]
    Confinement(String),
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
            | Self::Provider(_)
            | Self::MissingApiKey { .. }
            | Self::SessionRef { .. }
            | Self::Endpoint(_)
            | Self::McpConfig(_)
            | Self::Overlay(_)
            | Self::Approval(_)
            | Self::Confinement(_)
            | Self::NotATerminal => 2,
            Self::Io { .. }
            | Self::Persistence(_)
            | Self::Store(_)
            | Self::SessionStore(_)
            | Self::LlmClient(_)
            | Self::Session(_)
            | Self::Mcp(_)
            | Self::Composition(_)
            | Self::Materialize(_)
            | Self::Runtime(_)
            | Self::EventStream(_)
            | Self::InputRejected(_)
            | Self::Ui(_) => 1,
        }
    }
}
