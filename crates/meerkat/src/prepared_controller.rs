//! A controller prepared under a realm's actual native owner before the
//! session it serves exists, retained unchanged, and consumed exactly once by
//! that session's build.
//!
//! The factory prepares the controller graph with the same provider
//! resolution, decoration, fallback selection and pin as an ordinary build,
//! under the realm machine's own auth lifecycle owner. The graph never leaves
//! its single shared cell except into the one build that consumes it. That
//! build attaches only the session's event channel (through the adapters'
//! set-once late slot) and uses the retained client graph, event tap and pin
//! as they are: it does not resolve, decorate, select or pin again, and it
//! sends no request. It first compares the facts the graph was prepared from
//! with its own: pure configuration and selection, never a credential
//! resolution or refresh (credential freshness stays with the auth owner).
//!
//! Every [`PreparedControllerTicket`] clone names the same cell. The cell is
//! taken once; a retired cell refuses every alias. An unrecognized payload in
//! [`meerkat_core::service::SessionBuildOptions::prepared_controller`] is a
//! typed refusal, never a fall back to ordinary resolution.
//!
//! Controllers are prepared only by a staged governed realm
//! (`governed_realm::staged`); without it a build can still be
//! handed a ticket payload, which it refuses.
#![cfg_attr(
    not(all(
        feature = "session-store",
        feature = "local-authorization",
        not(target_arch = "wasm32")
    )),
    allow(dead_code)
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_core::config::{
    CustomModelConfig, ModelFallbackConfig, ProviderToolsConfig, SelfHostedConfig,
};
use meerkat_core::handles::GeneratedAuthLeaseHandle;
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::model_fallback::ModelFallbackSkipReason;
use meerkat_core::service::SessionBuildOptions;
use meerkat_core::{
    AgentLlmClient, AgentLlmClientDecorator, AuthBindingRef, Config, ControllerModelClient,
    Provider, RealmConfigSection, RealmId, SessionId, SessionLlmIdentity, ToolCategoryOverride,
};

use crate::AgentBuildConfig;

/// Why a prepared controller was not prepared or not consumed by a build.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreparedControllerRefusal {
    /// The build option carried a payload that is not a prepared controller
    /// ticket.
    #[error("the prepared controller payload is not a prepared controller ticket")]
    ForeignPayload,
    /// The build also carried an injected LLM client.
    #[error("a prepared controller cannot be combined with an injected LLM client")]
    InjectedClient,
    /// Another build already took this prepared controller.
    #[error("the prepared controller was already consumed")]
    Consumed,
    /// The stage that prepared it retired it.
    #[error("the prepared controller was retired by the stage that prepared it")]
    Retired,
    /// Only a session-owned (runtime-backed) build can consume it.
    #[error("a prepared controller is consumed only by a session-owned runtime build")]
    NotSessionOwned,
    /// The build is for another session than the one it was prepared for.
    #[error("the build's session is not the session the controller was prepared for")]
    SessionMismatch,
    /// The build's auth lifecycle owner is not the owner it was prepared under.
    #[error("the build's auth owner is not the owner the controller was prepared under")]
    AuthOwnerMismatch,
    /// One of the build's effective controller facts differs from the
    /// prepared one.
    #[error("the build's effective {0} differs from the prepared controller's")]
    InputsChanged(&'static str),
    /// The requested composition cannot be prepared ahead of its session.
    #[error("{0} is not supported for a prepared controller")]
    Unsupported(&'static str),
}

/// The configuration and selection facts a controller graph is prepared
/// from, captured before provider resolution: the request's own controller
/// facts, every configuration section that selects, defines, authorizes
/// or shapes a request of one of its routes, and whether the output budget
/// is known before resolution (which decides whether that resolution may
/// admit an account whose stated limits are pending). Pure configuration:
/// capturing and comparing them resolves no credential and sends no request.
///
/// A realm's connector OAuth clients are left out: they serve MCP connectors,
/// never a model route.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PreparedControllerInputs {
    model: String,
    provider: Provider,
    self_hosted_server_id: Option<String>,
    auth_binding: Option<AuthBindingRef>,
    realm_id: Option<RealmId>,
    provider_params: Option<ProviderParamsOverride>,
    override_web_search: ToolCategoryOverride,
    override_image_generation: ToolCategoryOverride,
    image_generation_provider: Option<Provider>,
    build_custom_models: BTreeMap<String, CustomModelConfig>,
    config_custom_models: BTreeMap<String, CustomModelConfig>,
    self_hosted: SelfHostedConfig,
    model_fallback: ModelFallbackConfig,
    provider_tools: ProviderToolsConfig,
    realm_connections: BTreeMap<String, RealmConfigSection>,
    /// An explicit or configured output budget (not its value): a graph
    /// resolved while the account's limits were pending serves only a build
    /// whose budget does not come from those limits.
    output_budget_known: bool,
}

