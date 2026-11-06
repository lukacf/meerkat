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
//!   endpoint binding) through the runtime's session-LLM reconfigure host
//!   ([`SessionServiceRuntimeExt::reconfigure_session_llm_identity`]); the
//!   next model call uses it.
//! - The handle keeps no copy of the session's model, provider or auth
//!   binding: [`SessionHandle::identity`] reads them from the session's
//!   authoritative metadata.
//! - With [`SessionOptions::approval`], every build of the session (fresh
//!   or resumed) carries the approval gate as its `ToolDispatchAdmission`
//!   ([`crate::approval::admission::install`]); a resumed session's
//!   classifier transcript is seeded from its authoritative history once it
//!   is live, and every prompt or steer the runtime admits is recorded in it
//!   under its caller idempotency key.
//! - Every build carries the runtime's declarative MCP servers
//!   ([`crate::bootstrap::mcp`]), the native auto-compaction trigger the
//!   caller chose ([`SessionOptions::compaction`], see [`CompactionTrigger`])
//!   and a tool access policy that denies the builtins the TUI owns itself
//!   ([`TUI_OWNED_BUILTINS`]: model switching, file writes).
//! - A fresh session gets the persona and the caller's instruction
//!   sections; a resumed one keeps the instructions it was created with (its
//!   persisted build state), because the runtime does not re-render the
//!   system prompt of a resumed session (see [`build_options`]).
//! - With the runtime's helper host ([`crate::mobs`]) every build enables
//!   the mob tools (`delegate`, `mob_*`) explicitly, the session registers
//!   its model and binding for its helpers to inherit, and
//!   [`SessionHandle::interrupt`] cancels its running helpers.

pub mod resolve;

use std::sync::Arc;

use meerkat::surface::{
    PersistentRuntimeExecutor, materialize_session_with_reserved_admission_and_actor_slot,
};
use meerkat_core::comms::EventStream;
use meerkat_core::lifecycle::run_primitive::{RuntimeTurnMetadata, TurnMetadataOverride};
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextStatus, CreateSessionRequest,
    DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionServiceControlExt,
};
use meerkat_core::{
    AgentToolDispatcher, AuthBindingRef, ContentInput, HandlingMode, InputId, Provider, Session,
    SessionId, SystemPromptOverride, ToolCategoryOverride, ToolNameSet,
};
use meerkat_runtime::completion::CompletionHandle;
use meerkat_runtime::run_stop::RunStopReceipt;
use meerkat_runtime::{
    AcceptOutcome, CanonicalKeyTextError, InputAbandonReason, InputTerminalOutcome, IdempotencyKey, Input, MeerkatMachine, PromptInput,
    RejectReason, SessionLlmReconfigureRequest, SessionServiceRuntimeExt,
};

use crate::approval::{ApprovalEngine, InputKey, admission};
use crate::bootstrap::{CODING_PERSONA, ModelChoice, Runtime, TuiService};
use crate::error::TuiError;
use crate::mobs::Helpers;

pub use crate::ui::compaction::CompactionTrigger;
pub use resolve::{list_sessions, resolve_session_ref};

/// Source recorded with the system context the TUI appends (`/remember`).
const SYSTEM_CONTEXT_SOURCE: &str = "rkat-tui";

/// Reason recorded with every interrupt and cancellation the TUI requests.
const INTERRUPT_REASON: &str = "rkat-tui: interrupted by the user";

/// Builtins every TUI session is denied through its tool access policy: the
/// TUI owns model switching (`/model`, the picker) and file writes (its own
/// diff-reporting file tools, gated by the approval policy).
pub const TUI_OWNED_BUILTINS: [&str; 2] = ["brain_swap", "apply_patch"];

/// Mob tools every TUI session and helper is denied because their child
/// would run outside the approval gate: a `fork_off` child seats through
/// the engine's persisted-fork path, which never consults the TUI's spawn
/// customizer, so it would get no gate, no persona, no follower and no
/// curated environment. Denied until the engine lets a host shape fork
/// seats.
pub const UNGATED_SPAWN_TOOLS: [&str; 1] = ["fork_off"];

/// The model, provider and auth binding a session runs on, as its
/// authoritative metadata records them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    /// Model id.
    pub model: String,
    /// Provider whose client serves the model.
    pub provider: Provider,
    /// Explicit auth binding (`None`: default resolution).
    pub auth_binding: Option<AuthBindingRef>,
}

