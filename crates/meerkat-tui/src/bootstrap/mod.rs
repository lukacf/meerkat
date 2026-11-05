//! Realm, config, factory and session-service construction.
//!
//! The TUI hosts its sessions on Meerkat's runtime-backed path, the one the
//! product surfaces (CLI, REST, RPC) use: a [`PersistentSessionService`] over
//! the realm's stores, with a [`MeerkatMachine`] owning input admission
//! (Queue/Steer), interruption, terminal publication and resume. Both halves
//! come from [`build_runtime_backed_service_with_default_reconfigure_host`],
//! which also installs the session-LLM reconfigure host (the model hot swap
//! of a later phase relies on it).
//!
//! 1. [`prepare`] resolves the [`RuntimeScope`] and loads the realm [`Config`]
//!    (for `--isolated` this mints a fresh realm id, so call it once).
//! 2. [`bootstrap_with`] opens the realm persistence and composes the service.
//!
//! [`bootstrap`] runs both back to back.
//!
//! Seams for later phases (deliberately absent here, not stubbed):
//! - P2 approval gate: a `SessionAgentBuilder` wrapper around
//!   [`FactoryAgentBuilder`] installing a `ToolDispatchAdmission`.
//! - P3 confinement: `AgentFactory::with_shell_confinement`; until then the
//!   shell tool stays off ([`build_factory`]).
//! - P4 endpoints and models: model selection here is `--model` /
//!   `--provider` / the realm config only ([`resolve_model`]).
//! - P5 mobs: `AgentFactory::mob(true)`.
//! - P6 instruction files beyond the native `AGENTS.md` discovery, MCP from
//!   `.rkat/mcp.toml`, native semantic memory.

pub mod credentials;
pub mod dirs;
pub mod persona;
pub mod scope;

use std::path::PathBuf;
use std::sync::Arc;

use meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host;
use meerkat::{
    AgentFactory, FactoryAgentBuilder, LlmClient, PersistentSessionService, SessionStore,
};
use meerkat_core::{Config, Provider};
use meerkat_runtime::MeerkatMachine;
use meerkat_store::{RealmBackend, realm_paths_in};

use crate::cli::Args;
use crate::error::TuiError;

pub use dirs::GlobalDirs;
pub use persona::CODING_PERSONA;
pub use scope::{RuntimeScope, load_config, resolve_scope};

/// Live sessions one TUI process may hold (the primary session plus a few
/// retiring ones while `/new` and `/resume` switch).
const MAX_SESSIONS: usize = 8;

/// File in the realm directory where the session-LLM reconfigure host keeps
/// its state (the same name the REST surface uses).
const CONFIG_STATE_FILE: &str = "config_state.json";

/// The session service the TUI drives.
pub type TuiService = PersistentSessionService<FactoryAgentBuilder>;

/// Everything a running TUI needs from the engine.
pub struct Runtime {
    /// Project, conventions roots and realm.
    pub scope: RuntimeScope,
    /// Realm config with env overrides applied.
    pub config: Config,
    /// Model for new sessions.
    pub model: String,
    /// Provider of [`Runtime::model`].
    pub provider: Provider,
    /// Session lifecycle substrate.
    pub service: Arc<TuiService>,
    /// Runtime control plane: input admission, interrupt, registrations.
    pub machine: Arc<MeerkatMachine>,
    /// Realm session store (for listing persisted sessions).
    pub store: Arc<dyn SessionStore>,
    /// Backend pinned in the realm manifest.
    pub backend: RealmBackend,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("scope", &self.scope)
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

/// Engine-level overrides for [`bootstrap_with`].
#[derive(Default, Clone)]
pub struct BootstrapOptions {
    /// Serve every session with this client instead of resolving provider
    /// credentials (tests, or a host that already built a client). With an
    /// override the user's provider token store is not opened.
    pub llm_override: Option<Arc<dyn LlmClient>>,
}

/// Resolve the runtime scope and load the realm config.
///
/// # Errors
///
/// See [`resolve_scope`] and [`load_config`].
pub async fn prepare(args: &Args) -> Result<(RuntimeScope, Config), TuiError> {
    let scope = resolve_scope(args)?;
    let config = load_config(&scope).await?;
    Ok((scope, config))
}

/// [`prepare`] followed by [`bootstrap_with`].
///
/// # Errors
///
/// See [`prepare`] and [`bootstrap_with`].
pub async fn bootstrap(args: &Args, options: BootstrapOptions) -> Result<Runtime, TuiError> {
    let (scope, config) = prepare(args).await?;
    bootstrap_with(scope, config, args, options).await
}

/// Open the realm persistence and compose the runtime-backed service.
///
/// # Errors
///
/// [`TuiError::Provider`] when no provider can be determined for the model,
/// [`TuiError::Persistence`] when the realm cannot be opened, and
/// [`TuiError::Composition`] when the runtime refuses the composition.
pub async fn bootstrap_with(
    scope: RuntimeScope,
    config: Config,
    args: &Args,
    options: BootstrapOptions,
) -> Result<Runtime, TuiError> {
    let (model, provider) = resolve_model(args, &config)?;
    let (manifest, persistence) = meerkat::open_realm_persistence_in(
        &scope.state_root,
        &scope.realm_id,
        scope.backend_hint,
        Some(scope.origin),
    )
    .await?;
    let store = persistence.session_store();
    let realm_root = realm_paths_in(&scope.state_root, &scope.realm_id).root;
    let store_path = persistence
        .store_path()
        .map_or_else(|| realm_root.clone(), PathBuf::from);

    let factory = build_factory(&scope, store_path, options.llm_override.is_none());
    let mut builder = FactoryAgentBuilder::new(factory, config.clone());
    builder.default_llm_client = options.llm_override;
    let (service, machine) = build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        MAX_SESSIONS,
        persistence,
        realm_root.join(CONFIG_STATE_FILE),
    )?;