impl PreparedControllerInputs {
    /// The facts of `build` under the effective `config`, for the canonical
    /// `provider` and self-hosted server the registry selected.
    pub(crate) fn capture(
        config: &Config,
        build: &AgentBuildConfig,
        provider: Provider,
        self_hosted_server_id: &Option<String>,
    ) -> Self {
        Self {
            model: build.model.clone(),
            provider,
            self_hosted_server_id: self_hosted_server_id.clone(),
            auth_binding: build.auth_binding.clone(),
            realm_id: build.realm_id.clone(),
            provider_params: build.provider_params.clone(),
            override_web_search: build.override_web_search,
            override_image_generation: build.override_image_generation,
            image_generation_provider: build.image_generation_provider,
            build_custom_models: build.custom_models.clone(),
            config_custom_models: config.models.custom.clone(),
            self_hosted: config.self_hosted.clone(),
            model_fallback: config.model_fallback.clone(),
            provider_tools: config.provider_tools.clone(),
            realm_connections: config
                .realm
                .iter()
                .map(|(realm, section)| {
                    let mut section = section.clone();
                    section.connector_client.clear();
                    (realm.clone(), section)
                })
                .collect(),
            output_budget_known: build.output_budget_known(config),
        }
    }

    /// The first fact that differs in `current`, named for the refusal.
    pub(crate) fn first_difference(&self, current: &Self) -> Option<&'static str> {
        let Self {
            model,
            provider,
            self_hosted_server_id,
            auth_binding,
            realm_id,
            provider_params,
            override_web_search,
            override_image_generation,
            image_generation_provider,
            build_custom_models,
            config_custom_models,
            self_hosted,
            model_fallback,
            provider_tools,
            realm_connections,
            output_budget_known,
        } = self;
        [
            (*model != current.model, "model"),
            (*provider != current.provider, "provider"),
            (
                *self_hosted_server_id != current.self_hosted_server_id,
                "self-hosted server",
            ),
            (*auth_binding != current.auth_binding, "auth binding"),
            (*realm_id != current.realm_id, "realm"),
            (
                *provider_params != current.provider_params,
                "provider params",
            ),
            (
                *override_web_search != current.override_web_search,
                "web search override",
            ),
            (
                *override_image_generation != current.override_image_generation,
                "image generation override",
            ),
            (
                *image_generation_provider != current.image_generation_provider,
                "image generation provider",
            ),
            (
                *build_custom_models != current.build_custom_models,
                "build custom models",
            ),
            (
                *config_custom_models != current.config_custom_models,
                "configured custom models",
            ),
            (
                *self_hosted != current.self_hosted,
                "self-hosted configuration",
            ),
            (*model_fallback != current.model_fallback, "model fallback"),
            (*provider_tools != current.provider_tools, "provider tools"),
            (
                *realm_connections != current.realm_connections,
                "realm connections",
            ),
            (
                *output_budget_known != current.output_budget_known,
                "output budget",
            ),
        ]
        .into_iter()
        .find_map(|(differs, fact)| differs.then_some(fact))
    }
}

