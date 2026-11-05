//! A handle on one live session of the runtime-backed service.
//!
//! The session lives in the [`TuiService`] (its actor, transcript, stores)
//! and is driven through the [`MeerkatMachine`] (its runtime registration):
//!
//! - [`SessionHandle::create`] / [`SessionHandle::resume`] make a session
//!   live with [`materialize_session_with_reserved_admission_and_actor_slot`],
//!   which prepares the session's runtime bindings
//!   (`RuntimeBuildMode::SessionOwned`), builds the agent through the
//!   factory and attaches a [`PersistentRuntimeExecutor`] whose terminal
//!   publication is bound to the service-minted actor slot.
//! - Turns are runtime inputs: [`SessionHandle::submit`] admits an
//!   `Input::Prompt` with `RuntimeTurnMetadata { handling_mode }` and a
//!   caller idempotency key through
//!   [`MeerkatMachine::accept_input_with_completion`]. `Queue` runs it as a
//!   turn (behind any running one); `Steer` delivers it to the running turn
//!   at its next model boundary, or as one follow-up turn.
//! - [`SessionHandle::interrupt`] stops the current run with
//!   [`MeerkatMachine::stop_run`] (so steers that joined it are abandoned
//!   instead of re-running) and cancels the caller's outstanding inputs with
//!   [`MeerkatMachine::cancel_input_if_present`].
//! - [`SessionHandle::events`] follows every event of the session
//!   (`SessionService::subscribe_session_events`), across runs.
//! - [`SessionHandle::switch_model`] changes the live session's model (and
//!   endpoint) through the runtime's session-LLM reconfigure host
//!   ([`SessionServiceRuntimeExt::reconfigure_session_llm_identity`]); the
//!   next model call uses it.
//! - With [`SessionOptions::approval`], every build of the session (fresh
//!   or resumed) carries the approval gate as its `ToolDispatchAdmission`
//!   ([`crate::approval::admission::install`]); a resumed session's
//!   classifier transcript is seeded from its authoritative history, and
//!   every prompt or steer the user submits is recorded in it.
//! - Every build carries the runtime's declarative MCP servers
//!   ([`crate::bootstrap::mcp`]) and the native auto-compaction threshold
//!   the caller chose ([`SessionOptions::auto_compact_threshold`]); the
//!   handle remembers the threshold its agent was built with
//!   ([`SessionHandle::compaction_threshold`]).
//! - With the runtime's helper host ([`crate::mobs`]) every build enables
//!   the mob tools (`delegate`, `mob_*`) explicitly, the session registers
//!   its model and binding for its helpers to inherit, and
//!   [`SessionHandle::interrupt`] cancels its running helpers.

pub mod resolve;

use std::num::NonZeroU64;
use std::sync::Arc;

use meerkat::surface::{
    PersistentRuntimeExecutor, materialize_session_with_reserved_admission_and_actor_slot,
};
use meerkat_core::comms::EventStream;
use meerkat_core::lifecycle::run_primitive::{RuntimeTurnMetadata, TurnMetadataOverride};
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextStatus, CreateSessionRequest,
    DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionServiceControlExt,
};
use meerkat_core::{
    AgentToolDispatcher, AuthBindingRef, ContentInput, HandlingMode, InputId, Provider, Session,
    SessionId, SystemPromptOverride, ToolCategoryOverride,
};
use meerkat_runtime::completion::CompletionHandle;
use meerkat_runtime::{
    AcceptOutcome, IdempotencyKey, Input, MeerkatMachine, PromptInput,
    SessionLlmReconfigureRequest, SessionServiceRuntimeExt,
};

use crate::approval::{ApprovalEngine, admission};
use crate::bootstrap::{CODING_PERSONA, ModelChoice, Runtime, TuiService};
use crate::error::TuiError;
use crate::mobs::Helpers;

pub use resolve::{list_sessions, resolve_session_ref};