/// Options for [`SessionHandle::create`] and [`SessionHandle::resume`].
#[derive(Default, Clone)]
pub struct SessionOptions {
    /// Extra tools (file tools, notes) composed into the agent's dispatcher.
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Instruction sections appended after [`CODING_PERSONA`] in a fresh
    /// session. A resumed session keeps the sections it was created with.
    pub additional_instructions: Vec<String>,
    /// Model, provider and auth binding of a fresh session, else the
    /// runtime's start-up choice. A resumed session keeps its stored model
    /// and binding (switch with [`SessionHandle::switch_model`]).
    pub choice: Option<ModelChoice>,
    /// The approval engine whose gate every tool call of the session passes.
    /// `None` runs tools ungated (embedders and tests only).
    pub approval: Option<Arc<ApprovalEngine>>,
    /// The native auto-compaction trigger of the build (see
    /// [`crate::ui::compaction`]).
    pub compaction: CompactionTrigger,
}

/// The runtime refused an input.
#[derive(Debug, thiserror::Error)]
pub enum AdmissionError {
    /// The runtime rejected the input.
    #[error("the runtime refused the input: {0}")]
    Rejected(RejectReason),
    /// The caller idempotency key is not valid key text.
    #[error("invalid idempotency key: {0}")]
    Key(#[source] CanonicalKeyTextError),
    /// An admission outcome this version does not know (the runtime's
    /// outcome type is non-exhaustive).
    #[error("unexpected admission outcome: {0}")]
    Unexpected(String),
}

/// How an admitted input reaches its terminal.
#[derive(Debug)]
pub enum Completion {
    /// Await the handle.
    Pending(CompletionHandle),
    /// The runtime admitted the input and reported it terminal during
    /// admission without an outcome: what it came to is unknown.
    TerminalAtAdmission,
    /// Deduplicated against an input that is already terminal (the
    /// runtime's documented dedupe case): nothing to wait for and no result.
    AlreadyTerminal,
}

/// An input the runtime accepted.
#[derive(Debug)]
pub struct Admission {
    /// The accepted input (the existing one when deduplicated).
    pub input_id: InputId,
    /// The caller idempotency key the input was admitted under (the key the
    /// classifier transcript records its text by).
    pub key: String,
    /// How the input reaches its terminal.
    pub completion: Completion,
}

/// A live session.
#[derive(Clone)]
pub struct SessionHandle {
    service: Arc<TuiService>,
    machine: Arc<MeerkatMachine>,
    id: SessionId,
    /// The runtime's profile generation when the agent was built.
    profiles: u64,
    approval: Option<Arc<ApprovalEngine>>,
    helpers: Option<Arc<Helpers>>,
    /// The native auto-compaction trigger the agent was built with.
    compaction: CompactionTrigger,
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle")
            .field("id", &self.id)
            .field("compaction", &self.compaction)
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
        let instructions = fresh_instructions(&options.additional_instructions);
        let mut build = build_options(options, choice.provider, Some(instructions), runtime);
        build.auth_binding.clone_from(&choice.auth_binding);
        build.mint_session_with_id(session.id().clone());
        if let Some(engine) = &approval {
            admission::install(&mut build, engine, session.id().clone());
        }
        let handle = Self::open(runtime, session, &choice.model, build, approval).await?;
        if let Some(helpers) = &runtime.helpers {
            helpers.register_session(&handle.id, &choice.model, choice.auth_binding.as_ref());
        }
        Ok(handle)
    }