/// The retained controller graph of one prepared controller.
pub(crate) struct PreparedControllerGraph {
    /// The final agent-facing client: decorated, and the fallback selector
    /// over its retained candidates when fallback is configured.
    pub(crate) llm_adapter: Arc<dyn AgentLlmClient>,
    /// The pin taken once from that client.
    pub(crate) controller: ControllerModelClient,
    /// The one event tap shared by every retained adapter, handed to the
    /// consuming build's agent.
    pub(crate) event_tap: meerkat_core::EventTap,
    /// The set-once run channel slot shared by every retained adapter.
    pub(crate) late_event: meerkat_llm_core::LateEventChannel,
    pub(crate) account_output_token_limit: Option<u32>,
    /// The selected account's prompt limit, as the client stated it.
    #[cfg(feature = "session-compaction")]
    pub(crate) account_prompt_token_limit: Option<u32>,
    pub(crate) credential_identity: Option<meerkat_core::AuthCredentialIdentity>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) auto_image_generation_executor:
        Option<Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
    /// The model and auth binding provider resolution settled.
    pub(crate) resolved_model: String,
    pub(crate) resolved_auth_binding: Option<AuthBindingRef>,
    /// The auth lifecycle owner the graph was resolved under.
    pub(crate) auth_lease: GeneratedAuthLeaseHandle,
    /// The effective model registry the graph was resolved against. Its
    /// authority minted the fallback candidates' profile witnesses, so the
    /// consuming build's agent uses this registry (whose defining facts it
    /// compared) rather than one built anew.
    pub(crate) registry: meerkat_core::ModelRegistry,
    /// The final-client decorator it was built with, retained so its
    /// identity stays comparable.
    pub(crate) decorator: Option<AgentLlmClientDecorator>,
    pub(crate) inputs: PreparedControllerInputs,
    /// The fallback targets and skips the configuration named when prepared.
    pub(crate) fallback_identities: Vec<SessionLlmIdentity>,
    pub(crate) fallback_skips: Vec<(SessionLlmIdentity, ModelFallbackSkipReason)>,
}

impl PreparedControllerGraph {
    /// The resolved identity of the graph's primary route.
    pub(crate) fn resolved_identity(&self) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: self.resolved_model.clone(),
            provider: self.inputs.provider,
            self_hosted_server_id: self.inputs.self_hosted_server_id.clone(),
            provider_params: self.inputs.provider_params.clone(),
            auth_binding: self.resolved_auth_binding.clone(),
        }
    }
}

/// Whether `current` is the retained decorator itself (or both are absent).
pub(crate) fn same_decorator(
    retained: Option<&AgentLlmClientDecorator>,
    current: Option<&AgentLlmClientDecorator>,
) -> bool {
    match (retained, current) {
        (None, None) => true,
        (Some(retained), Some(current)) => {
            std::ptr::addr_eq(Arc::as_ptr(retained), Arc::as_ptr(current))
        }
        _ => false,
    }
}

/// The configuration-owned facts of fallback skips: each skipped target and
/// why it was skipped.
pub(crate) fn fallback_skip_facts(
    skipped: &[meerkat_core::AgentLlmFallbackSkippedTarget],
) -> Vec<(SessionLlmIdentity, ModelFallbackSkipReason)> {
    skipped
        .iter()
        .map(|target| (target.identity.clone(), target.reason))
        .collect()
}

enum CellState {
    Ready(Box<PreparedControllerGraph>),
    Taken,
    Retired,
}

/// What retiring a cell found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedControllerDisposal {
    /// No controller was prepared.
    NotPrepared,
    /// The retained graph was drained and dropped; every alias is inert.
    Drained,
    /// A build had already taken the graph; that build owns it now.
    Taken,
    /// The cell was already retired.
    AlreadyRetired,
}