/// Source recorded with the system context the TUI appends (`/remember`).
const SYSTEM_CONTEXT_SOURCE: &str = "rkat-tui";

/// Reason recorded with every interrupt and cancellation the TUI requests.
const INTERRUPT_REASON: &str = "rkat-tui: interrupted by the user";

/// Options for [`SessionHandle::create`] and [`SessionHandle::resume`].
#[derive(Default, Clone)]
pub struct SessionOptions {
    /// Extra tools (file tools, notes) composed into the agent's dispatcher.
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Instruction sections appended after [`CODING_PERSONA`].
    pub additional_instructions: Vec<String>,
    /// Model, provider and auth binding of a fresh session, else the
    /// runtime's start-up choice. A resumed session keeps its stored model
    /// and binding (switch with [`SessionHandle::switch_model`]).
    pub choice: Option<ModelChoice>,
    /// The approval engine whose gate every tool call of the session passes.
    /// `None` runs tools ungated (embedders and tests only).
    pub approval: Option<Arc<ApprovalEngine>>,
    /// The native auto-compaction threshold of the build
    /// (`SessionBuildOptions::auto_compact_threshold_override`, see
    /// [`crate::ui::compaction`]); `None` keeps upstream's model-aware
    /// default.
    pub auto_compact_threshold: Option<NonZeroU64>,
}

/// An input the runtime accepted.
#[derive(Debug)]
pub struct Admission {
    /// The accepted input (the existing one when deduplicated).
    pub input_id: InputId,
    /// Resolves when the input reaches its terminal; `None` when it already
    /// did during admission.
    pub completion: Option<CompletionHandle>,
}