    /// Resume a persisted session of the realm and make it live.
    ///
    /// The session keeps its model, binding and the instructions it was
    /// created with: the runtime does not re-render a resumed session's
    /// system prompt, so its persisted build state (not
    /// `options.additional_instructions`) is passed on.
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
        let identity = identity_of(&session).unwrap_or_else(|| SessionIdentity {
            model: runtime.choice.model.clone(),
            provider: runtime.choice.provider,
            auth_binding: runtime.choice.auth_binding.clone(),
        });
        // A model only an endpoint knows (switched to in an earlier run)
        // needs its profile again before the agent is rebuilt.
        runtime
            .ensure_profile(identity.provider, &identity.model, None)
            .await?;
        let approval = options.approval.clone();
        let persisted = session
            .build_state()
            .and_then(|state| state.additional_instructions);
        let mut build = build_options(options, identity.provider, persisted, runtime);
        build.auth_binding.clone_from(&identity.auth_binding);
        build.resume_existing_session(session.clone());
        if let Some(engine) = &approval {
            admission::install(&mut build, engine, session.id().clone());
        }
        let handle = Self::open(runtime, session.clone(), &identity.model, build, approval).await?;
        // Only once the session is live: a failed open leaves no trace in
        // the classifier transcript or the helper registry.
        if let Some(engine) = &handle.approval {
            engine.seed_transcript(&session);
        }
        if let Some(helpers) = &runtime.helpers {
            helpers.register_session(&handle.id, &identity.model, identity.auth_binding.as_ref());
        }
        Ok(handle)
    }

    async fn open(
        runtime: &Runtime,
        session: Session,
        model: &str,
        build: SessionBuildOptions,
        approval: Option<Arc<ApprovalEngine>>,
    ) -> Result<Self, TuiError> {
        let profiles = runtime.profiles();
        let compaction = CompactionTrigger::from_override(build.auto_compact_threshold_override);
        let id = session.id().clone();
        let request = CreateSessionRequest {
            injected_context: Vec::new(),
            model: model.to_string(),
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
            profiles,
            approval,
            helpers: runtime.helpers.clone(),
            compaction,
        })
    }

    /// The native auto-compaction trigger the session's agent was built
    /// with.
    #[must_use]
    pub fn compaction(&self) -> CompactionTrigger {
        self.compaction
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

    /// The model, provider and auth binding the session's authoritative
    /// metadata records.
    ///
    /// # Errors
    ///
    /// As [`SessionHandle::read`]; [`TuiError::SessionRef`] when the session
    /// has no metadata.
    pub async fn identity(&self) -> Result<SessionIdentity, TuiError> {
        identity_of(&self.read().await?).ok_or_else(|| TuiError::SessionRef {
            reference: self.id.to_string(),
            reason: "the session has no metadata".to_string(),
        })
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
    pub async fn switch_model(&self, choice: &ModelChoice) -> Result<(), TuiError> {
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
        if let Some(helpers) = &self.helpers {
            helpers.register_session(&self.id, &choice.model, choice.auth_binding.as_ref());
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
    /// idempotency key derived from its fresh input id. Once the runtime
    /// admitted it, the text is recorded as the user's words in the
    /// classifier transcript under that key ([`Admission::key`]).
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the runtime fails the admission and
    /// [`TuiError::Admission`] when it rejects the input.
    pub async fn submit(
        &self,
        text: &str,
        handling_mode: HandlingMode,
    ) -> Result<Admission, TuiError> {
        let admission = self.admit(text, handling_mode).await?;
        if let Some(engine) = &self.approval {
            engine.record_user_input(&self.id, InputKey::new(admission.key.clone()), text);
        }
        Ok(admission)
    }

    /// The input admitted under `key` will never reach the model (it was
    /// cancelled or ended undelivered): its text leaves the classifier
    /// transcript.
    pub fn forget_input(&self, key: &str) {
        if let Some(engine) = &self.approval {
            engine.forget_user_input(&self.id, &InputKey::new(key));
        }
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
        prompt.header.idempotency_key =
            Some(IdempotencyKey::caller(&key).map_err(AdmissionError::Key)?);
        let (outcome, handle) = self
            .machine
            .accept_input_with_completion(&self.id, Input::Prompt(prompt))
            .await?;
        let (input_id, completion) = match (outcome, handle) {
            (AcceptOutcome::Rejected { reason }, _) => {
                return Err(AdmissionError::Rejected(reason).into());
            }
            (
                AcceptOutcome::Accepted { input_id, .. }
                | AcceptOutcome::Deduplicated {
                    existing_id: input_id,
                    ..
                },
                Some(handle),
            ) => (input_id, Completion::Pending(handle)),
            (AcceptOutcome::Accepted { input_id, .. }, None) => {
                (input_id, Completion::TerminalAtAdmission)
            }
            (AcceptOutcome::Deduplicated { existing_id, .. }, None) => {
                (existing_id, Completion::AlreadyTerminal)
            }
            #[allow(unreachable_patterns)]
            (other, _) => {
                return Err(AdmissionError::Unexpected(format!("{other:?}")).into());
            }
        };
        Ok(Admission {
            input_id,
            key,
            completion,
        })
    }

    /// Interrupt: cancel the session's running helpers, stop the current
    /// run, then cancel each of `outstanding` (oldest first) that has not
    /// reached its terminal. Returns the inputs the runtime reports this
    /// interrupt cancelled: the stop receipt's contributors whose committed
    /// terminal is `Abandoned { reason: Cancelled }`, and each input a
    /// cancel found still pending.
    ///
    /// Stopping the run (rather than only interrupting it) abandons the
    /// steers that joined it instead of returning them to their lane for a
    /// follow-up turn. A steer still queued is abandoned by the cancel; one a
    /// follow-up run already picked up has that run interrupted.
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the runtime fails the stop or a cancel.
    pub async fn interrupt(&self, outstanding: &[InputId]) -> Result<Vec<InputId>, TuiError> {
        if let Some(helpers) = &self.helpers {
            helpers.cancel_for(&self.id).await;
        }
        let mut cancelled = Vec::new();
        if let Some(run_id) = self.machine.current_run(&self.id).await {
            let receipt = self
                .machine
                .stop_run(&self.id, &run_id, INTERRUPT_REASON)
                .await?;
            tracing::debug!(?receipt, "stopped the current run");
            cancelled.extend(stopped_inputs(&receipt));
        }
        for input_id in outstanding {
            if self
                .machine
                .cancel_input_if_present(&self.id, input_id, INTERRUPT_REASON)
                .await?
            {
                cancelled.push(input_id.clone());
            }
        }
        Ok(cancelled)
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
    /// and can be resumed (a rebuild resumes it right away; see
    /// [`SessionHandle::close`] for leaving it).
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

    /// Leave the session (`/new`, `/resume`, quit): [`SessionHandle::retire`]
    /// it and drop its state from the approval engine (classifier
    /// transcript, allow cache, stop flag, helpers), also when the teardown
    /// fails.
    ///
    /// # Errors
    ///
    /// [`TuiError::Runtime`] when the teardown fails.
    pub async fn close(&self) -> Result<(), TuiError> {
        let retired = self.retire().await;
        if let Some(engine) = &self.approval {
            engine.forget_session(&self.id);
        }
        retired
    }
}

/// The inputs a stop receipt reports cancelled (committed terminal
/// `Abandoned { reason: Cancelled }`).
fn stopped_inputs(receipt: &RunStopReceipt) -> impl Iterator<Item = InputId> + '_ {
    let contributors = match receipt {
        RunStopReceipt::Stopped { contributors, .. } => contributors.as_slice(),
        _ => &[],
    };
    contributors
        .iter()
        .filter(|contributor| {
            matches!(
                contributor.terminal,
                Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled
                })
            )
        })
        .map(|contributor| contributor.input_id.clone())
}

/// The identity `session`'s metadata records.
fn identity_of(session: &Session) -> Option<SessionIdentity> {
    session.session_metadata().map(|meta| SessionIdentity {
        model: meta.model,
        provider: meta.provider,
        auth_binding: meta.auth_binding,
    })
}

/// The instruction sections of a fresh session: the persona, then the
/// caller's sections.
fn fresh_instructions(additional: &[String]) -> Vec<String> {
    let mut instructions = Vec::with_capacity(1 + additional.len());
    instructions.push(CODING_PERSONA.to_string());
    instructions.extend(additional.iter().cloned());
    instructions
}

/// The tool access policy of every TUI session: [`TUI_OWNED_BUILTINS`]
/// denied.
#[must_use]
pub fn tool_access_policy() -> ToolAccessPolicy {
    ToolAccessPolicy::DenyList(
        TUI_OWNED_BUILTINS
            .into_iter()
            .chain(UNGATED_SPAWN_TOOLS)
            .collect::<ToolNameSet>(),
    )
}

/// Build options shared by fresh and resumed sessions: `instructions`, the
/// external tools, the runtime's MCP servers, the compaction trigger, the
/// tool access policy, the confined shell's curated environment
/// ([`crate::bootstrap::ConfinementPlan::shell_env`]; `None` when the shell
/// runs unconfined and inherits rkat-tui's environment) and (with the
/// runtime's helper host) the mob tools enabled explicitly, which a resumed
/// session's metadata alone never does.
fn build_options(
    options: SessionOptions,
    provider: Provider,
    instructions: Option<Vec<String>>,
    runtime: &Runtime,
) -> SessionBuildOptions {
    let SessionOptions {
        external_tools,
        compaction,
        ..
    } = options;
    let mut build = SessionBuildOptions {
        provider: Some(provider),
        external_tools,
        additional_instructions: instructions,
        mcp_servers: runtime.mcp_servers.clone(),
        auto_compact_threshold_override: compaction.threshold_override(),
        tool_access_policy: Some(tool_access_policy()),
        shell_env: runtime.confinement.shell_env.clone(),
        ..SessionBuildOptions::default()
    };
    if runtime.helpers.is_some() {
        build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::Enable);
    }
    build
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tui_owned_builtins_are_denied() {
        let ToolAccessPolicy::DenyList(names) = tool_access_policy() else {
            unreachable!("a deny list");
        };
        for name in TUI_OWNED_BUILTINS.into_iter().chain(UNGATED_SPAWN_TOOLS) {
            assert!(names.0.iter().any(|n| n.as_str() == name), "{name}");
        }
        assert!(names.0.iter().any(|n| n.as_str() == "fork_off"));
    }

    #[test]
    fn a_fresh_session_gets_the_persona_once_then_the_sections() {
        let sections = fresh_instructions(&["a".to_string()]);
        assert_eq!(sections, vec![CODING_PERSONA.to_string(), "a".to_string()]);
    }
}