    Ok(Runtime {
        scope,
        config,
        model,
        provider,
        service,
        machine,
        store,
        backend: manifest.backend,
    })
}

/// The agent factory every session is built with.
///
/// `project_root` and `context_root` are the workspace, so the factory's
/// native `AGENTS.md`, skills and hooks discovery see the project. Builtin
/// tools are on; the shell stays off until the approval gate (P2) and
/// confinement (P3) are wired, and mob tools until P5. The file and note
/// tools come in per session as external tools (see [`crate::session`]).
/// `with_token_store` attaches the user's provider token store, so OAuth and
/// stored keys resolve like they do for `rkat`.
fn build_factory(
    scope: &RuntimeScope,
    store_path: PathBuf,
    with_token_store: bool,
) -> AgentFactory {
    let mut factory = AgentFactory::new(store_path)
        .runtime_root(scope.realm_root())
        .project_root(scope.project_root.as_path())
        .context_root(scope.context_root.clone())
        .builtins(true)
        .shell(false)
        .mob(false);
    if let Some(user_root) = &scope.user_config_root {
        factory = factory.user_config_root(user_root.clone());
    }
    if with_token_store {
        factory = match factory.clone().with_default_token_store() {
            Ok(with_store) => with_store,
            Err(err) => {
                tracing::warn!(error = %err, "provider token store unavailable; using environment keys only");
                factory
            }
        };
    }
    factory
}

/// The model of new sessions and its provider.
///
/// `--model` wins over the realm config's `agent.model`. The provider is
/// `--provider` / `--endpoint` when it names a built-in provider, else
/// inferred from the model id through the model catalog.
///
/// # Errors
///
/// [`TuiError::Provider`] when `--provider` names no built-in provider or the
/// model's provider cannot be inferred.
pub fn resolve_model(args: &Args, config: &Config) -> Result<(String, Provider), TuiError> {
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| config.agent.model.clone());
    let named = args.provider.as_deref().or(args.endpoint.as_deref());
    let provider = match named {
        Some(name) => Provider::parse_strict(name).ok_or_else(|| {
            TuiError::Provider(format!(
                "`{name}` is not a built-in provider (anthropic, openai, gemini); custom endpoints arrive in a later release"
            ))
        })?,
        None => meerkat_models::catalog::infer_provider(&model).ok_or_else(|| {
            TuiError::Provider(format!(
                "cannot tell the provider of model `{model}`; pass --provider"
            ))
        })?,
    };
    Ok((model, provider))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["rkat-tui"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).unwrap()
    }

    #[test]
    fn model_flag_wins_and_provider_is_inferred() {
        let config = Config::default();
        let (model, provider) =
            resolve_model(&args(&["--model", "claude-sonnet-4-6"]), &config).unwrap();
        assert_eq!(model, "claude-sonnet-4-6");
        assert_eq!(provider, Provider::Anthropic);
        let (model, provider) = resolve_model(&args(&["--model", "gpt-5.5"]), &config).unwrap();
        assert_eq!((model.as_str(), provider), ("gpt-5.5", Provider::OpenAI));
    }

    #[test]
    fn explicit_provider_and_errors() {
        let config = Config::default();
        let (_, provider) = resolve_model(
            &args(&["--model", "my-local-model", "--provider", "openai"]),
            &config,
        )
        .unwrap();
        assert_eq!(provider, Provider::OpenAI);
        let err = resolve_model(&args(&["--model", "my-local-model"]), &config).unwrap_err();
        assert!(err.to_string().contains("--provider"), "{err}");
        let err = resolve_model(&args(&["--provider", "litellm"]), &config).unwrap_err();
        assert!(err.to_string().contains("not a built-in provider"), "{err}");
    }

    #[test]
    fn default_model_comes_from_the_config() {
        let mut config = Config::default();
        config.agent.model = "gemini-3.5-flash".to_string();
        let (model, provider) = resolve_model(&args(&[]), &config).unwrap();
        assert_eq!(
            (model.as_str(), provider),
            ("gemini-3.5-flash", Provider::Gemini)
        );
    }
}