/// A live session plus the model it runs on.
#[derive(Clone)]
pub struct SessionHandle {
    service: Arc<TuiService>,
    machine: Arc<MeerkatMachine>,
    id: SessionId,
    model: String,
    provider: Provider,
    auth_binding: Option<AuthBindingRef>,
    /// The runtime's profile generation when the agent was built.
    profiles: u64,
    approval: Option<Arc<ApprovalEngine>>,
    helpers: Option<Arc<Helpers>>,
    /// The native auto-compaction threshold the agent was built with.
    threshold: Option<NonZeroU64>,
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle")
            .field("id", &self.id)
            .field("model", &self.model)
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl SessionHandle {
    /// Create a fresh session and make it live (no turn runs).
    ///
    /// # Errors
    ///
    /// [`TuiError::Session`] / [`TuiError::Materialize`] when the agent cannot
    /// be built or the runtime refuses the registration.
    pub async fn create(runtime: &Runtime, options: SessionOptions) -> Result<Self, TuiError> {
        let session = Session::new();
        let choice = options
            .choice
            .clone()
            .unwrap_or_else(|| runtime.choice.clone());
        let approval = options.approval.clone();
        let mut build = build_options(options, choice.provider, runtime);
        build.auth_binding.clone_from(&choice.auth_binding);
        build.mint_session_with_id(session.id().clone());
        if let Some(engine) = &approval {
            admission::install(&mut build, engine, session.id().clone());
        }
        Self::open(runtime, session, choice, build, approval).await
    }

    /// Resume a persisted session of the realm and make it live.
    ///
    /// # Errors
    ///
    /// [`TuiError::SessionRef`] when the session does not exist, otherwise as
    /// [`SessionHandle::create`].
    pub async fn resume(
        runtime: &Runtime,
        id: &SessionId,
        options: SessionOptions,
    ) -> Result<Self, TuiError> {
        let session = runtime
            .service
            .load_authoritative_session(id)
            .await?
            .ok_or_else(|| TuiError::SessionRef {
                reference: id.to_string(),
                reason: "session not found in this realm".to_string(),
            })?;
        let choice = session.session_metadata().map_or_else(
            || runtime.choice.clone(),
            |meta| ModelChoice {
                endpoint: String::new(),
                model: meta.model,
                provider: meta.provider,
                auth_binding: meta.auth_binding,
            },
        );
        // A model only an endpoint knows (switched to in an earlier run)
        // needs its profile again before the agent is rebuilt.
        runtime.ensure_profile(&choice, None).await?;
        let approval = options.approval.clone();
        let mut build = build_options(options, choice.provider, runtime);
        build.auth_binding.clone_from(&choice.auth_binding);
        build.resume_existing_session(session.clone());
        if let Some(engine) = &approval {
            admission::install(&mut build, engine, session.id().clone());
            engine.seed_transcript(&session);
        }
        Self::open(runtime, session, choice, build, approval).await
    }

    async fn open(
        runtime: &Runtime,
        session: Session,
        choice: ModelChoice,
        build: SessionBuildOptions,
        approval: Option<Arc<ApprovalEngine>>,
    ) -> Result<Self, TuiError> {
        let ModelChoice {
            model,
            provider,
            auth_binding,
            ..
        } = choice;
        let profiles = runtime.profiles();
        let threshold = build.auto_compact_threshold_override;
        let id = session.id().clone();
        if let Some(helpers) = &runtime.helpers {
            helpers.register_session(&id, &model, auth_binding.as_ref());
        }
        let request = CreateSessionRequest {
            injected_context: Vec::new(),
            model: model.clone(),
            prompt: ContentInput::Text(String::new()),
            system_prompt: SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };
        let admission = runtime.service.reserve_create_session_admission().await?;
        let service = Arc::clone(&runtime.service);
        let machine = Arc::clone(&runtime.machine);
        Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
            &runtime.service,
            &runtime.machine,
            session,
            request,
            admission,
            move |session_id, _attachment, actor_slot| {
                Box::new(
                    PersistentRuntimeExecutor::new(service, machine, session_id)
                        .with_publication_actor_slot(actor_slot),
                )
            },
        ))
        .await?;
        Ok(Self {
            service: Arc::clone(&runtime.service),
            machine: Arc::clone(&runtime.machine),
            id,
            model,
            provider,
            auth_binding,
            profiles,
            approval,
            helpers: runtime.helpers.clone(),
            threshold,
        })
    }

    /// The native auto-compaction threshold the session's agent was built
    /// with (`None`: upstream's model-aware default).
    #[must_use]
    pub fn compaction_threshold(&self) -> Option<NonZeroU64> {
        self.threshold
    }

    /// Session id.
    #[must_use]
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// The approval engine gating this session's tool calls, if any.
    #[must_use]
    pub fn approval(&self) -> Option<&Arc<ApprovalEngine>> {
        self.approval.as_ref()
    }

    /// Model the session runs on.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Provider of [`SessionHandle::model`].
    #[must_use]
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// The explicit auth binding the session resolves its client through
    /// (`None`: default resolution).
    #[must_use]
    pub fn auth_binding(&self) -> Option<&AuthBindingRef> {
        self.auth_binding.as_ref()
    }

    /// The runtime's profile generation when this session's agent was built:
    /// its model registry holds the profiles added up to then.
    #[must_use]
    pub fn profiles(&self) -> u64 {
        self.profiles
    }

    /// Switch the live session to `choice`'s model, provider and binding
    /// through the runtime's session-LLM reconfigure host. The identity is
    /// persisted with the session, and the next model call uses it. The
    /// target needs a capability profile ([`Runtime::ensure_profile`]).
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the runtime refuses the switch (no
    /// profile, a client that cannot be built, a session not ready).
    pub async fn switch_model(&mut self, choice: &ModelChoice) -> Result<(), TuiError> {
        let request = SessionLlmReconfigureRequest {
            model: Some(choice.model.clone()),
            provider: Some(choice.provider.as_str().to_string()),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(match &choice.auth_binding {
                Some(binding) => TurnMetadataOverride::Set(binding.clone()),
                None => TurnMetadataOverride::Clear,
            }),
        };
        let report = self
            .machine
            .reconfigure_session_llm_identity(&self.id, request)
            .await?;
        tracing::debug!(?report, "switched the session's model");
        self.model.clone_from(&choice.model);
        self.provider = choice.provider;
        self.auth_binding.clone_from(&choice.auth_binding);
        if let Some(helpers) = &self.helpers {
            helpers.register_session(&self.id, &self.model, self.auth_binding.as_ref());
        }
        Ok(())
    }

    /// Follow every event of the session, across runs, from now on.
    ///
    /// # Errors
    ///
    /// [`TuiError::EventStream`] when the session is not live.
    pub async fn events(&self) -> Result<EventStream, TuiError> {
        Ok(self.service.subscribe_session_events(&self.id).await?)
    }

    /// Start a turn: a `Queue` input, run behind any running turn.
    ///
    /// # Errors
    ///
    /// See [`SessionHandle::submit`].
    pub async fn start_turn(&self, text: &str) -> Result<Admission, TuiError> {
        self.submit(text, HandlingMode::Queue).await
    }

    /// Steer a message into the running turn: a durable `Steer` input,
    /// written at the turn's next model boundary as a user message (or run
    /// as one follow-up turn when no boundary opens in time).
    ///
    /// # Errors
    ///
    /// See [`SessionHandle::submit`].
    pub async fn steer(&self, text: &str) -> Result<Admission, TuiError> {
        self.submit(text, HandlingMode::Steer).await
    }

    /// Admit `text` as a user prompt with `handling_mode`.
    ///
    /// The prompt carries only its handling mode as turn metadata, which
    /// keeps a steer in the runtime's durable delivery class, and a caller
    /// idempotency key derived from its fresh input id. The text is recorded
    /// as the user's words in the classifier transcript first (the run can
    /// reach a tool call before admission returns) and forgotten again when
    /// the runtime refuses it.
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the runtime fails the admission and
    /// [`TuiError::InputRejected`] when it rejects the input.
    pub async fn submit(
        &self,
        text: &str,
        handling_mode: HandlingMode,
    ) -> Result<Admission, TuiError> {
        if let Some(engine) = &self.approval {
            engine.record_prompt(&self.id, text);
        }
        let admitted = self.admit(text, handling_mode).await;
        if admitted.is_err()
            && let Some(engine) = &self.approval
        {
            engine.forget_prompt(&self.id, text);
        }
        admitted
    }

    async fn admit(&self, text: &str, handling_mode: HandlingMode) -> Result<Admission, TuiError> {
        let mut prompt = PromptInput::new(
            text,
            Some(RuntimeTurnMetadata {
                handling_mode: Some(handling_mode),
                ..RuntimeTurnMetadata::default()
            }),
        );
        let key = format!("rkat-tui:{}", prompt.header.id);
        prompt.header.idempotency_key = Some(
            IdempotencyKey::caller(&key).map_err(|err| TuiError::InputRejected(err.to_string()))?,
        );
        let (outcome, completion) = self
            .machine
            .accept_input_with_completion(&self.id, Input::Prompt(prompt))
            .await?;
        let input_id = match outcome {
            AcceptOutcome::Accepted { input_id, .. } => input_id,
            AcceptOutcome::Deduplicated { existing_id, .. } => existing_id,
            AcceptOutcome::Rejected { reason } => {
                return Err(TuiError::InputRejected(format!("{reason:?}")));
            }
            other => {
                return Err(TuiError::InputRejected(format!(
                    "unexpected admission outcome: {other:?}"
                )));
            }
        };
        Ok(Admission {
            input_id,
            completion,
        })
    }

    /// Interrupt: cancel the session's running helpers, stop the current
    /// run, then cancel each of `outstanding` (oldest first) that has not
    /// reached its terminal.
    ///
    /// Stopping the run (rather than only interrupting it) abandons the
    /// steers that joined it instead of returning them to their lane for a
    /// follow-up turn. A steer still queued is abandoned by the cancel; one a
    /// follow-up run already picked up has that run interrupted.
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the runtime fails the stop or a cancel.
    pub async fn interrupt(&self, outstanding: &[InputId]) -> Result<(), TuiError> {
        if let Some(helpers) = &self.helpers {
            helpers.cancel_for(&self.id).await;
        }
        if let Some(run_id) = self.machine.current_run(&self.id).await {
            let receipt = self
                .machine
                .stop_run(&self.id, &run_id, INTERRUPT_REASON)
                .await?;
            tracing::debug!(?receipt, "stopped the current run");
        }
        for input_id in outstanding {
            self.machine
                .cancel_input_if_present(&self.id, input_id, INTERRUPT_REASON)
                .await?;
        }
        Ok(())
    }

    /// Cancel one input (abandoned while queued; its run interrupted when it
    /// already runs).
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the runtime fails the cancel.
    pub async fn cancel_input(&self, input_id: &InputId) -> Result<bool, TuiError> {
        Ok(self
            .machine
            .cancel_input_if_present(&self.id, input_id, INTERRUPT_REASON)
            .await?)
    }

    /// Append `text` to the live session as an ordinary durable System
    /// message (`SessionServiceControlExt::append_system_context`). The
    /// append is idempotent per `idempotency_key`; the model reads it from
    /// its next request on.
    ///
    /// # Errors
    ///
    /// [`TuiError::SessionControl`] when the service refuses the append.
    pub async fn append_system_context(
        &self,
        text: String,
        idempotency_key: String,
    ) -> Result<AppendSystemContextStatus, TuiError> {
        let mut request = AppendSystemContextRequest::from_text(text);
        request.source = Some(SYSTEM_CONTEXT_SOURCE.to_string());
        request.idempotency_key = Some(idempotency_key);
        Ok(self
            .service
            .append_system_context(&self.id, request)
            .await?
            .status)
    }

    /// The session's committed transcript and metadata.
    ///
    /// # Errors
    ///
    /// [`TuiError::Session`] when the stores cannot be read,
    /// [`TuiError::SessionRef`] when the session is gone.
    pub async fn read(&self) -> Result<Session, TuiError> {
        self.service
            .load_authoritative_session(&self.id)
            .await?
            .ok_or_else(|| TuiError::SessionRef {
                reference: self.id.to_string(),
                reason: "session not found in this realm".to_string(),
            })
    }

    /// Take the session off the runtime: its registration is unregistered
    /// until terminal (which joins its actor). The session stays persisted
    /// and can be resumed.
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the teardown fails.
    pub async fn retire(&self) -> Result<(), TuiError> {
        if let Some(registration) = self
            .machine
            .current_session_registration_witness(&self.id)
            .await
        {
            self.machine
                .unregister_session_registration_until_terminal_if_current(&registration)
                .await?;
        }
        Ok(())
    }
}

/// Build options shared by fresh and resumed sessions: the persona plus the
/// caller's instruction sections, the external tools, the runtime's MCP
/// servers, the compaction threshold, and (with the runtime's helper host)
/// the mob tools enabled explicitly, which a resumed session's metadata
/// alone never does.
fn build_options(
    options: SessionOptions,
    provider: Provider,
    runtime: &Runtime,
) -> SessionBuildOptions {
    let SessionOptions {
        external_tools,
        additional_instructions,
        auto_compact_threshold,
        ..
    } = options;
    let mut instructions = Vec::with_capacity(1 + additional_instructions.len());
    instructions.push(CODING_PERSONA.to_string());
    instructions.extend(additional_instructions);
    let mut build = SessionBuildOptions {
        provider: Some(provider),
        external_tools,
        additional_instructions: Some(instructions),
        mcp_servers: runtime.mcp_servers.clone(),
        auto_compact_threshold_override: auto_compact_threshold,
        ..SessionBuildOptions::default()
    };
    if runtime.helpers.is_some() {
        build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::Enable);
    }
    build
}