/// The single shared owner of one prepared controller graph.
pub(crate) struct PreparedControllerCell {
    session_id: SessionId,
    state: Mutex<CellState>,
}

impl PreparedControllerCell {
    pub(crate) fn new(session_id: SessionId, graph: PreparedControllerGraph) -> Arc<Self> {
        Arc::new(Self {
            session_id,
            state: Mutex::new(CellState::Ready(Box::new(graph))),
        })
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Take the graph once, after `accept` approved it under the cell lock.
    /// A refusal leaves the graph in the cell.
    pub(crate) fn take_if(
        &self,
        accept: impl FnOnce(&PreparedControllerGraph) -> Result<(), PreparedControllerRefusal>,
    ) -> Result<Box<PreparedControllerGraph>, PreparedControllerRefusal> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match &*state {
            CellState::Ready(graph) => accept(graph)?,
            CellState::Taken => return Err(PreparedControllerRefusal::Consumed),
            CellState::Retired => return Err(PreparedControllerRefusal::Retired),
        }
        match std::mem::replace(&mut *state, CellState::Taken) {
            CellState::Ready(graph) => Ok(graph),
            // Checked above under the same lock.
            CellState::Taken | CellState::Retired => Err(PreparedControllerRefusal::Consumed),
        }
    }

    /// Retire the cell: a ready graph is drained and dropped now, and every
    /// alias refuses from here on.
    pub(crate) fn retire(&self) -> PreparedControllerDisposal {
        let previous = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::replace(&mut *state, CellState::Retired)
        };
        match previous {
            CellState::Ready(graph) => {
                drop(graph);
                PreparedControllerDisposal::Drained
            }
            CellState::Taken => PreparedControllerDisposal::Taken,
            CellState::Retired => PreparedControllerDisposal::AlreadyRetired,
        }
    }
}

/// A handle to one prepared controller, for one session build.
///
/// Clones are aliases of the same single cell: exactly one build takes the
/// prepared controller, every other alias then refuses, and all of them
/// refuse once the stage that prepared it retired it. The ticket carries no
/// client, credential or auth owner a caller can use.
#[derive(Clone)]
pub struct PreparedControllerTicket {
    cell: Arc<PreparedControllerCell>,
}

impl PreparedControllerTicket {
    pub(crate) fn new(cell: Arc<PreparedControllerCell>) -> Self {
        Self { cell }
    }

    /// The pre-assigned session the controller was prepared for.
    pub fn session_id(&self) -> &SessionId {
        self.cell.session_id()
    }

    /// Seat `build` on the new session minted under the pre-assigned id,
    /// carrying this ticket (see
    /// [`SessionBuildOptions::mint_session_with_id`] and
    /// [`SessionBuildOptions::prepared_controller`]).
    pub fn seat_build(&self, build: &mut SessionBuildOptions) {
        build.mint_session_with_id(self.session_id().clone());
        build.prepared_controller = Some(encode_prepared_controller_for_service(self.clone()));
    }

    pub(crate) fn cell(&self) -> &PreparedControllerCell {
        &self.cell
    }
}

impl std::fmt::Debug for PreparedControllerTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedControllerTicket")
            .field("session_id", self.session_id())
            .finish_non_exhaustive()
    }
}

/// Encode a prepared controller ticket for transport in
/// [`SessionBuildOptions::prepared_controller`].
pub fn encode_prepared_controller_for_service(
    ticket: PreparedControllerTicket,
) -> Arc<dyn std::any::Any + Send + Sync> {
    Arc::new(ticket)
}

/// Decode the build option; any other payload is a typed refusal.
pub(crate) fn decode_prepared_controller(
    payload: &Arc<dyn std::any::Any + Send + Sync>,
) -> Result<PreparedControllerTicket, PreparedControllerRefusal> {
    payload
        .as_ref()
        .downcast_ref::<PreparedControllerTicket>()
        .cloned()
        .ok_or(PreparedControllerRefusal::ForeignPayload)
}
