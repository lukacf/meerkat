//! MeerkatMachine — session-scoped execution kernel.
//!
//! One of two kernels in the Meerkat two-kernel architecture:
//!
//! - **MeerkatMachine** (this module) owns session-scoped runtime state:
//!   input ingress, run lifecycle, completion waiters, async-ops registry,
//!   comms drain, and tool visibility publication. All mutations flow through
//!   one unified internal reducer, gated by TLA+-derived precondition guards.
//!
//! - **MobMachine** (`meerkat-mob`) owns mob-scoped orchestration: roster,
//!   flow frames, delegation, and inter-member wiring.
//!
//! MeerkatMachine lives in `meerkat-runtime` so `meerkat-session` does not
//! depend on runtime execution internals. When a session registers a
//! `CoreExecutor`, a background `RuntimeLoop` task is spawned. Input acceptance
//! queues through the driver; wake signals the loop; the loop dequeues, stages,
//! applies via `CoreExecutor`, and marks inputs consumed.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{OnceLock, Weak};

use futures::FutureExt as _;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::time_compat::Instant;
use meerkat_core::tool_scope::ToolScopeTurnOverlay;
use meerkat_core::types::SessionId;
use meerkat_core::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
use meerkat_core::{
    DeferredToolLoadAuthority, SessionToolVisibilityState, ToolFilter, ToolScopeApplyError,
    ToolScopeRevision, ToolScopeStageError, ToolVisibilityOwner, ToolVisibilityWitness,
};

use crate::accept::AcceptOutcome;
use crate::driver::ephemeral::EphemeralRuntimeDriver;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputStateSeed, InputTerminalOutcome,
};
use crate::meerkat_machine_types::{
    HydratedSessionLlmState, MeerkatAdmittedInputSnapshot, MeerkatArchiveSnapshot,
    MeerkatBindingSnapshot, MeerkatCompletionWaiterSnapshot, MeerkatCompletionWaitersSnapshot,
    MeerkatControlSnapshot, MeerkatCursorSnapshot, MeerkatDrainSnapshot, MeerkatDriverKind,
    MeerkatFormalStateProjection, MeerkatInputsSnapshot, MeerkatLedgerSnapshot,
    MeerkatMachineCommand, MeerkatMachineCommandError, MeerkatMachineCommandResult,
    MeerkatMachineRunFailure, MeerkatMachineSpineSnapshot, MeerkatOpsSnapshot,
    MemberResidencyExpectation, SessionLlmCapabilityDelta, SessionLlmCapabilitySurface,
    SessionLlmReconfigureHost, SessionLlmReconfigureReport, SessionLlmReconfigureRequest,
    SessionToolVisibilityDelta,
};
use crate::runtime_state::RuntimeState;
use crate::service_ext::SessionServiceRuntimeExt;
use crate::store::RuntimeStore;
use crate::tokio;
use crate::tokio::sync::{Mutex, RwLock, mpsc};
#[cfg(test)]
use crate::traits::RuntimeDriver;
use crate::traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport,
    RuntimeControlPlaneError, RuntimeDriverError,
};

/// Opaque proof minted only inside MeerkatMachine after AcceptWithCompletion
/// has returned a durable Accepted/Deduplicated result, or a typed validation
/// rejection. The shell can carry but cannot construct or alter this value.
pub(crate) struct DurablePeerIngressAdmissionReceipt {
    facts: meerkat_core::interaction::PeerIngressClaimCommitFacts,
    outcome: meerkat_core::interaction::PeerIngressClaimTerminalOutcome,
}

impl DurablePeerIngressAdmissionReceipt {
    fn new(
        facts: meerkat_core::interaction::PeerIngressClaimCommitFacts,
        outcome: meerkat_core::interaction::PeerIngressClaimTerminalOutcome,
    ) -> Self {
        Self { facts, outcome }
    }

    pub(crate) fn validate(
        &self,
        facts: meerkat_core::interaction::PeerIngressClaimCommitFacts,
    ) -> Option<meerkat_core::interaction::PeerIngressClaimTerminalOutcome> {
        (self.facts == facts).then(|| self.outcome.clone())
    }
}

pub(crate) enum PeerIngressAcceptFinalization {
    Finalized {
        receipt: Arc<dyn std::any::Any + Send + Sync>,
        completion: Option<crate::completion::CompletionHandle>,
    },
    MechanismError(RuntimeDriverError),
}

#[allow(clippy::expect_used)]
#[cfg(test)]
pub(crate) fn recover_projected_authority(
    state: dsl::MeerkatMachineState,
    context: &'static str,
) -> dsl::MeerkatMachineAuthority {
    dsl::MeerkatMachineAuthority::recover_from_state(state).expect(context)
}

struct ToolVisibilityOwnerGeneratedAuthorityBridgeToken;

static TOOL_VISIBILITY_OWNER_GENERATED_AUTHORITY_BRIDGE_TOKEN:
    ToolVisibilityOwnerGeneratedAuthorityBridgeToken =
    ToolVisibilityOwnerGeneratedAuthorityBridgeToken;

fn tool_visibility_owner_generated_authority_bridge_token()
-> &'static (dyn std::any::Any + Send + Sync) {
    &TOOL_VISIBILITY_OWNER_GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_tool_visibility_owner_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn tool_visibility_owner_generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<ToolVisibilityOwnerGeneratedAuthorityBridgeToken>()
}

fn generated_tool_visibility_owner(
    owner: Arc<dyn ToolVisibilityOwner>,
) -> Result<meerkat_core::GeneratedToolVisibilityOwner, String> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_core_runtime_generated_tool_visibility_owner_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn core_runtime_generated_tool_visibility_owner_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            owner: Arc<dyn ToolVisibilityOwner>,
        ) -> Result<meerkat_core::GeneratedToolVisibilityOwner, String>;
    }
    #[allow(unsafe_code)]
    unsafe {
        core_runtime_generated_tool_visibility_owner_build(
            tool_visibility_owner_generated_authority_bridge_token(),
            owner,
        )
    }
}

/// Shared generated authorities for a standalone facade session.
///
/// Standalone has no runtime loop, but turn recovery, model routing, and tool
/// visibility still describe one session machine. Keeping all three handles on
/// the same authority is required for sticky fallback: the turn handle admits
/// ErrorRecovery and the routing handle commits identity + visibility against
/// that exact state.
#[derive(Clone)]
pub struct StandaloneSessionRuntimeAuthorities {
    tool_visibility_owner: meerkat_core::GeneratedToolVisibilityOwner,
    turn_state: Arc<dyn meerkat_core::TurnStateHandle>,
    model_routing: Arc<dyn meerkat_core::handles::ModelRoutingHandle>,
    #[cfg(test)]
    model_routing_test: Arc<crate::handles::RuntimeModelRoutingHandle>,
}

impl StandaloneSessionRuntimeAuthorities {
    pub fn tool_visibility_owner(&self) -> &meerkat_core::GeneratedToolVisibilityOwner {
        &self.tool_visibility_owner
    }

    pub fn turn_state(&self) -> &Arc<dyn meerkat_core::TurnStateHandle> {
        &self.turn_state
    }

    pub fn model_routing(&self) -> &Arc<dyn meerkat_core::handles::ModelRoutingHandle> {
        &self.model_routing
    }

    #[cfg(test)]
    pub(crate) fn commit_sticky_model_fallback_for_test(
        &self,
        previous_identity: &meerkat_core::SessionLlmIdentity,
        target_identity: &meerkat_core::SessionLlmIdentity,
        target_profile: &meerkat_core::ModelProfileWitness,
        visibility_plan: &meerkat_core::handles::StickyModelFallbackVisibilityPlan,
        retry_attempt: u32,
    ) -> Result<(), meerkat_core::handles::DslTransitionError> {
        self.model_routing_test
            .commit_sticky_model_fallback_for_test(
                previous_identity,
                target_identity,
                target_profile,
                visibility_plan,
                retry_attempt,
            )
    }
}

/// Build the shared generated authority bundle for a standalone session.
///
/// Standalone sessions do not have a runtime loop, but durable tool visibility
/// and sticky model fallback are still machine facts. This bundle gives those
/// sessions the same single-authority path used by runtime-backed sessions.
pub fn standalone_session_runtime_authorities(
    session_id: &SessionId,
    current_identity: &meerkat_core::SessionLlmIdentity,
    model_profile: Option<&meerkat_core::model_profile::ModelProfile>,
    capability_base_filter: &ToolFilter,
) -> Result<StandaloneSessionRuntimeAuthorities, String> {
    let mut authority =
        dsl_authority::new_registered_authority_without_runtime_entry(session_id)
            .map_err(|err| dsl_authority::map_error(err, "standalone visibility authority"))?;
    let (current_capability_surface, current_capability_surface_status) = match model_profile {
        Some(profile) => (
            Some(dsl::SessionLlmCapabilitySurface {
                supports_temperature: profile.supports_temperature,
                supports_thinking: profile.supports_thinking,
                supports_reasoning: profile.supports_reasoning,
                inline_video: profile.inline_video,
                vision: profile.vision,
                image_input: profile.image_input,
                image_tool_results: profile.image_tool_results,
                supports_web_search: profile.supports_web_search,
                supports_mid_conversation_system_messages: profile
                    .supports_mid_conversation_system_messages,
                image_generation: profile.image_generation,
                realtime: profile.realtime,
                call_timeout_secs: profile.call_timeout_secs,
            }),
            dsl::SessionLlmCapabilitySurfaceStatus::Resolved,
        ),
        None => (None, dsl::SessionLlmCapabilitySurfaceStatus::Unresolved),
    };
    dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::HydrateSessionLlmState {
            current_identity: dsl::SessionLlmIdentity::from_domain(current_identity),
            current_capability_surface,
            current_capability_surface_status,
            current_capability_base_filter: dsl::ToolFilter::from_domain(capability_base_filter),
        },
    )
    .map_err(|err| dsl_authority::map_error(err, "standalone visibility hydration"))?;
    dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::SetModelRoutingBaseline {
            baseline_model: current_identity.model.clone(),
            realtime_capable: model_profile.is_some_and(|profile| profile.realtime),
        },
    )
    .map_err(|err| dsl_authority::map_error(err, "standalone model routing baseline"))?;
    let authority = Arc::new(std::sync::Mutex::new(authority));
    let owner = Arc::new(MachineToolVisibilityOwner::new());
    owner.bind_dsl_authority(Arc::clone(&authority));
    let shared_handle_authority = Arc::new(crate::handles::HandleDslAuthority::from_shared(
        Arc::clone(&authority),
    ));
    let tool_visibility_owner =
        generated_tool_visibility_owner(Arc::clone(&owner) as Arc<dyn ToolVisibilityOwner>)?;
    let turn_state = Arc::new(crate::handles::RuntimeTurnStateHandle::standalone(
        Arc::clone(&shared_handle_authority),
        session_id.clone(),
    )) as Arc<dyn meerkat_core::TurnStateHandle>;
    let runtime_model_routing = Arc::new(
        crate::handles::RuntimeModelRoutingHandle::new_with_visibility_owner(
            shared_handle_authority,
            owner,
        ),
    );
    let model_routing =
        Arc::clone(&runtime_model_routing) as Arc<dyn meerkat_core::handles::ModelRoutingHandle>;
    Ok(StandaloneSessionRuntimeAuthorities {
        tool_visibility_owner,
        turn_state,
        model_routing,
        #[cfg(test)]
        model_routing_test: runtime_model_routing,
    })
}

/// Build only the standalone visibility projection for tool-only hosts.
/// AgentFactory uses [`standalone_session_runtime_authorities`] so turn,
/// routing, and visibility never split across private authorities.
pub fn standalone_tool_visibility_owner(
    session_id: &SessionId,
    current_identity: &meerkat_core::SessionLlmIdentity,
    model_profile: Option<&meerkat_core::model_profile::ModelProfile>,
    capability_base_filter: &ToolFilter,
) -> Result<meerkat_core::GeneratedToolVisibilityOwner, String> {
    standalone_session_runtime_authorities(
        session_id,
        current_identity,
        model_profile,
        capability_base_filter,
    )
    .map(|authorities| authorities.tool_visibility_owner)
}

/// Error type for [`MeerkatMachine::prepare_bindings`].
///
/// Non-exhaustive: a later release can add a refusal without breaking a
/// downstream match, which needs a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeBindingsError {
    /// Session was not found after registration (should not happen in practice).
    #[error("session {0} not found in runtime adapter after registration")]
    SessionNotFound(SessionId),
    /// Machine-owned binding preparation failed before bindings were published.
    #[error("failed to prepare runtime bindings for session {0}: {1}")]
    PrepareFailed(SessionId, String),
    #[error("materialization registration for session {0} is no longer current")]
    RegistrationNotCurrent(SessionId),
    #[error("materialization registration for session {0} has another owner")]
    RegistrationOwned(SessionId),
    /// This runtime owner may not host the session: another runtime owner on
    /// the realm hosts it ([`meerkat_core::SessionError::ServedElsewhere`]),
    /// or its hosting claim cannot be taken
    /// ([`meerkat_core::SessionError::HostingUnavailable`]). The typed
    /// session refusal is carried as is, so a caller can tell it from a
    /// preparation failure and keep its wire form.
    #[error("runtime bindings for session {0} refused by session hosting: {1}")]
    Hosting(SessionId, meerkat_core::SessionError),
}

impl RuntimeBindingsError {
    /// The bindings error for a driver error a preparation stopped at: a
    /// hosting refusal keeps its typed session error; any other error stays a
    /// preparation failure with its message.
    pub(crate) fn from_preparation_error(
        session_id: SessionId,
        error: &RuntimeDriverError,
    ) -> Self {
        match error.hosting_session_error() {
            Some(cause) => Self::Hosting(session_id, cause),
            None => Self::PrepareFailed(session_id, error.to_string()),
        }
    }

    /// Whether preparation was refused before this call reserved a
    /// materialization claim, inserted a registration, or installed handles.
    ///
    /// Both registration verdicts are returned only from checks that run
    /// before any mutation, so a caller may treat such a failed attempt as
    /// proven no-effect rather than as an unproven cleanup obligation. A
    /// hosting refusal is not claimed as no-effect: a caller keeps its
    /// cleanup open, as for any other preparation failure.
    #[must_use]
    pub fn rejected_before_effect(&self) -> bool {
        matches!(
            self,
            Self::RegistrationOwned(_) | Self::RegistrationNotCurrent(_)
        )
    }
}

/// Generated public projection for an input-state seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPublicStateProjection {
    pub lifecycle_state: dsl::InputPublicLifecycleState,
    pub terminal_outcome: Option<dsl::InputPublicTerminalOutcome>,
}

/// Runtime lifecycle/admission facts emitted by generated MeerkatMachine
/// authority for a public runtime-state projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLifecycleFacts {
    pub terminality: dsl::RuntimeLifecycleTerminality,
    pub input_admission: dsl::RuntimeInputAdmission,
    pub queue_admission: dsl::RuntimeQueueAdmission,
    pub prepare_admission: dsl::RuntimePrepareAdmission,
    pub ingress_admission: dsl::RuntimeIngressAdmission,
}

impl RuntimeLifecycleFacts {
    #[must_use]
    pub fn can_accept_input(self) -> bool {
        self.input_admission == dsl::RuntimeInputAdmission::AcceptsInput
    }

    #[must_use]
    pub fn can_process_queue(self) -> bool {
        self.queue_admission == dsl::RuntimeQueueAdmission::ProcessesQueue
    }

    #[must_use]
    pub fn can_prepare_run(self) -> bool {
        self.prepare_admission == dsl::RuntimePrepareAdmission::Ready
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        self.terminality == dsl::RuntimeLifecycleTerminality::Terminal
    }
}

/// Runtime-loop queue-drain admission feedback emitted by generated
/// MeerkatMachine authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLoopQueueAdmissionPlan {
    pub queue_admission: dsl::RuntimeQueueAdmission,
    pub run_binding: dsl::RuntimeLoopRunBinding,
}

impl RuntimeLoopQueueAdmissionPlan {
    #[must_use]
    pub fn can_process_queue(self) -> bool {
        self.queue_admission == dsl::RuntimeQueueAdmission::ProcessesQueue
    }

    #[must_use]
    pub fn uses_prebound_run(self) -> bool {
        self.run_binding == dsl::RuntimeLoopRunBinding::UsePrebound
    }
}

/// Classify runtime lifecycle/admission facts through generated
/// MeerkatMachine authority. Callers provide only the observed state variant;
/// all behavior-affecting facts come back as generated typed feedback.
pub fn classify_runtime_lifecycle_state(
    state: RuntimeState,
) -> Result<RuntimeLifecycleFacts, String> {
    let observed_state = dsl_authority::observed_runtime_lifecycle_state(state);
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ClassifyRuntimeLifecycleState {
            state: observed_state,
        },
    )
    .map_err(|err| {
        format!("MeerkatMachine rejected runtime lifecycle classification for {state}: {err}")
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::RuntimeLifecycleStateClassified {
                state,
                terminality,
                input_admission,
                queue_admission,
                prepare_admission,
                ingress_admission,
            } if state == observed_state => Some(RuntimeLifecycleFacts {
                terminality,
                input_admission,
                queue_admission,
                prepare_admission,
                ingress_admission,
            }),
            _ => None,
        })
        .ok_or_else(|| {
            format!("MeerkatMachine emitted no runtime lifecycle classification for {state}")
        })
}

/// Classify the store-visible durable runtime lifecycle state through
/// generated MeerkatMachine authority. The caller supplies only the live
/// observed state; generated feedback decides the recovery projection.
pub fn classify_runtime_lifecycle_durable_state(
    state: RuntimeState,
) -> Result<RuntimeState, String> {
    classify_runtime_lifecycle_durable_state_with_pre_run_phase(state, None)
}

/// Classify the durable lifecycle while retaining the live run's coarse
/// pre-run phase. Running itself is process-local, but a run admitted from a
/// retired runtime must remain durably retired after cold recovery.
pub(crate) fn classify_runtime_lifecycle_durable_state_with_pre_run_phase(
    state: RuntimeState,
    pre_run_phase: Option<RuntimeState>,
) -> Result<RuntimeState, String> {
    let observed_state = dsl_authority::observed_runtime_lifecycle_state(state);
    let pre_run_phase = pre_run_phase.and_then(dsl_authority::pre_run_phase_from_runtime_state);
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ClassifyRuntimeLifecycleDurability {
            state: observed_state,
            pre_run_phase,
        },
    )
    .map_err(|err| {
        format!(
            "MeerkatMachine rejected runtime lifecycle durability classification for {state}: {err}"
        )
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::RuntimeLifecycleDurabilityClassified {
                state,
                durable_state,
            } if state == observed_state => Some(
                dsl_authority::runtime_state_from_observed_lifecycle_state(durable_state),
            ),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "MeerkatMachine emitted no runtime lifecycle durability classification for {state}"
            )
        })
}

/// Classify runtime-loop queue admission through generated MeerkatMachine
/// authority. The caller provides the observed runtime state and the structural
/// fact that a current run id is bound; generated feedback decides whether the
/// queue may drain and whether that bound run id must be reused.
pub fn classify_runtime_loop_queue_admission(
    state: RuntimeState,
    current_run_bound: bool,
) -> Result<RuntimeLoopQueueAdmissionPlan, String> {
    let observed_state = dsl_authority::observed_runtime_lifecycle_state(state);
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ClassifyRuntimeLoopQueueAdmission {
            state: observed_state,
            current_run_bound,
        },
    )
    .map_err(|err| {
        format!(
            "MeerkatMachine rejected runtime-loop queue admission for {state} with current_run_bound={current_run_bound}: {err}"
        )
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::RuntimeLoopQueueAdmissionClassified {
                state,
                current_run_bound: observed_current_run_bound,
                queue_admission,
                run_binding,
            } if state == observed_state && observed_current_run_bound == current_run_bound => {
                Some(RuntimeLoopQueueAdmissionPlan {
                    queue_admission,
                    run_binding,
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "MeerkatMachine emitted no runtime-loop queue admission for {state} with current_run_bound={current_run_bound}"
            )
        })
}

/// Machine-owned arbitration verdict between the live DSL lifecycle phase and
/// the durable control projection, emitted by generated MeerkatMachine
/// authority. `publish_control` is the terminal-precedence decision (the
/// published control projection supersedes the live DSL phase);
/// `selected_raw_phase` is the chosen phase without the visibility rewrite;
/// `visible_phase` is the externally-visible phase after the
/// Running+pre_run(Retired)->Retired rewrite. The shell mirrors all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleRuntimePhasePlan {
    pub publish_control: bool,
    pub selected_raw_phase: RuntimeState,
    pub visible_phase: RuntimeState,
}

/// Resolve the authoritative/visible runtime phase through generated
/// MeerkatMachine authority. The shell feeds only the five pure
/// [`RuntimeState`] observations it already holds; the machine owns BOTH the
/// terminal-precedence `publish_control` policy AND the
/// Running+pre_run(Retired)->Retired visibility rewrite. The shell mirrors the
/// emitted verdict and re-derives nothing, failing closed if no verdict is
/// emitted.
pub fn resolve_visible_runtime_phase(
    dsl_phase: RuntimeState,
    dsl_pre_run_phase: Option<RuntimeState>,
    control_phase: RuntimeState,
    control_pre_run_phase: Option<RuntimeState>,
    has_runtime_persistence: bool,
) -> Result<VisibleRuntimePhasePlan, String> {
    let observed_dsl = dsl_authority::observed_runtime_lifecycle_state(dsl_phase);
    let observed_control = dsl_authority::observed_runtime_lifecycle_state(control_phase);
    let observed_dsl_pre_run =
        dsl_pre_run_phase.map(dsl_authority::observed_runtime_lifecycle_state);
    let observed_control_pre_run =
        control_pre_run_phase.map(dsl_authority::observed_runtime_lifecycle_state);
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ResolveVisibleRuntimePhase {
            dsl_phase: observed_dsl,
            dsl_pre_run_phase: observed_dsl_pre_run,
            control_phase: observed_control,
            control_pre_run_phase: observed_control_pre_run,
            has_runtime_persistence,
        },
    )
    .map_err(|err| {
        format!(
            "MeerkatMachine rejected visible runtime phase resolution \
             (dsl={dsl_phase}, control={control_phase}, persistence={has_runtime_persistence}): {err}"
        )
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::VisibleRuntimePhaseResolved {
                publish_control,
                selected_raw_phase,
                visible_phase,
            } => Some(VisibleRuntimePhasePlan {
                publish_control,
                selected_raw_phase: dsl_authority::runtime_state_from_observed_lifecycle_state(
                    selected_raw_phase,
                ),
                visible_phase: dsl_authority::runtime_state_from_observed_lifecycle_state(
                    visible_phase,
                ),
            }),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "MeerkatMachine emitted no visible runtime phase resolution \
                 (dsl={dsl_phase}, control={control_phase}, persistence={has_runtime_persistence})"
            )
        })
}

/// Resolve the public lifecycle class for a machine-derived input phase
/// through generated MeerkatMachine authority.
pub fn resolve_input_public_lifecycle_projection(
    input_id: &InputId,
    phase: InputLifecycleState,
) -> Result<dsl::InputPublicLifecycleState, String> {
    let input_key = input_id.to_string();
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ResolveInputPublicLifecycle {
            input_id: input_key.clone(),
            phase: observed_input_phase(phase),
        },
    )
    .map_err(|err| {
        format!("MeerkatMachine rejected public lifecycle projection for '{input_id}': {err}")
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::InputPublicLifecycleResolved { input_id, phase }
                if input_id == input_key =>
            {
                Some(phase)
            }
            _ => None,
        })
        .ok_or_else(|| {
            format!("MeerkatMachine emitted no public lifecycle projection for '{input_id}'")
        })
}

/// Resolve public lifecycle and terminal result classes for a machine-derived
/// input-state seed through generated MeerkatMachine authority.
pub fn resolve_input_public_state_projection(
    input_id: &InputId,
    seed: &InputStateSeed,
) -> Result<InputPublicStateProjection, String> {
    let lifecycle_state = resolve_input_public_lifecycle_projection(input_id, seed.phase)?;
    let terminal_outcome = resolve_input_public_terminal_projection(input_id, seed)?;
    Ok(InputPublicStateProjection {
        lifecycle_state,
        terminal_outcome,
    })
}

pub(crate) fn input_seed_behavioral_terminality_via_authority(
    input_id: &InputId,
    seed: &InputStateSeed,
) -> Result<bool, String> {
    classify_input_behavioral_terminality(input_id, seed.phase, seed.terminal_outcome.as_ref())
}

pub(crate) fn input_phase_behavioral_terminality_via_authority(
    input_id: &InputId,
    phase: InputLifecycleState,
    terminal_outcome: Option<InputTerminalOutcome>,
) -> Result<bool, String> {
    classify_input_behavioral_terminality(input_id, phase, terminal_outcome.as_ref())
}

/// Authorize DSL-owned input-state seed facts before they are written to a
/// runtime store.
pub(crate) fn authorize_stored_input_state_seed(
    input_id: &InputId,
    seed: &InputStateSeed,
) -> Result<(), String> {
    let input_key = input_id.to_string();
    let (terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count) =
        input_seed_terminal_parts(seed)?;
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::AuthorizeStoredInputStateSeed {
            input_id: input_key.clone(),
            phase: observed_input_phase(seed.phase),
            terminal_kind,
            superseded_by,
            aggregate_id,
            abandon_reason,
            abandon_attempt_count,
            attempt_count: u64::from(seed.attempt_count),
            run_id: seed.last_run_id.as_ref().map(dsl::RunId::from_domain),
            boundary_sequence: seed.last_boundary_sequence,
            admission_sequence: seed.admission_sequence,
            recovery_lane: seed.recovery_lane.map(dsl::InputLane::from),
        },
    )
    .map_err(|err| {
        format!("MeerkatMachine rejected stored input-state seed for '{input_id}': {err}")
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::StoredInputStateSeedAuthorized { input_id }
                if input_id == input_key =>
            {
                Some(())
            }
            _ => None,
        })
        .ok_or_else(|| {
            format!("MeerkatMachine emitted no stored input-state seed authority for '{input_id}'")
        })
}

fn classify_input_behavioral_terminality(
    input_id: &InputId,
    phase: InputLifecycleState,
    terminal_outcome: Option<&InputTerminalOutcome>,
) -> Result<bool, String> {
    let input_key = input_id.to_string();
    let (terminal_kind, abandon_reason) = input_terminality_parts(terminal_outcome);
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ClassifyInputTerminality {
            input_id: input_key.clone(),
            phase: observed_input_phase(phase),
            terminal_kind,
            abandon_reason,
        },
    )
    .map_err(|err| {
        format!("MeerkatMachine rejected behavioral input terminality for '{input_id}': {err}")
    })?;

    let mut terminality = None;
    for effect in transition.into_effects() {
        match effect {
            dsl::MeerkatMachineEffect::InputBehavioralTerminalityResolved {
                input_id,
                terminal,
            } if input_id == input_key => terminality = Some(terminal),
            other => {
                return Err(format!(
                    "MeerkatMachine emitted unexpected behavioral input terminality effect for '{input_id}': {other:?}"
                ));
            }
        }
    }
    terminality.ok_or_else(|| {
        format!("MeerkatMachine emitted no behavioral input terminality for '{input_id}'")
    })
}

fn resolve_input_public_terminal_projection(
    input_id: &InputId,
    seed: &InputStateSeed,
) -> Result<Option<dsl::InputPublicTerminalOutcome>, String> {
    resolve_input_public_terminal_class(input_id, seed.phase, seed.terminal_outcome.as_ref())
}

/// Resolve the public terminal result class of one committed input terminal
/// through generated MeerkatMachine authority. The lifecycle phase is the one
/// the terminal kind implies; surfaces use this to project a committed
/// terminal (for example a [`crate::RunStopContributor`]) without a seed.
pub fn resolve_input_public_terminal_outcome_projection(
    input_id: &InputId,
    terminal: &InputTerminalOutcome,
) -> Result<Option<dsl::InputPublicTerminalOutcome>, String> {
    let phase = match terminal {
        InputTerminalOutcome::Consumed => InputLifecycleState::Consumed,
        InputTerminalOutcome::Superseded { .. } => InputLifecycleState::Superseded,
        InputTerminalOutcome::Coalesced { .. } => InputLifecycleState::Coalesced,
        InputTerminalOutcome::Abandoned { .. } => InputLifecycleState::Abandoned,
    };
    resolve_input_public_terminal_class(input_id, phase, Some(terminal))
}

fn resolve_input_public_terminal_class(
    input_id: &InputId,
    phase: InputLifecycleState,
    terminal: Option<&InputTerminalOutcome>,
) -> Result<Option<dsl::InputPublicTerminalOutcome>, String> {
    let input_key = input_id.to_string();
    let (terminal_kind, abandon_reason) = input_terminality_parts(terminal);
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ResolveInputPublicTerminalOutcome {
            input_id: input_key.clone(),
            phase: observed_input_phase(phase),
            terminal_kind,
            abandon_reason,
        },
    )
    .map_err(|err| {
        format!("MeerkatMachine rejected public terminal projection for '{input_id}': {err}")
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::InputPublicTerminalOutcomeResolved {
                input_id,
                terminal_outcome,
            } if input_id == input_key => Some(terminal_outcome),
            _ => None,
        })
        .ok_or_else(|| {
            format!("MeerkatMachine emitted no public terminal projection for '{input_id}'")
        })
}

fn projection_authority() -> dsl::MeerkatMachineAuthority {
    dsl_authority::new_initialized_authority("projection authority must initialize")
}

#[cfg(feature = "live")]
fn live_unbound_rejection_authority() -> crate::driver::ephemeral::SharedIngressDslAuthority {
    Arc::new(std::sync::Mutex::new(
        dsl_authority::new_initialized_authority(
            "live unbound rejection authority must initialize",
        ),
    ))
}

fn observed_input_phase(phase: InputLifecycleState) -> dsl::RecoveredInputObservedPhase {
    match phase {
        InputLifecycleState::Accepted => dsl::RecoveredInputObservedPhase::Accepted,
        InputLifecycleState::Queued => dsl::RecoveredInputObservedPhase::Queued,
        InputLifecycleState::Staged => dsl::RecoveredInputObservedPhase::Staged,
        InputLifecycleState::Applied => dsl::RecoveredInputObservedPhase::Applied,
        InputLifecycleState::AppliedPendingConsumption => {
            dsl::RecoveredInputObservedPhase::AppliedPendingConsumption
        }
        InputLifecycleState::Consumed => dsl::RecoveredInputObservedPhase::Consumed,
        InputLifecycleState::Superseded => dsl::RecoveredInputObservedPhase::Superseded,
        InputLifecycleState::Coalesced => dsl::RecoveredInputObservedPhase::Coalesced,
        InputLifecycleState::Abandoned => dsl::RecoveredInputObservedPhase::Abandoned,
    }
}

type InputSeedTerminalParts = (
    Option<dsl::InputTerminalKind>,
    Option<String>,
    Option<String>,
    Option<dsl::InputAbandonReason>,
    u64,
);

fn input_seed_terminal_parts(seed: &InputStateSeed) -> Result<InputSeedTerminalParts, String> {
    match seed.terminal_outcome.as_ref() {
        None => Ok((None, None, None, None, 0)),
        Some(InputTerminalOutcome::Consumed) => {
            Ok((Some(dsl::InputTerminalKind::Consumed), None, None, None, 0))
        }
        Some(InputTerminalOutcome::Superseded { superseded_by }) => Ok((
            Some(dsl::InputTerminalKind::Superseded),
            Some(superseded_by.to_string()),
            None,
            None,
            0,
        )),
        Some(InputTerminalOutcome::Coalesced { aggregate_id }) => Ok((
            Some(dsl::InputTerminalKind::Coalesced),
            None,
            Some(aggregate_id.to_string()),
            None,
            0,
        )),
        Some(InputTerminalOutcome::Abandoned { reason }) => {
            let abandon_attempt_count = match reason {
                InputAbandonReason::MaxAttemptsExhausted { attempts } => u64::from(*attempts),
                _ => u64::from(seed.attempt_count),
            };
            Ok((
                Some(dsl::InputTerminalKind::Abandoned),
                None,
                None,
                input_terminality_parts(seed.terminal_outcome.as_ref()).1,
                abandon_attempt_count,
            ))
        }
    }
}

fn input_terminality_parts(
    outcome: Option<&InputTerminalOutcome>,
) -> (
    Option<dsl::InputTerminalKind>,
    Option<dsl::InputAbandonReason>,
) {
    match outcome {
        None => (None, None),
        Some(InputTerminalOutcome::Consumed) => (Some(dsl::InputTerminalKind::Consumed), None),
        Some(InputTerminalOutcome::Superseded { .. }) => {
            (Some(dsl::InputTerminalKind::Superseded), None)
        }
        Some(InputTerminalOutcome::Coalesced { .. }) => {
            (Some(dsl::InputTerminalKind::Coalesced), None)
        }
        Some(InputTerminalOutcome::Abandoned { reason }) => (
            Some(dsl::InputTerminalKind::Abandoned),
            Some(match reason {
                InputAbandonReason::Retired => dsl::InputAbandonReason::Retired,
                InputAbandonReason::Reset => dsl::InputAbandonReason::Reset,
                InputAbandonReason::Stopped => dsl::InputAbandonReason::Stopped,
                InputAbandonReason::Destroyed => dsl::InputAbandonReason::Destroyed,
                InputAbandonReason::Cancelled => dsl::InputAbandonReason::Cancelled,
                InputAbandonReason::MaxAttemptsExhausted { .. } => {
                    dsl::InputAbandonReason::MaxAttemptsExhausted
                }
                InputAbandonReason::NeverExecuted => dsl::InputAbandonReason::NeverExecuted,
                InputAbandonReason::ToolProcessInterrupted => {
                    dsl::InputAbandonReason::ToolProcessInterrupted
                }
                InputAbandonReason::RequeueExpired => dsl::InputAbandonReason::RequeueExpired,
                InputAbandonReason::ResumeRefused => dsl::InputAbandonReason::ResumeRefused,
            }),
        ),
    }
}

#[derive(Debug, Default)]
struct UnavailableBlobStore;

impl UnavailableBlobStore {
    fn error() -> BlobStoreError {
        BlobStoreError::Unsupported(
            "persistent runtime constructed without blob store; blob-backed inputs require a BlobStore"
                .to_string(),
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct PersistentAuthAuthorityBundle {
    store: StdMutex<Weak<dyn RuntimeStore>>,
    auth_lease: Arc<crate::handles::RuntimeAuthLeaseHandle>,
    oauth_flows: Arc<crate::handles::RuntimeOAuthFlowHandle>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PersistentAuthAuthorityKey {
    Durable(String),
    Process(usize),
}

/// One store key's provider-auth authority bundle. Creating the bundle
/// rehydrates the store's OAuth flows (a store read, and a write back when
/// flows were persisted), so it runs under this slot's own lock: callers for
/// one key wait for that key's bundle, callers for other keys never do.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct PersistentAuthAuthoritySlot {
    bundle: StdMutex<Option<Arc<PersistentAuthAuthorityBundle>>>,
}

/// Every store key's slot. The map's lock covers only the slot lookup, and
/// nothing takes it while holding a slot's lock.
#[cfg(not(target_arch = "wasm32"))]
static PERSISTENT_AUTH_AUTHORITIES: OnceLock<
    StdMutex<HashMap<PersistentAuthAuthorityKey, Arc<PersistentAuthAuthoritySlot>>>,
> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn runtime_store_identity(store: &Arc<dyn RuntimeStore>) -> PersistentAuthAuthorityKey {
    store
        .auth_authority_key()
        .map(PersistentAuthAuthorityKey::Durable)
        .unwrap_or_else(|| {
            PersistentAuthAuthorityKey::Process(Arc::as_ptr(store).cast::<()>() as usize)
        })
}

fn runtime_stores_share_authority(a: &Arc<dyn RuntimeStore>, b: &Arc<dyn RuntimeStore>) -> bool {
    match (a.auth_authority_key(), b.auth_authority_key()) {
        (Some(a), Some(b)) => a == b,
        _ => Arc::ptr_eq(a, b),
    }
}

fn generated_runtime_auth_lease_handle(
    handle: Arc<crate::handles::RuntimeAuthLeaseHandle>,
) -> meerkat_core::handles::GeneratedAuthLeaseHandle {
    #[allow(clippy::expect_used)]
    crate::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(handle)
        .expect("runtime AuthLeaseHandle must be certified by generated AuthMachine authority")
}

/// Runtime-certified provider-auth authority bundle.
///
/// The fields are private so native auth surfaces cannot accidentally pair an
/// OAuth flow authority from one MeerkatMachine with credential lifecycle
/// authority from another.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct ProviderAuthRuntimeAuthority {
    auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
    oauth_flows: Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ProviderAuthRuntimeAuthority {
    pub fn generated_auth_lease_handle(&self) -> meerkat_core::handles::GeneratedAuthLeaseHandle {
        self.auth_lease.clone()
    }

    pub fn oauth_flow_authority(
        &self,
    ) -> Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority> {
        Arc::clone(&self.oauth_flows)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn persistent_auth_authorities(
    store: &Arc<dyn RuntimeStore>,
) -> Arc<PersistentAuthAuthorityBundle> {
    persistent_auth_authorities_for_key(runtime_store_identity(store), store)
}

#[cfg(not(target_arch = "wasm32"))]
fn persistent_auth_authority_slot(
    key: PersistentAuthAuthorityKey,
) -> Arc<PersistentAuthAuthoritySlot> {
    let authorities = PERSISTENT_AUTH_AUTHORITIES.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut authorities = authorities
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(authorities.entry(key).or_default())
}

#[cfg(not(target_arch = "wasm32"))]
fn persistent_auth_authorities_for_key(
    key: PersistentAuthAuthorityKey,
    store: &Arc<dyn RuntimeStore>,
) -> Arc<PersistentAuthAuthorityBundle> {
    let durable = matches!(key, PersistentAuthAuthorityKey::Durable(_));
    let slot = persistent_auth_authority_slot(key);
    let mut bundle = slot
        .bundle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = bundle.as_ref() {
        let stored_store_alive = existing
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
            .is_some();
        // A process key whose store is gone gets a fresh bundle. That case is
        // defensive: the bundle's weak reference keeps the store's allocation,
        // so no other store takes its address while the entry exists. Only a
        // test reaches it, with a key of its own.
        if durable || stored_store_alive {
            existing.oauth_flows.bind_persistent_store(store);
            *existing
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(store);
            return Arc::clone(existing);
        }
    }
    let auth_lease = Arc::new(crate::handles::RuntimeAuthLeaseHandle::new());
    let oauth_flows = Arc::new(
        crate::handles::RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            std::time::Duration::from_secs(10 * 60),
            Arc::clone(&auth_lease),
            store,
        ),
    );
    let created = Arc::new(PersistentAuthAuthorityBundle {
        store: StdMutex::new(Arc::downgrade(store)),
        auth_lease,
        oauth_flows,
    });
    *bundle = Some(Arc::clone(&created));
    created
}

/// Forget the authority bundle cached for `store`'s key, as a process
/// restart would: the next machine over a store with that key rehydrates
/// from the store. Only that key is forgotten, so tests running beside the
/// caller keep theirs.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn forget_persistent_auth_authorities_for_test(store: &Arc<dyn RuntimeStore>) {
    let key = runtime_store_identity(store);
    if let Some(authorities) = PERSISTENT_AUTH_AUTHORITIES.get() {
        authorities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl BlobStore for UnavailableBlobStore {
    async fn put_image(&self, _media_type: &str, _data: &str) -> Result<BlobRef, BlobStoreError> {
        Err(Self::error())
    }

    async fn get(&self, _blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
        Err(Self::error())
    }

    async fn delete(&self, _blob_id: &BlobId) -> Result<(), BlobStoreError> {
        Err(Self::error())
    }

    async fn exists(&self, _blob_id: &BlobId) -> Result<bool, BlobStoreError> {
        Err(Self::error())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[cfg(not(target_arch = "wasm32"))]
type MeerkatMachineCommandFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<MeerkatMachineCommandResult, MeerkatMachineCommandError>>
            + Send
            + 'a,
    >,
>;

#[cfg(target_arch = "wasm32")]
type MeerkatMachineCommandFuture<'a> = Pin<
    Box<dyn Future<Output = Result<MeerkatMachineCommandResult, MeerkatMachineCommandError>> + 'a>,
>;

#[cfg(test)]
pub(crate) use driver::fail_machine_run;
pub(crate) use driver::{
    DriverEntry, FailedRunContributorDisposition, SharedCompletionRegistry, SharedDriver,
    cancel_runtime_loop_run, commit_machine_terminal_run, commit_runtime_loop_run,
    fail_runtime_loop_run, machine_authorize_runtime_loop_batch,
    machine_batch_primitive_projections, machine_batch_runtime_semantics,
    machine_commit_prepared_destroy, machine_commit_service_turn_terminal_receipt,
    machine_prepare_bindings_projection, machine_prepare_destroy, machine_recover_ephemeral_driver,
    machine_recover_persistent_inputs_from_observed, machine_recycle_preserving_work,
    machine_reset, machine_retire, machine_stop_runtime, prepare_runtime_loop_batch_start,
};
pub(crate) mod driver;

mod comms_drain;
mod controller_custody;
pub(crate) mod credential_custody;
#[cfg(feature = "local-authorization")]
mod local_authorization;
pub use controller_custody::NativeControllerGrantMutation;
#[cfg(feature = "local-authorization")]
pub use local_authorization::{
    AgentConfiguredMcpProvider, AgentInstallationGrant, ConfiguredMcpAttachmentProvider,
    ConfiguredMcpConjuncts, NativeGrantWorkConfiguration, NativeIngressCheck,
    NativeRetainedResumeCheck,
};
#[cfg(feature = "local-authorization")]
mod scoped_original;
pub use mcp_attachment::scoped_destination_target;
#[cfg(feature = "local-authorization")]
pub use scoped_original::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, EnrolledServiceOriginal,
    OriginalDisposition, RetainedCustody, ScopedGrantComposition, ScopedInvocation,
    ScopedMandateOwner, ScopedOriginalIngress, SourceOriginalBasis, bind_scoped_original,
    original_operation_grant, require_active_original, require_source_original_live,
    review_bounds_text,
};
#[cfg(feature = "local-authorization")]
mod scoped_retained_resume;
#[cfg(feature = "local-authorization")]
pub use scoped_retained_resume::{
    RetainedControllerOwner, RetainedInvocationOwner, ScopedRetainedResume,
};
mod agent_mandate;
mod boot_obligations;
pub use boot_obligations::{
    RuntimeBootObligations, RuntimeRecoveryWork, RuntimeRecoveryWorkUnobserved,
};
pub mod composition;
mod dispatch_control;
mod dispatch_drain;
mod dispatch_ingress;
mod dispatch_session;
#[allow(unused_variables, dead_code, clippy::cmp_owned)]
#[allow(clippy::assign_op_pattern)]
pub mod dsl;
pub(crate) mod dsl_authority;
mod dsl_effects;
mod durability_health;
#[cfg(feature = "live")]
mod live_context_preparation;
mod llm_reconfigure;
mod mcp_attachment;
mod mcp_observation;
pub use mcp_observation::McpObservationRefusal;
mod admitted_original;
pub use admitted_original::{
    AdmittedOriginal, AdmittedOriginalRefusal, DurableAdmittedOriginal,
    DurableAdmittedOriginalRefusal,
};
mod resumed_run;
mod retained_resume;
pub(crate) use retained_resume::SettledRefusedResumes;
mod runtime_control;
pub use agent_mandate::{
    AgentBindingUnavailable, AgentMandateError, SessionAgentBinding, SessionAgentMandate,
};
pub(crate) mod scoped_context;
mod service_call;
pub use service_call::{ServiceCall, ServiceCallAuditError, ServiceCallOutcome};
mod predecessor_settlement;
mod session_management;
mod session_mutation_gate;
mod terminal_receipt;
mod traits;
mod visibility;

pub(crate) use durability_health::{DurabilityHealthHandle, DurabilityReloadRequired};
pub(crate) use session_mutation_gate::{
    SessionMutationGate, SettledGateWait, WeakSessionMutationGate,
};

// Live-only: the live lifecycle lease's gate helper is its one user.
#[cfg(feature = "live")]
pub(crate) use session_mutation_gate::SessionMutationGateWait;

pub(crate) use session_management::{
    DeleteOpsFinalizationAuthority, RetainOpsFinalizationAuthority,
    RuntimeLoopTeardownUnregisterAdmission,
};

pub use composition::{MeerkatCompositionSignalDispatcher, MeerkatConsumerSurface};
pub use predecessor_settlement::{
    ColdPredecessorSettlement, MachinePredecessorSettlementLease, PredecessorOwnerLoss,
    PredecessorPublication, PredecessorPublicationRefused, PredecessorSettlementRefusal,
    PredecessorTailCommit, PredecessorWork, SettledBy, SettledInput,
};
pub use runtime_control::RuntimeCompletionWaitFailureAuthority;

pub use comms_drain::{
    CommsDrainMode, CommsDrainPhase, DrainExitReason, PeerEndpointStageError, PeerIngressOwner,
    SupervisorBinding, SupervisorBindingStageError,
};
pub(crate) use comms_drain::{
    CommsDrainSlot, GeneratedSupervisorBinding, GeneratedSupervisorRotationReceipt,
    GeneratedSupervisorRotationSubmit, SupervisorAuthorizeAdmission, SupervisorBindAdmission,
    SupervisorBridgeCommandAdmission, SupervisorRotationObservation, SupervisorRotationSubmission,
    SupervisorRotationTaskSlot,
};
pub(crate) use dsl_effects::{DslTransitionEffects, apply_dsl_transition_on_authority};
pub(crate) use visibility::MachineToolVisibilityOwner;

struct StagedSessionDslInput {
    previous_snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    committed_snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    effects: DslTransitionEffects,
    signal_dispatcher: Option<composition::MeerkatCompositionSignalDispatcher>,
}

impl StagedSessionDslInput {
    /// True when the committed transition was a machine-owned revival of a
    /// stopped session (`RegisterSessionResumesStopped` /
    /// `EnsureSessionWithExecutorStopped`): the machine emits the typed
    /// `RuntimeNotice { kind: Recover }` effect, and the shell keys the
    /// durable lifecycle persist on it so a revived session is never left
    /// durably `Stopped` for cross-process readers.
    fn revived_stopped_session(&self) -> bool {
        self.effects.as_slice().iter().any(|effect| {
            matches!(
                effect,
                dsl::MeerkatMachineEffect::RuntimeNotice {
                    kind: dsl::RuntimeNoticeKind::Recover,
                    ..
                }
            )
        })
    }

    /// Whether committing this already-staged transition could publish a
    /// cross-machine seam signal. A caller may restore `previous_snapshot`
    /// after dispatch failure only when this is false: once any routed signal
    /// may have escaped, rolling local authority back would split truth.
    fn has_routed_signal_effect(&self) -> bool {
        self.effects
            .as_slice()
            .iter()
            .any(|effect| composition::lift_routed_signal(effect).is_some())
    }

    /// [`Self::has_routed_signal_effect`] without the registration release,
    /// which the final unregister holds back and dispatches only once
    /// its durable finalization committed.
    fn has_routed_signal_effect_besides_registration_release(&self) -> bool {
        self.effects.as_slice().iter().any(|effect| {
            !matches!(
                effect,
                dsl::MeerkatMachineEffect::RuntimeRegistrationReleased { .. }
            ) && composition::lift_routed_signal(effect).is_some()
        })
    }

    /// The released registration's routed signal: every final
    /// unregister publishes the exact runtime id, fence and epoch it released
    /// to the owning mob.
    fn registration_release_signals(&self) -> Vec<composition::MeerkatSeamSignal> {
        self.effects
            .as_slice()
            .iter()
            .filter(|effect| {
                matches!(
                    effect,
                    dsl::MeerkatMachineEffect::RuntimeRegistrationReleased { .. }
                )
            })
            .filter_map(composition::lift_routed_signal)
            .collect()
    }
}

#[derive(Clone, Copy)]
enum CommittedEffectDispatchFailure {
    PreserveCommittedDslState,
}

type UnregisterTeardownResult = Result<(), RuntimeDriverError>;
type RuntimeStopCleanupResult = Result<(), RuntimeDriverError>;
type ReloadRequiredDiscardResult =
    Result<ReloadRequiredRegistrationUseSettlement, RuntimeDriverError>;

/// Joinable result channel for the one owned unregister saga of an epoch.
#[derive(Clone)]
struct UnregisterTeardownCoordinator {
    epoch_id: meerkat_core::RuntimeEpochId,
    coordinator_id: uuid::Uuid,
    result_rx: crate::tokio::sync::watch::Receiver<Option<UnregisterTeardownResult>>,
    /// The checked disposition the caller that installed this coordinator
    /// asked for. `BeginUnregisterSession` records it once; from then on the
    /// generated state and the durable progress own it.
    queued_inputs: UnregisterDisposition,
    /// The disposition the machine recorded for this unregister, which the
    /// owned worker reads from the generated state and publishes once the
    /// machine began the unregister; empty until then. Every observer of
    /// this coordinator holds it.
    recorded: UnregisterRecordedDisposition,
}

/// The disposition `BeginUnregisterSession` recorded for one unregister, as
/// its owned worker publishes it from the generated state: a read of the
/// machine's record, set at most once.
type UnregisterRecordedDisposition = Arc<std::sync::OnceLock<UnregisterDisposition>>;

/// Read-only terminal-result observer for one exact runtime unregister
/// coordinator. The observer carries no mutation authority and cannot start a
/// retry; it only retains the coordinator's result channel plus the opaque
/// registration identity used for its atomic admission, and the disposition
/// the machine recorded for that unregister once it is published.
#[derive(Clone)]
pub struct RuntimeSessionUnregisterObserver {
    registration: RuntimeSessionRegistrationWitness,
    coordinator_id: uuid::Uuid,
    result_rx: crate::tokio::sync::watch::Receiver<Option<UnregisterTeardownResult>>,
    recorded: UnregisterRecordedDisposition,
}

impl RuntimeSessionUnregisterObserver {
    fn new(
        registration: RuntimeSessionRegistrationWitness,
        coordinator_id: uuid::Uuid,
        result_rx: crate::tokio::sync::watch::Receiver<Option<UnregisterTeardownResult>>,
        recorded: UnregisterRecordedDisposition,
    ) -> Self {
        Self {
            registration,
            coordinator_id,
            result_rx,
            recorded,
        }
    }

    /// Exact registration whose coordinator result this observer follows.
    #[must_use]
    pub fn registration(&self) -> &RuntimeSessionRegistrationWitness {
        &self.registration
    }

    /// The disposition the machine recorded for the exact unregister this
    /// observer follows, for the run in flight and the inputs queued behind
    /// it: what `BeginUnregisterSession` (or the unserved attachment's begin)
    /// recorded, which can differ from what any caller asked for (a keep is
    /// recorded as a stop for a registration that served live work, and an
    /// unserved attachment's unregister records a stop for a requeue). A call
    /// that joins an unregister already in progress gets that unregister's,
    /// whatever it passed. `None` until the unregister's owned worker
    /// published it, right after the machine began the unregister: once
    /// [`Self::wait_for_result`] returned `Ok`, it is `Some` unless that
    /// unregister found the registration already gone. Nothing is polled:
    /// the worker publishes it to every observer of the unregister.
    #[must_use]
    pub fn queued_inputs(&self) -> Option<UnregisterQueuedInputs> {
        self.recorded
            .get()
            .copied()
            .map(UnregisterDisposition::to_queued_inputs)
    }

    /// Return the terminal result when published, without waiting or
    /// acquiring runtime mutation authority.
    pub fn try_result(&self) -> Result<Option<Result<(), RuntimeDriverError>>, RuntimeDriverError> {
        if let Some(result) = self.result_rx.borrow().clone() {
            return Ok(Some(result));
        }
        if self.result_rx.has_changed().is_err() {
            return Err(RuntimeDriverError::Internal(format!(
                "unregister coordinator {} closed without publishing a result for session {}",
                self.coordinator_id,
                self.registration.session_id()
            )));
        }
        Ok(None)
    }

    /// Await the exact coordinator's terminal result.
    ///
    /// Observation only: the coordinator owns the teardown, so dropping this
    /// future (for example on a caller deadline) never cancels it, and a later
    /// caller can start or join the next attempt.
    pub async fn wait_for_result(&mut self) -> Result<(), RuntimeDriverError> {
        loop {
            if let Some(result) = self.result_rx.borrow_and_update().clone() {
                return result;
            }
            self.result_rx.changed().await.map_err(|_| {
                RuntimeDriverError::Internal(format!(
                    "unregister coordinator {} closed without publishing a result for session {}",
                    self.coordinator_id,
                    self.registration.session_id()
                ))
            })?;
        }
    }
}

impl std::fmt::Debug for RuntimeSessionUnregisterObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeSessionUnregisterObserver")
            .field("registration", &self.registration)
            .field("coordinator_id", &self.coordinator_id)
            .finish_non_exhaustive()
    }
}

/// Atomic admission result for exact runtime unregister observation.
#[derive(Debug)]
pub enum RuntimeSessionUnregisterAdmission {
    /// The exact unregister coordinator already completed successfully.
    Completed,
    /// The supplied registration is absent or no longer current.
    NotCurrent,
    /// The exact coordinator remains process-owned; poll this observer on a
    /// later caller retry.
    Pending(RuntimeSessionUnregisterObserver),
}

/// What a runtime unregister does with inputs still queued behind the
/// session's running turn when it stops the runtime.
///
/// [`Self::Stop`] ends every non-terminal input as stopped, as an
/// unregister always has. [`Self::Requeue`] keeps each eligible queued input
/// (a durable prompt in the Queue lane that is neither directed nor bound to
/// an objective) durably queued instead, marked for the runtime's next start,
/// which runs it unless it is older than `max_age` by then. Its waiter is
/// answered
/// [`CompletionWaitError::RequeuedAcrossStop`](crate::completion::CompletionWaitError::RequeuedAcrossStop).
/// [`Self::Keep`] keeps every non-terminal input the registration recovered
/// from the store exactly as the store holds it, for the next registration
/// to recover again, while the registration served no live work; once it did,
/// the unregister stops every input, as [`Self::Stop`] does.
/// An unregister already in progress keeps the disposition it began with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum UnregisterQueuedInputs {
    /// End every non-terminal input as stopped.
    #[default]
    Stop,
    /// Keep eligible queued inputs durably queued for the next start.
    Requeue {
        /// The age, measured from admission, past which the next start ends
        /// a kept input as expired instead of running it.
        max_age: std::time::Duration,
    },
    /// Keep every input the registration recovered from the store, exactly
    /// as the store holds it, unless the registration served live work (an
    /// admission, a deduplicated resubmission or a recycle): an unregister
    /// of a registration a host refused before it served, so the refusal
    /// ends nothing a later start would run.
    Keep,
}

impl UnregisterQueuedInputs {
    /// The longest max age a requeue may carry.
    pub const MAX_AGE_CAP: std::time::Duration = std::time::Duration::from_hours(24);

    /// The max age of the requeue a graceful process shutdown uses when its
    /// host names none ([`MeerkatMachine::graceful_shutdown_queued_inputs`]):
    /// 15 minutes.
    pub const DEFAULT_MAX_AGE: std::time::Duration = std::time::Duration::from_mins(15);

    /// Check the disposition every unregister entry point checks before it
    /// starts anything. A requeue's max age is recorded in whole
    /// milliseconds, so a max age under one millisecond (zero included) is
    /// refused, as is one over [`Self::MAX_AGE_CAP`]; neither is clamped.
    ///
    /// # Errors
    /// [`UnregisterQueuedInputsError`] names the refused max age.
    pub fn check(self) -> Result<(), UnregisterQueuedInputsError> {
        self.recorded().map(|_| ())
    }

    /// The generated disposition and the max age in whole milliseconds that
    /// `BeginUnregisterSession` records, for a disposition that passes
    /// [`Self::check`].
    pub(crate) fn recorded(self) -> Result<UnregisterDisposition, UnregisterQueuedInputsError> {
        match self {
            Self::Stop => Ok(UnregisterDisposition::STOP),
            Self::Keep => Ok(UnregisterDisposition::KEEP),
            Self::Requeue { max_age } => {
                if max_age > Self::MAX_AGE_CAP {
                    return Err(UnregisterQueuedInputsError::MaxAgeOverCap {
                        max_age,
                        cap: Self::MAX_AGE_CAP,
                    });
                }
                match u64::try_from(max_age.as_millis()) {
                    Ok(0) | Err(_) => {
                        Err(UnregisterQueuedInputsError::MaxAgeUnderOneMillisecond { max_age })
                    }
                    Ok(requeue_max_age_ms) => Ok(UnregisterDisposition {
                        queued_inputs: dsl::UnregisterQueuedInputsDisposition::Requeue,
                        requeue_max_age_ms,
                    }),
                }
            }
        }
    }
}

/// A refused [`UnregisterQueuedInputs`]: nothing was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UnregisterQueuedInputsError {
    /// The requeue max age is under one millisecond, the resolution the stop
    /// mark records; zero is refused here too.
    #[error("requeue max age {max_age:?} is under one millisecond")]
    MaxAgeUnderOneMillisecond { max_age: std::time::Duration },
    /// The requeue max age is over [`UnregisterQueuedInputs::MAX_AGE_CAP`].
    #[error("requeue max age {max_age:?} is over the {cap:?} cap")]
    MaxAgeOverCap {
        max_age: std::time::Duration,
        cap: std::time::Duration,
    },
}

/// A checked [`UnregisterQueuedInputs`], as an unregister coordinator holds
/// it: the generated disposition and the whole milliseconds
/// `BeginUnregisterSession` records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnregisterDisposition {
    queued_inputs: dsl::UnregisterQueuedInputsDisposition,
    requeue_max_age_ms: u64,
}

impl UnregisterDisposition {
    /// Stop every non-terminal input; nothing to record.
    pub(crate) const STOP: Self = Self {
        queued_inputs: dsl::UnregisterQueuedInputsDisposition::Stop,
        requeue_max_age_ms: 0,
    };

    /// Keep every non-terminal input the registration recovered from the
    /// store exactly as the store holds it, with no requeue mark: every
    /// materialization rollback, the retirement of an attachment that never
    /// served (a pending attachment's abort, a failed startup's cleanup), and
    /// a host's refusal of a restored registration
    /// ([`UnregisterQueuedInputs::Keep`]).
    /// An input admitted while the registration stood still ends as stopped.
    /// The machine grants the keep only to a registration that served no
    /// live work: after an admission, a deduplicated resubmission or a recycle
    /// it records the unregister as Stop.
    pub(crate) const KEEP: Self = Self {
        queued_inputs: dsl::UnregisterQueuedInputsDisposition::Keep,
        requeue_max_age_ms: 0,
    };

    pub(crate) fn dsl_disposition(self) -> (dsl::UnregisterQueuedInputsDisposition, u64) {
        (self.queued_inputs, self.requeue_max_age_ms)
    }

    /// This disposition as an [`UnregisterQueuedInputs`]: a requeue's max
    /// age is the whole milliseconds it records.
    pub(crate) fn to_queued_inputs(self) -> UnregisterQueuedInputs {
        match self.queued_inputs {
            dsl::UnregisterQueuedInputsDisposition::Stop => UnregisterQueuedInputs::Stop,
            dsl::UnregisterQueuedInputsDisposition::Requeue => UnregisterQueuedInputs::Requeue {
                max_age: std::time::Duration::from_millis(self.requeue_max_age_ms),
            },
            dsl::UnregisterQueuedInputsDisposition::Keep => UnregisterQueuedInputs::Keep,
        }
    }
}

/// Answer of
/// [`MeerkatMachine::observe_unregister_session_registration_if_use_unchanged`].
#[derive(Debug)]
#[non_exhaustive]
pub enum RuntimeSessionUnregisterUseAdmission {
    /// The registration's use was the expected one when the unregister was
    /// started or joined, or the registration is no longer current: the
    /// exact admission's answer.
    Admitted(RuntimeSessionUnregisterAdmission),
    /// The registration is current but was handed to another user after the
    /// expected use was read. Nothing was started or joined.
    Changed,
}

/// Typed answer of the same-registration quiescent close admission
/// ([`MeerkatMachine::unregister_session_registration_until_terminal_if_quiescent`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeSessionQuiescentClose {
    /// The exact registration held no non-terminal input and no run. Its
    /// owned unregister teardown completed after `BeginUnregisterSession`
    /// committed while its mutation gate still excluded ingress.
    Closed,
    /// The exact registration holds a non-terminal input or a run. Nothing
    /// was staged: the registration and its running owner keep serving, and
    /// every retained input stays queued native work. A drain window that
    /// lost its owner answers this only while a run is still in flight.
    Busy,
    /// Another unregister already owns this registration's teardown. This
    /// call staged and joined nothing.
    AlreadyClosing,
    /// The supplied registration is absent or no longer current.
    NotCurrent,
    /// The exact registration's orphaned drain or pending finalization was
    /// completed with no run in flight. This is not a quiescent admission:
    /// the teardown abandons any inputs the drain still held (reason
    /// `Stopped`) and resolves their completions as runtime-terminated, so a
    /// later resume does not run them. This variant also applies when the
    /// orphaned drain held no inputs.
    ClosedOrphanedDrain,
}

/// The wait a caller dispatched on after starting or joining an owned
/// unregister teardown saga, reported to a test that armed
/// [`MeerkatMachine::test_witness_next_unregister_wait`].
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnregisterTeardownWaitWitness {
    /// Bounded by the ordinary caller grace: a saga still running when it
    /// elapses is answered with typed `UnregisterInProgress`.
    CallerGrace,
    /// Awaits the saga's terminal result.
    UntilTerminal,
}

/// Whether one session's runtime registration can serve, as a single typed
/// fact read from one registration entry.
///
/// Dispatch and member reload must read this same fact: a durability-ready
/// registration is not a serving one, and a registration without a committed
/// executor attachment is detached, whatever its durability (#1248).
#[derive(Debug, Clone)]
pub enum RuntimeSessionAttachmentState {
    /// No registration for the session exists on this machine.
    Unregistered,
    /// The registration is durability-ready and its committed executor
    /// attachment serves the session.
    Attached(RuntimeExecutorAttachmentWitness),
    /// The registration lost durable authority (`ReloadRequired`). Its
    /// committed attachment, when one still exists, is reported so callers
    /// keep observing the typed reload-required refusal at admission.
    ReloadRequired {
        registration: RuntimeSessionRegistrationWitness,
        attachment: Option<RuntimeExecutorAttachmentWitness>,
    },
    /// The registration is durability-ready but holds no committed executor
    /// attachment, so it cannot serve. An explicit resume (member reload)
    /// must complete `unregister` and re-attach the session.
    Detached {
        registration: RuntimeSessionRegistrationWitness,
        unregister: RuntimeDetachedUnregister,
    },
}

/// What [`MeerkatMachine::take_over_stopped_attachment_registration_if_current`]
/// did with one exact registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoppedRegistrationTakeover {
    /// The registration was the completed residue of a stopped attachment,
    /// and its canonical unregister completed.
    Unregistered,
    /// The witness no longer names the session's current registration.
    /// Nothing changed.
    NotCurrent,
    /// The exact registration is not a stopped attachment's completed
    /// residue: it is attached, its stop or post-stop cleanup did not
    /// complete, it was never attached, it is not Stopped (a Retired
    /// runtime included), it is claimed, or it needs a durability reload.
    /// Nothing changed.
    NotStoppedResidue,
    /// A stop cleanup on the exact registration was still in flight when
    /// the caller's bound on joining it ran out. Nothing was decided and
    /// nothing changed; the cleanup goes on, owned by the runtime.
    StopCleanupPending,
}

/// Where a detached registration's machine-owned unregister stands.
#[derive(Debug, Clone)]
pub enum RuntimeDetachedUnregister {
    /// No unregister coordinator exists and none has failed.
    NotStarted,
    /// An unregister coordinator owns the teardown now.
    InFlight,
    /// The runtime-loop teardown's owned unregister failed. Its failure is
    /// retained with generated `Draining` truth, and nothing retries it until
    /// an explicit caller starts the next attempt.
    Failed(RuntimeDriverError),
}

/// Joinable result channel for the one owned ordinary-stop cleanup operation
/// of an epoch. Completed results remain installed until a successful resume
/// replaces the executor, or an explicit stop/unregister authorizes a retry.
#[derive(Clone)]
struct RuntimeStopCleanupCoordinator {
    epoch_id: meerkat_core::RuntimeEpochId,
    coordinator_id: uuid::Uuid,
    teardown_slot: Option<Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
    result_rx: crate::tokio::sync::watch::Receiver<Option<RuntimeStopCleanupResult>>,
}

/// Process-owned cleanup of one exact durability-degraded registration.
///
/// This is deliberately distinct from unregister and ordinary stop: it owns
/// no generated lifecycle transition and may only discard process-local
/// material before a later cold registration reloads durable authority.
#[derive(Clone)]
struct ReloadRequiredDiscardCoordinator {
    epoch_id: meerkat_core::RuntimeEpochId,
    coordinator_id: uuid::Uuid,
    result_rx: crate::tokio::sync::watch::Receiver<Option<ReloadRequiredDiscardResult>>,
}

#[derive(Clone)]
struct PendingUnregisterFinalization {
    finalization_id: uuid::Uuid,
    durability_authority: session_management::RuntimeOpsLifecycleDurabilityAuthority,
    committed_snapshot: dsl::MeerkatMachineAuthoritySnapshot,
}

struct UnregisterTeardownMechanicalObservations {
    runtime_loop_forced_abort: std::sync::atomic::AtomicBool,
    comms_drain_forced_abort: std::sync::atomic::AtomicBool,
    /// Report intervals elapsed while the unregister saga waited for the
    /// runtime loop to hand off the exact executor.
    ///
    /// Non-zero means that handoff stalled. This is an observation and never a
    /// bound: nothing is aborted or failed on its account, and the wait itself
    /// stays deliberately unbounded for the reason recorded at its await site.
    runtime_loop_handoff_wait_reports: std::sync::atomic::AtomicU32,
}

/// Owned persistence worker for one runtime's ops epoch. The unregister saga
/// closes the registry producer and joins this worker before the atomic store
/// finalization can retire the epoch.
#[cfg(not(target_arch = "wasm32"))]
struct OpsLifecyclePersistenceWorker {
    handle: std::thread::JoinHandle<()>,
}

#[cfg(target_arch = "wasm32")]
struct OpsLifecyclePersistenceWorker {
    handle: crate::tokio::task::JoinHandle<()>,
}

impl UnregisterTeardownMechanicalObservations {
    fn new() -> Self {
        Self {
            runtime_loop_forced_abort: std::sync::atomic::AtomicBool::new(false),
            comms_drain_forced_abort: std::sync::atomic::AtomicBool::new(false),
            runtime_loop_handoff_wait_reports: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn from_durable_process_recovery(
        progress: Option<&crate::store::MachineUnregisterProgressSnapshot>,
    ) -> Self {
        let observations = Self::new();
        if let Some(progress) = progress {
            // A pending producer obligation recovered in a fresh process has
            // lost its original JoinHandle. Closing it now is necessarily a
            // process-loss/forced disposition, never evidence of clean drain.
            observations.runtime_loop_forced_abort.store(
                progress.runtime_loop_drain_pending(),
                std::sync::atomic::Ordering::Release,
            );
            observations.comms_drain_forced_abort.store(
                progress.comms_drain_exit_pending(),
                std::sync::atomic::Ordering::Release,
            );
        }
        observations
    }
}

/// Why a registration was inserted mechanically, without a serving owner.
/// An ordinary registration has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MechanicalRegistration {
    /// Reconstructed solely for archive convergence. `from_quiescent`: the
    /// archive recovery began from durable Retired/Destroyed truth, a
    /// public-result fact only that never substitutes for cleanup ownership.
    Archive { from_quiescent: bool },
    /// The internal, non-serving registration of a predecessor settlement
    /// (`predecessor_settlement`). Its marker refuses, with
    /// `UnregisterInProgress` and at each path's first decision, every path
    /// that could make it live or end it: registration and adoption,
    /// executor and exact-materialization claims, admission, control and
    /// lifecycle commands, explicit and quiescent unregister, stop, and the
    /// reload of a degraded registration. Only the settlement takes its
    /// mutation gate, and only the settlement removes it, exactly and without
    /// retiring anything.
    PredecessorSettlement,
}

/// Per-session state: driver + generated authority binding + shell handles.
struct RuntimeSessionEntry {
    /// Canonical runtime control-plane identity for this registered session.
    runtime_id: LogicalRuntimeId,
    /// This registration's current use ([`RuntimeSessionRegistrationUse`]):
    /// stamped from the machine-wide sequence when the entry is built, and
    /// stamped again by every adoption that hands the registration to a new
    /// user. Adoptions stamp it under the session map lock, which every
    /// reader of it holds too.
    registration_use: std::sync::atomic::AtomicU64,
    /// The agent this registration's session acts as, bound by the trusted
    /// governed host. Every registration owns a fresh, unbound slot.
    agent_binding: agent_mandate::SessionAgentBinding,
    /// The tool-surface gate this registration's trusted host installed, if
    /// any. Every registration owns a fresh, empty slot.
    tool_surface_gate: meerkat_core::ToolSurfaceGateSlot,
    /// The resumed-run admission this registration's host bound, if any.
    /// Every registration owns a fresh, empty slot.
    resumed_run_admission: crate::resumed_run_admission::ResumedRunAdmissionSlot,
    /// The MCP adapter this registration's latest finished construction
    /// installed, for observation only. Every registration owns a fresh slot.
    mcp_observation: meerkat_core::McpObservationSlot,
    /// This runtime owner's hosting claim for the session (#1813), taken
    /// before the registration's durable recovery and held exactly as long as
    /// this entry: unregister finalization, which removes the entry after
    /// teardown, releases it.
    _hosting_claim: meerkat_core::session_hosting::HostingClaim,
    /// The owner-loss proof whose settlement this registration completed
    /// when it published: present only when its claim's lineage began with a
    /// cross-process fresh acquisition or takeover, the issuing consent
    /// owner settled it, and no earlier registration of the lineage
    /// completed it. Historical record only.
    owner_loss_proof: Option<meerkat_core::session_hosting::HostingOwnerLossProof>,
    /// Why this registration was inserted mechanically, without a serving
    /// owner: archive convergence or a predecessor settlement. Its cleanup
    /// ownership survives caller timeout; ordinary and concurrent
    /// registrations never carry one.
    mechanical_registration: Option<MechanicalRegistration>,
    /// Per-session mutation gate (M). It carries this entry's
    /// `durability_health`, so every copy of it can end a live wait on the
    /// session's degradation (see [`SessionMutationGate`]).
    mutation_gate: SessionMutationGate,
    /// Shared fail-closed durability gate for persistent sessions.
    ///
    /// The persistent driver degrades this exact handle when its live shell can
    /// no longer be proven against durable state. Direct session mutations
    /// check it while holding `mutation_gate`; a degraded entry mechanically
    /// detaches and interrupts its executor and can only be replaced by a new
    /// registration-authorized cold install. Storeless sessions carry `None`.
    durability_health: Option<DurabilityHealthHandle>,
    /// Serializes the complete live-open materialization window against a
    /// lifecycle owner's physical-absence proof + terminal marker window.
    /// This is separate from `mutation_gate`: live orchestration calls back
    /// into ordinary DSL mutations while it holds this lease.
    #[cfg(feature = "live")]
    live_lifecycle_gate: Arc<Mutex<()>>,
    /// Session-owned liveness driver for the currently pending supervisor
    /// rotation. Durable operation state remains in generated authority.
    supervisor_rotation_task: Arc<SupervisorRotationTaskSlot>,
    /// Shared driver handle (accessed by both adapter methods and RuntimeLoop).
    driver: SharedDriver,
    /// Canonical coarse control projection for this session.
    ///
    /// The driver reads this to realize shell mechanics, but machine-facing
    /// queries should publish from this shared cell rather than treating the
    /// driver shell as the source of lifecycle truth.
    control_projection: Arc<StdRwLock<crate::driver::ephemeral::RuntimeControlProjection>>,
    /// Session-scoped staged -> executing window record, shared with the
    /// driver shell that created it and armed by the runtime loop at the
    /// durable `StageForRun` commit.
    ///
    /// Mechanical observation state for the health census only - see the void
    /// condition on [`crate::run_progress::SharedRunStartWindowCell`]. The
    /// verdict is never stored here; [`RuntimeSessionEntry::run_start_health`]
    /// recomputes it from `dsl_authority` on every read.
    run_start_window: crate::run_progress::SharedRunStartWindowCell,
    /// Shared async-operation lifecycle registry for this runtime/session.
    ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
    /// Joinable durability worker for `ops_lifecycle`. Never detached: final
    /// unregister must prove it has observed the closed producer before the
    /// store records the epoch tombstone.
    ops_lifecycle_persistence_worker: Option<OpsLifecyclePersistenceWorker>,
    /// Runtime epoch identity — stable across rebuilds, rotated on reset/restart-without-recovery.
    epoch_id: meerkat_core::RuntimeEpochId,
    /// Mechanical close gate for handles minted from this session entry.
    ///
    /// The DSL still owns runtime terminality; this gate only invalidates cloned
    /// cross-crate handles after the entry is torn down.
    handle_teardown_gate: Arc<crate::handles::HandleTeardownGate>,
    /// Exact non-clone materialization claimant. Compatibility bindings do not
    /// reserve this slot; actor begin, unique preparation, attachment, and
    /// teardown transition it synchronously so no claimant can steal or roll
    /// back another transaction.
    materialization_claim_state:
        Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    /// Shared consumer cursor state for the epoch.
    cursor_state: Arc<meerkat_core::EpochCursorState>,
    /// Observe-only hook dispatcher for facts committed by this session.
    post_commit_hooks: Arc<meerkat_core::PostCommitHookDispatcher>,
    /// Completion waiters (accessed by accept_input_with_completion and RuntimeLoop).
    completions: SharedCompletionRegistry,
    /// Canonical durable visibility owner for this session.
    tool_visibility_owner: Arc<MachineToolVisibilityOwner>,
    /// One canonical epoch-local mechanical handle bundle.
    ///
    /// Actor reconstruction for an unchanged attachment clones these exact
    /// Arcs and replaces only its one-shot materialization authority. Minting
    /// fresh surface handles for the same epoch would replay MCP lifecycle
    /// transitions and split handle identity from the attached executor.
    canonical_runtime_bindings: Option<meerkat_core::SessionRuntimeBindings>,
    /// Runtime-loop channel publication slot.
    ///
    /// This is mechanical shell state only. The generated `MeerkatMachine`
    /// `registration_phase` is the semantic executor registration authority.
    attachment_slot: RuntimeLoopAttachmentSlot,
    /// Exact executor-cleanup handoff retained across unregister retries.
    ///
    /// The attachment channels and loop JoinHandle are consumed by the first
    /// teardown attempt, but a failing external cleanup restores its executor
    /// into this slot so the next machine-owned saga can retry without
    /// fabricating quiescence.
    runtime_loop_teardown: Option<Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
    /// Current epoch's owned unregister saga, if one is active.
    unregister_coordinator: Option<UnregisterTeardownCoordinator>,
    /// Current epoch's single ordinary-stop cleanup owner. Unlike unregister,
    /// its successful terminal leaves this registration in generated Stopped.
    runtime_stop_cleanup_coordinator: Option<RuntimeStopCleanupCoordinator>,
    /// Exact process-owned disposal of a `ReloadRequired` registration.
    reload_required_discard_coordinator: Option<ReloadRequiredDiscardCoordinator>,
    /// A missing-live materialization normalized stale executor authority but
    /// has not yet committed its replacement attachment. The next exact
    /// attachment commit owns the matching durable lifecycle publication.
    pending_revival_lifecycle_persist: Arc<std::sync::atomic::AtomicBool>,
    /// Typed interrupted-run notices owed to the model. The runtime loop
    /// records them in the transcript at its next pre-dequeue position (no
    /// turn of their own), then acknowledges their evidence.
    interrupted_tool_notices: Arc<std::sync::Mutex<Option<InterruptedToolNotices>>>,
    /// Retry witness installed before the final generated UnregisterSession
    /// transition. If an owned saga panics after that transition changes the
    /// live projection to Queuing/session_id=None, the next saga resumes the
    /// same atomic store commit instead of trying to BeginUnregister again.
    pending_unregister_finalization: Option<PendingUnregisterFinalization>,
    /// Cancellation-safe shell observations waiting to be committed through
    /// the matching generated unregister feedback inputs.
    unregister_teardown_observations: Arc<UnregisterTeardownMechanicalObservations>,
    /// Durable-session publication capability retained independently of the
    /// loop channels. Unregister detaches those channels before its final
    /// terminal sweep; publication retries must still use the same owning
    /// session surface rather than dropping the outbox or inventing another
    /// publisher.
    publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
    /// Cloneable surface/service teardown retained across loop detachment so
    /// external unregister and retry-finalize paths can finish the same exact
    /// cleanup transaction.
    post_stop_cleanup_handle:
        Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPostStopCleanupHandle>>,
    /// Attachment incarnation authorized to run `post_stop_cleanup_handle`.
    post_stop_cleanup_attachment_id: Option<RuntimeLoopAttachmentId>,
    /// Whether the retained cleanup completed for that incarnation.
    post_stop_cleanup_complete: bool,
    /// Serializes cloneable cleanup attempts for the current attachment while
    /// the machine mutation gate is deliberately released. External
    /// unregister, loop-owned unregister, and final-unregister retry may race
    /// in `Draining`; exactly one of them may call the surface cleanup handle
    /// at a time. On the not-yet-completed path the ordinary cleanup callback
    /// also joins the removed actor's exit while this attachment-local gate is
    /// held, which assumes an actor drain never needs this gate.
    post_stop_cleanup_gate: Arc<Mutex<()>>,
    /// Temporary live interrupt capability for prepared, session-owned turns
    /// that run before the runtime loop attachment is published.
    provisional_interrupt_handle:
        Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>>,
    /// Exact process-owned hard-interrupt dispatch. Same-run retries join this
    /// result instead of spawning duplicate callbacks against a wedged custom
    /// executor. Cold restart drops this process-local slot and reissues from
    /// durable run/lifecycle authority.
    pending_user_interrupt_dispatch: Option<PendingUserInterruptDispatch>,
    /// Exact prepared-materialization transaction that installed the
    /// provisional interrupt/cleanup pair. A stale binding cannot replace or
    /// clear another transaction's provisional ownership.
    provisional_materialization_claim_id: Option<uuid::Uuid>,
    /// DSL authority for coarse lifecycle phase transitions.
    /// Sync field — validates transitions, writes back phase.
    ///
    /// `Arc<std::sync::Mutex<_>>` so cross-crate handle impls
    /// (`meerkat-runtime::handles::*`) can share the same underlying authority
    /// from a sync context without awaiting the outer `sessions` tokio lock.
    /// The Arc heap-allocates the authority's large expanded state (31 fields
    /// including several Maps/Sets) so holding a reference to a
    /// `RuntimeSessionEntry` does not bloat async future sizes.
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
    /// Commit generation of `dsl_authority`. It advances once after every
    /// transition committed through the session's apply seam
    /// (`apply_session_dsl_input*`), which carries every live delegation
    /// result-release guard input. Waiters re-check a refused guarded
    /// transition on each advance instead of on a timer.
    dsl_commits: Arc<crate::tokio::sync::watch::Sender<u64>>,
    /// Per-session comms drain lifecycle slot.
    ///
    /// Collapsed from the sibling `MeerkatMachine.comms_drain_slots:
    /// RwLock<HashMap<SessionId, CommsDrainSlot>>` in wave-c C-H2 (F5 in
    /// docs/wave-c-prep/state-scope-audit.md) — keeping the slot here
    /// makes "session exists" a single HashMap insertion and eliminates
    /// the class of bugs where the sibling map and the session map
    /// could fall out of sync across a registration/unregistration
    /// boundary.
    drain_slot: CommsDrainSlot,
    /// Reverse composition endpoint installed by the exact materialization
    /// claim on this registration. Direct sessions have no mob consumer.
    /// Exact degraded reload preserves delivery custody on its unbound cold
    /// successor. Ordinary removal drops it; effect batches retain their
    /// captured endpoint across awaits instead of looking up a successor.
    composition_signal_dispatcher: Option<composition::MeerkatCompositionSignalDispatcher>,
    /// Exact installer custody for failed materialization compensation.
    composition_materialization_claim_id: Option<uuid::Uuid>,
}

/// Fully recovered persistent session entry that has not yet been published
/// into the process-local session map. Durable observation and driver recovery
/// happen before the stable registration transaction is acquired; publication
/// then rechecks the map under that transaction and either installs this exact
/// candidate or discards it in favor of the concurrent winner.
struct PreparedArchiveSessionRegistration {
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
    entry: RuntimeSessionEntry,
    cold_recovered_generated_draining: bool,
    ops_lifecycle_persistence_receiver: Option<
        crate::tokio::sync::mpsc::UnboundedReceiver<
            crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
        >,
    >,
}

#[derive(Clone)]
struct MemberIncarnationRegistration {
    incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    session_mutation_gate: SessionMutationGate,
    tracked_turn_journal: Option<Arc<dyn crate::member_observation::TrackedTurnJournal>>,
}

#[derive(Clone)]
struct DirectMemberIncarnationRegistration {
    fence: meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence,
    runtime_epoch_id: meerkat_core::RuntimeEpochId,
    session_mutation_gate: SessionMutationGate,
}

enum MemberResidencyState {
    /// Ordinary runtime session with no host-placement authority.
    PeerOnly {
        /// Exact controller-member authority installed by V5 `BindMember`.
        /// This remains under the stable residency slot so successor binds and
        /// retire validation share one serialization boundary.
        direct_member: Option<DirectMemberIncarnationRegistration>,
    },
    /// A host-owned session id whose placed incarnation is absent, released,
    /// or in cutover. This must never be treated as peer-only.
    VacantPlaced,
    /// Exact current host placement plus its runtime/journal authorities.
    Placed(MemberIncarnationRegistration),
}

struct MemberResidencySlot {
    gate: Arc<Mutex<()>>,
    state: StdRwLock<MemberResidencyState>,
}

struct MemberEffectAuthorityLease {
    slot: Arc<MemberResidencySlot>,
    slot_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    session_mutation_gate: SessionMutationGate,
}

/// Exact effect interval for a host-member residency. The stable residency
/// slot blocks G1→G2/vacate publication while the captured session guard
/// blocks unregister/same-SessionId runtime replacement. Neither authority
/// may be dropped before the caller's effect completes.
pub(crate) struct MemberEffectAuthorityGuard {
    _slot_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    _session_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

#[derive(Debug)]
pub(crate) enum DirectMemberRetireError {
    Stale {
        current:
            Option<meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFenceEvidence>,
    },
    Runtime(crate::traits::RuntimeControlPlaneError),
}

impl From<crate::traits::RuntimeControlPlaneError> for DirectMemberRetireError {
    fn from(error: crate::traits::RuntimeControlPlaneError) -> Self {
        Self::Runtime(error)
    }
}

/// Exact residency plus registration-stability interval for member-live work.
///
/// Unlike an ordinary member effect, member-live operations call back into
/// machine mutations through the shared live pipeline. Holding the session
/// mutation gate here would deadlock that canonical pipeline. The stable
/// residency slot prevents generation cutover, while the registration
/// transaction prevents unregister or same-id replacement until the live
/// operation returns.
pub(crate) struct MemberLiveAuthorityGuard {
    _slot_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    _registration_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

impl MemberResidencySlot {
    fn peer_only() -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            state: StdRwLock::new(MemberResidencyState::PeerOnly {
                direct_member: None,
            }),
        }
    }
}

/// Non-cloneable lease that holds one session's mutation authority across a
/// cross-crate archive transaction. The session service acquires this before
/// its recovery/checkpointer gates, then realizes Retire through the captured
/// driver without re-locking the mutation gate.
pub struct MachineSessionArchiveLease {
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
    driver: SharedDriver,
    completions: SharedCompletionRegistry,
    wake_tx: Option<mpsc::Sender<()>>,
    publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
    /// True only when archive recovery itself inserted the in-memory runtime
    /// registration from durable authority. A quiescent terminal lease with
    /// this witness may remove that reconstructable registration without
    /// touching durable lifecycle truth; a registration that predated archive
    /// must never be removed through that cleanup path.
    recovered_registration_for_archive: bool,
    /// True only when the exact recovered registration originated from a
    /// durable quiescent terminal. Successful convergence preserves public
    /// duplicate-archive NotFound only for this origin.
    recovered_from_quiescent_archive: bool,
    /// Stable absent-entry/register/unregister transaction boundary. It is
    /// acquired before the live and mutation leases and retained through any
    /// archive-only removal of the recovered entry.
    _registration_transaction_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    /// Shared live/lifecycle boundary retained from physical absence proof
    /// through the durable archive/retire marker.
    _live_lifecycle_lease: Option<crate::member_live::MemberLiveLifecycleLease>,
    _mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait MachineSessionArchivePostCommitHook: Send + Sync {
    /// Run only after the runtime lifecycle is durably Retired and before the
    /// exact recovered archive registration is removed. The hook must be
    /// idempotent and bounded: failure retains that registration, and an
    /// `AlreadyArchived` retry invokes the same obligation again. Higher
    /// layers may persist a typed receipt in their hook state and return `Ok`
    /// only once that receipt makes a repeated invocation convergent. No
    /// caller can cancel it: the retire runs in its own task while it holds
    /// the session's mutation gate, so a hook that never returns holds that
    /// gate until it does.
    async fn after_runtime_retire_commit(&self) -> Result<(), RuntimeControlPlaneError>;
}

/// Selects whether local materialization preserves ordinary authority or
/// performs the narrow machine-owned missing-live normalization first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalSessionMaterializationMode {
    /// Prepare local session resources without replacing a machine-authorized
    /// executor binding.
    #[default]
    Ordinary,
    /// Normalize the narrow ownerless executor shape authorized by Mob
    /// missing-live revival before preparing its replacement attachment.
    MissingLiveRevival,
}

/// Unique, cancellation-safe owner of one prepared session materialization.
///
/// Runtime bindings remain cloneable factory data; this lease is deliberately
/// non-clone so rollback ownership can only move between orchestration layers by
/// an explicit Rust move. Dropping an armed lease synchronously fences executor
/// attachment, then schedules exact provisional cleanup/registration rollback.
pub struct PreparedSessionMaterialization {
    machine: Arc<MeerkatMachine>,
    bindings: meerkat_core::SessionRuntimeBindings,
    claim_id: uuid::Uuid,
    claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    inserted_registration: bool,
    cleanup_spawner: MachineCleanupTaskSpawner,
    archived_resume_authorization_issued: bool,
    armed: bool,
}

/// Exact post-actor commit authority for an Archived+Retired materialization.
///
/// This non-clone lease retains the machine mutation gate while the session
/// owner confirms the same RuntimeStore authority and while the runtime
/// performs the Retired -> Idle transition. A broad session-control token
/// cannot cross this boundary while an exact materialization claim is
/// outstanding.
pub struct PreparedArchivedResumeCommitLease {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    claim_id: uuid::Uuid,
    claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    _mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

/// Type-state proof that the session owner and this exact machine attachment
/// claim share one RuntimeStore authority.
///
/// Only this authorized form can realize Retired -> Idle.
pub struct AuthorizedArchivedResumeCommitLease {
    prepared: PreparedArchivedResumeCommitLease,
}

impl PreparedArchivedResumeCommitLease {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Bind the exact prepared lease to the session owner's RuntimeStore.
    /// The shared-store check prevents a foreign service from authorizing this
    /// machine's runtime reset.
    pub fn confirm_runtime_store_authority(
        self,
        runtime_store: &Arc<dyn crate::RuntimeStore>,
    ) -> Result<AuthorizedArchivedResumeCommitLease, RuntimeDriverError> {
        if !self.machine.shares_runtime_store_authority(runtime_store) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "archived-resume authorization for session {} used a runtime store not owned by the prepared machine",
                    self.session_id
                ),
            });
        }
        Ok(AuthorizedArchivedResumeCommitLease { prepared: self })
    }
}

impl AuthorizedArchivedResumeCommitLease {
    pub fn session_id(&self) -> &SessionId {
        self.prepared.session_id()
    }

    /// Realize the exact store-owned Retired -> Idle transition.
    pub async fn reset_retired_runtime(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        self.prepared
            .machine
            .reset_runtime_for_authorized_archived_resume(self)
            .await
    }
}

/// One-shot authorization to construct an actor at the Archived+Retired
/// revival midpoint for one exact prepared machine registration.
///
/// This is deliberately non-cloneable and non-constructible outside the
/// machine. It carries the exact claim and registration identities, while
/// [`begin_session_runtime_actor_materialization_for_archived_resume`](crate::begin_session_runtime_actor_materialization_for_archived_resume)
/// acquires the current machine mutation gate and revalidates those identities
/// plus the exact `Retired` lifecycle phase before actor construction starts.
#[must_use = "archived-resume authorization must be consumed by actor materialization"]
pub struct ArchivedSessionActorMaterializationAuthorization {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    claim_id: uuid::Uuid,
    claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
    teardown_gate: Arc<crate::handles::HandleTeardownGate>,
}

impl ArchivedSessionActorMaterializationAuthorization {
    pub(crate) async fn begin(
        self,
        bindings: &meerkat_core::SessionRuntimeBindings,
    ) -> Result<crate::RuntimeActorMaterializationPermit, crate::RuntimeActorMaterializationError>
    {
        let binding_authority = crate::validated_session_runtime_bindings_authority(bindings)?;
        if bindings.session_id() != &self.session_id
            || bindings.epoch_id() != &self.epoch_id
            || binding_authority.materialization_claim_id != Some(self.claim_id)
            || !Arc::ptr_eq(
                &binding_authority.materialization_claim_state,
                &self.claim_state,
            )
            || !Arc::ptr_eq(&binding_authority.dsl_authority, &self.dsl_authority)
            || !Arc::ptr_eq(&binding_authority.teardown_gate, &self.teardown_gate)
        {
            return Err(crate::RuntimeActorMaterializationError::InvalidAuthority(
                "archived-resume authorization does not match the exact prepared binding"
                    .to_string(),
            ));
        }

        let mutation_guard = self
            .machine
            .lock_current_durability_ready_session_mutation_gate(&self.session_id)
            .await
            .map_err(|_| crate::RuntimeActorMaterializationError::RegistrationClosed)?;
        {
            let sessions = self.machine.sessions.read().await;
            let entry = sessions
                .get(&self.session_id)
                .ok_or(crate::RuntimeActorMaterializationError::RegistrationClosed)?;
            let exact_registration = entry.epoch_id == self.epoch_id
                && Arc::ptr_eq(&entry.materialization_claim_state, &self.claim_state)
                && Arc::ptr_eq(&entry.dsl_authority, &self.dsl_authority)
                && Arc::ptr_eq(&entry.handle_teardown_gate, &self.teardown_gate)
                && entry.provisional_materialization_claim_id == Some(self.claim_id)
                && !entry.physical_attachment_is_live();
            let exact_claim = self
                .claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .exact_claim_is(
                    self.claim_id,
                    &[
                        crate::RuntimeActorMaterializationClaimPhase::Prepared,
                        crate::RuntimeActorMaterializationClaimPhase::Staged,
                    ],
                );
            if !exact_registration || !exact_claim {
                return Err(crate::RuntimeActorMaterializationError::RegistrationClosed);
            }
        }

        crate::begin_session_runtime_actor_materialization_with_phase_policy(
            bindings,
            crate::RuntimeActorMaterializationPhasePolicy::RequireArchivedRevivalBoundary,
            Some(mutation_guard),
        )
    }
}

/// Exact actor-only recovery lease for a session whose executor attachment is
/// already committed and serving.
///
/// The lease holds the machine mutation gate for that attachment, so service
/// actor reconstruction cannot race executor replacement or unregister. It
/// owns no registration/attachment rollback authority and can never spawn a
/// second runtime loop.
pub struct PreparedAttachedSessionActorRecovery {
    bindings: meerkat_core::SessionRuntimeBindings,
    witness: RuntimeExecutorAttachmentWitness,
    claim_id: uuid::Uuid,
    claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    previous_phase: crate::RuntimeActorMaterializationClaimPhase,
    mutation_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    armed: bool,
}

/// Exact actor-materialization authority that may be consumed by one executor
/// attachment attempt. This remains crate-private: surfaces transfer the
/// non-clone [`PreparedSessionMaterialization`] lease rather than minting or
/// comparing claim identifiers themselves.
#[derive(Clone)]
struct RuntimeExecutorAttachmentMaterializationClaim {
    claim_id: uuid::Uuid,
    claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    epoch_id: meerkat_core::RuntimeEpochId,
}

/// What a run-start hold (#1500) found when it took effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStartsHold {
    /// The run current when the hold took effect: the only run that may still
    /// execute, and the only one a stop may still cancel. `None` means the
    /// member had no run.
    pub current_run: Option<meerkat_core::lifecycle::RunId>,
}

/// The work a session had admitted at a cancel point, taken by
/// [`MeerkatMachine::abandon_queued_inputs_at_cancel_point`] under one hold
/// of the session's mutation gate.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedWork {
    /// The run the machine recorded as current at the cancel point. The cancel
    /// point leaves it running; the caller cancels exactly this run.
    pub current_run: Option<meerkat_core::lifecycle::RunId>,
    /// The inputs that were queued at the cancel point, in either lane, in
    /// admission order. The cancel point abandoned each of them.
    pub queued_inputs: Vec<InputId>,
}

/// Why a runtime's run starts are held (#1500). No new run starts while any
/// reason holds a runtime; each holder releases only its own reason.
#[non_exhaustive]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RunStartHoldReason {
    /// A mob Stop holds its members until Resume.
    MobStop,
    /// A restored member waits until its host has published its tools.
    ToolsNotPublished,
}

impl RunStartHoldReason {
    pub(crate) fn dsl(self) -> dsl::RunStartHoldReason {
        match self {
            Self::MobStop => dsl::RunStartHoldReason::MobStop,
            Self::ToolsNotPublished => dsl::RunStartHoldReason::ToolsNotPublished,
        }
    }
}

/// What a caller refused with
/// [`RuntimeBindingsError::RegistrationOwned`] observes once a session's
/// actor-materialization claim is no longer in flight.
///
/// In-flight claim phases (prepared, staged, actor creating, pending commit,
/// aborting) always leave through a transition that notifies, so they are
/// awaited. A claim that settled as an actor committed without an executor is
/// not in flight: nothing obliges its owner to ever attach one, so waiting for
/// it to clear could wait forever. That settlement is reported instead of
/// awaited, and only an authority entitled to replace the session's actor may
/// reclaim it.
#[derive(Clone)]
pub enum MaterializationClaimObservation {
    /// No claim blocks a new materialization: the claim is vacant, the
    /// registration left the registry or was replaced, or a committed
    /// executor attachment now owns the session.
    Released,
    /// The claim settled as a retained actor with no executor attachment.
    /// `registration` names the exact registration that actor was built
    /// against.
    RetainedUnattached {
        registration: RuntimeSessionRegistrationWitness,
    },
}

impl std::fmt::Debug for MaterializationClaimObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Released => f.write_str("Released"),
            Self::RetainedUnattached { registration } => f
                .debug_struct("RetainedUnattached")
                .field("session_id", registration.session_id())
                .finish(),
        }
    }
}

/// Opaque identity for one exact runtime-session registration.
///
/// Unlike [`RuntimeExecutorAttachmentWitness`], this witness deliberately
/// carries no executor identity. It exists for machine-owned cleanup of a
/// terminal registration that never published an attachment (for example, a
/// registration materialized only to recover its durable ops lifecycle).
/// Callers may clone and compare the witness, but only this machine can use it
/// to admit exact compare-and-remove teardown. Durable epoch identity alone is
/// not exact because an epoch may survive an in-process entry rebuild; the
/// private weak mutation-gate identity distinguishes those incarnations.
#[derive(Clone)]
pub struct RuntimeSessionRegistrationWitness {
    machine: std::sync::Weak<MeerkatMachineShared>,
    session_id: SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    registration_gate: WeakSessionMutationGate,
}

impl RuntimeSessionRegistrationWitness {
    fn new(
        machine: std::sync::Weak<MeerkatMachineShared>,
        session_id: SessionId,
        epoch_id: meerkat_core::RuntimeEpochId,
        registration_gate: WeakSessionMutationGate,
    ) -> Self {
        Self {
            machine,
            session_id,
            epoch_id,
            registration_gate,
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn epoch_id(&self) -> &meerkat_core::RuntimeEpochId {
        &self.epoch_id
    }

    /// Whether this exact registration has ended for good: its runtime entry
    /// is gone and nothing holds the entry's private incarnation identity,
    /// so no machine can have it current, now or later. Read-only and
    /// one-directional: `false` does not mean the registration is current
    /// (an ended entry's identity can still be held while a task finishes
    /// with it). Grants nothing and starts nothing.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        // A live entry holds its mutation gate strongly for its whole
        // lifetime, and a rebuilt entry gets a gate of its own, so no strong
        // reference left means this incarnation's entry exists nowhere. A
        // dropped allocation can never gain a strong reference again.
        self.registration_gate.upgrade().is_none()
    }

    fn belongs_to(&self, machine: &MeerkatMachine) -> bool {
        std::ptr::eq(self.machine.as_ptr(), Arc::as_ptr(&machine.shared))
    }

    fn matches_entry(&self, entry: &RuntimeSessionEntry) -> bool {
        self.registration_gate
            .upgrade()
            .is_some_and(|gate| gate.same_gate(&entry.mutation_gate))
    }

    fn registration_gate(&self) -> Option<SessionMutationGate> {
        self.registration_gate.upgrade()
    }
}

impl std::fmt::Debug for RuntimeSessionRegistrationWitness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeSessionRegistrationWitness")
            .field("session_id", &self.session_id)
            .field("epoch_id", &self.epoch_id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RuntimeSessionRegistrationWitness {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.epoch_id == other.epoch_id
            && std::sync::Weak::ptr_eq(&self.machine, &other.machine)
            && self.registration_gate.same_gate(&other.registration_gate)
    }
}

impl Eq for RuntimeSessionRegistrationWitness {}

/// Which use of one exact runtime-session registration a caller saw.
///
/// A registration can be handed to a new user without being replaced: a
/// later `register_session` reuses it as it is, and an executor attachment
/// or an actor materialization adopts it in place, so its
/// [`RuntimeSessionRegistrationWitness`] stays the same. Every such adoption
/// stamps the registration with a fresh value of one machine-wide sequence.
/// A use read earlier therefore differs from the current one once anyone
/// has taken the registration since, and a successor registration never
/// carries its predecessor's use. Opaque: compare it for equality only. Read
/// it with [`MeerkatMachine::session_registration_use`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSessionRegistrationUse(u64);

/// Result of exact cleanup for one durability-degraded runtime registration.
///
/// This classification is registration-incarnation scoped. `NotCurrent`
/// includes an absent registration, a same-session replacement, and a witness
/// minted by another machine. None of those observations authorizes mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadRequiredRegistrationDisposition {
    /// The witness no longer names this machine's exact current registration.
    NotCurrent,
    /// The exact registration remains durability-ready and was left untouched.
    NotDegraded,
    /// The exact ReloadRequired process shell was quiesced and atomically
    /// replaced by a recovered cold registration from durable authority.
    Discarded,
}

/// Terminal result of a registration reload, including the exact successor
/// minted at publication. Sampling by session id after settlement could
/// mistake a later replacement for the registration this reload published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadRequiredRegistrationSettlement {
    NotCurrent,
    NotDegraded,
    Discarded {
        successor: RuntimeSessionRegistrationWitness,
    },
}

/// Exclusive physical lease for exact actor cleanup after a cold registration
/// reload. It carries no lifecycle mutation authority and starts no teardown.
/// Holding it prevents executor publication and registration replacement.
/// Service callers acquire B first, then this lease acquires T, optional L,
/// and exact M. Exact persistent actor cleanup may acquire R while it is held.
pub struct RuntimeOwnerlessRegistrationLease {
    _registration_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    #[cfg(feature = "live")]
    _live_lifecycle_lease: crate::member_live::MemberLiveLifecycleLease,
    _mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

pub enum RuntimeOwnerlessRegistrationAdmission {
    Acquired(RuntimeOwnerlessRegistrationLease),
    /// The exact registration remains current but another physical owner or
    /// cleanup/claim obligation prevents actor cleanup. Retain retry custody.
    CurrentButOwned,
    NotCurrent,
}

impl ReloadRequiredRegistrationSettlement {
    pub fn disposition(&self) -> ReloadRequiredRegistrationDisposition {
        match self {
            Self::NotCurrent => ReloadRequiredRegistrationDisposition::NotCurrent,
            Self::NotDegraded => ReloadRequiredRegistrationDisposition::NotDegraded,
            Self::Discarded { .. } => ReloadRequiredRegistrationDisposition::Discarded,
        }
    }
}

/// Answer of
/// [`MeerkatMachine::reload_required_registration_settlement_if_current`]:
/// the registration reload's terminal result, with the use the published
/// successor starts at.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReloadRequiredRegistrationUseSettlement {
    /// The witness no longer names this machine's exact current registration.
    NotCurrent,
    /// The exact registration remains durability-ready and was left untouched.
    NotDegraded,
    /// The exact ReloadRequired process shell was replaced by `successor`,
    /// the registration this reload published, whose use starts at
    /// `successor_use`.
    Discarded {
        successor: RuntimeSessionRegistrationWitness,
        successor_use: RuntimeSessionRegistrationUse,
    },
    /// The registration was handed to another user after the expected use
    /// was read: nothing was discarded and no running discard was joined.
    Changed,
}

impl ReloadRequiredRegistrationUseSettlement {
    /// The settlement of a reload that was not gated on a use.
    fn into_ungated_settlement(
        self,
    ) -> Result<ReloadRequiredRegistrationSettlement, RuntimeDriverError> {
        match self {
            Self::NotCurrent => Ok(ReloadRequiredRegistrationSettlement::NotCurrent),
            Self::NotDegraded => Ok(ReloadRequiredRegistrationSettlement::NotDegraded),
            Self::Discarded { successor, .. } => {
                Ok(ReloadRequiredRegistrationSettlement::Discarded { successor })
            }
            Self::Changed => Err(RuntimeDriverError::Internal(
                "a use-gated reload result reached a caller that gave no expected use".to_string(),
            )),
        }
    }
}

/// Public projection of one registered session's fail-closed durability gate.
///
/// `MeerkatMachine::durability_reload_required` returns this when a persistent
/// runtime shell has degraded to `ReloadRequired`: every ordinary input
/// admission for that session fails closed with
/// `RuntimeDriverError::RecoveryRepairBlocked` until a registration-authorized
/// cold reload replaces the registration. Surfaces that serialize deliveries
/// across many sessions (the mob actor) use it to reject work for the degraded
/// session before dispatch instead of discovering the refusal downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionDurabilityReloadRequired {
    /// The durable operation whose failure degraded the shell.
    pub operation: String,
    /// Display detail retained from the first degradation.
    pub reason: String,
}

impl std::fmt::Display for SessionDurabilityReloadRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "durability reload required after `{}`: {}",
            self.operation, self.reason
        )
    }
}

/// Exact durable lifecycle observation for one session registration target.
///
/// Construction is owned by MeerkatMachine so a raw-content version observed
/// for one runtime id cannot be replayed as the precondition for another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSessionLifecycleObservation {
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
    lifecycle: crate::store::MachineLifecycleObservation,
}

impl RuntimeSessionLifecycleObservation {
    fn new(
        session_id: SessionId,
        runtime_id: LogicalRuntimeId,
        lifecycle: crate::store::MachineLifecycleObservation,
    ) -> Self {
        Self {
            session_id,
            runtime_id,
            lifecycle,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }

    #[must_use]
    pub fn lifecycle(&self) -> &crate::store::MachineLifecycleObservation {
        &self.lifecycle
    }
}

/// Result of one observation-bound, externally fenced cold registration.
///
/// The durable lifecycle observation is target-local runtime content. The
/// external write fence is evaluated separately and is never encoded into that
/// content or its version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSessionRegistrationOutcome {
    /// The exact observed lifecycle row was replaced with the canonical fresh
    /// Idle shell and the matching live registration was published.
    Applied {
        registration: RuntimeSessionRegistrationWitness,
        lifecycle: RuntimeSessionLifecycleObservation,
    },
    /// The exact durable row was already the canonical fresh Idle shell. Its
    /// external fence was still checked and the matching live registration was
    /// published.
    AlreadyExact {
        registration: RuntimeSessionRegistrationWitness,
        lifecycle: RuntimeSessionLifecycleObservation,
    },
    /// Either the target observation or the external write authority changed.
    /// No live registration was published by this invocation.
    Conflict {
        current: RuntimeSessionLifecycleObservation,
        reason: String,
    },
    /// The exact row or required store capability cannot be repaired safely.
    RepairBlocked {
        evidence_digest: Option<String>,
        reason: String,
    },
    /// Observation, fence admission, or target persistence was temporarily
    /// unavailable. The caller should re-observe and retry.
    Backoff { reason: String },
    /// Another runtime owner hosts the session: another process on the same
    /// realm, or another machine of this process (it holds the session's
    /// hosting claim, #1813). No live registration was published and the
    /// durable row was not touched.
    ServedElsewhere { session_id: SessionId },
    /// The store selected cross-process hosting claims, but the session's
    /// claim cannot be taken (#1813). No live registration was published and
    /// the durable row was not touched.
    HostingUnavailable { session_id: SessionId },
}

/// Opaque identity for one exact runtime-executor attachment.
///
/// The machine mints this witness before constructing the executor so
/// surface-owned cleanup handles can retain the same identity.  It is
/// deliberately non-serializable and exposes no raw attachment identifier;
/// callers may clone and compare it, but cannot manufacture authority over a
/// replacement attachment.
#[derive(Clone)]
pub struct RuntimeExecutorAttachmentWitness {
    machine: std::sync::Weak<MeerkatMachineShared>,
    session_id: SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    attachment_id: RuntimeLoopAttachmentId,
}

impl RuntimeExecutorAttachmentWitness {
    fn new(
        machine: std::sync::Weak<MeerkatMachineShared>,
        session_id: SessionId,
        epoch_id: meerkat_core::RuntimeEpochId,
        attachment_id: RuntimeLoopAttachmentId,
    ) -> Self {
        Self {
            machine,
            session_id,
            epoch_id,
            attachment_id,
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn epoch_id(&self) -> &meerkat_core::RuntimeEpochId {
        &self.epoch_id
    }

    fn belongs_to(&self, machine: &MeerkatMachine) -> bool {
        std::ptr::eq(self.machine.as_ptr(), Arc::as_ptr(&machine.shared))
    }
}

impl std::fmt::Debug for RuntimeExecutorAttachmentWitness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeExecutorAttachmentWitness")
            .field("session_id", &self.session_id)
            .field("epoch_id", &self.epoch_id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RuntimeExecutorAttachmentWitness {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.epoch_id == other.epoch_id
            && self.attachment_id == other.attachment_id
            && std::sync::Weak::ptr_eq(&self.machine, &other.machine)
    }
}

impl Eq for RuntimeExecutorAttachmentWitness {}

/// Exact result of ensuring a runtime executor.
///
/// `Existing` proves a committed, serving attachment already owns the
/// session. `Pending` owns a newly attached/startup-reconciled executor whose
/// serving capabilities remain unpublished until the surface commits its
/// exact sidecars.
pub enum EnsureRuntimeExecutorAttachment {
    Existing(RuntimeExecutorAttachmentWitness),
    Pending(PendingRuntimeExecutorAttachment),
}

/// Process-lifetime executor for machine cleanup work transferred out of an
/// RAII lease.
///
/// Native Tokio handles do allow cross-thread spawning, but once their owning
/// application runtime is dropped a newly spawned task is immediately
/// cancelled. Exact attachment cleanup must outlive both the task that opened
/// the lease and that task's runtime, so native builds use one runtime owned by
/// this crate for the rest of the process. WebAssembly already has a
/// process-wide JavaScript event loop and `tokio_with_wasm::spawn` is not tied
/// to an application-owned Tokio runtime. The slot caches only a successfully
/// built runtime: resource exhaustion during the first attempt is retryable.
#[cfg(not(target_arch = "wasm32"))]
static MACHINE_CLEANUP_RUNTIME: OnceLock<StdMutex<Option<crate::tokio::runtime::Runtime>>> =
    OnceLock::new();

/// A durability-degradation caller must observe its canonical repair-blocked
/// result even when a public custom interrupt handle never completes. The
/// exact process-owned callback continues after this acknowledgement budget.
#[cfg(test)]
const DURABILITY_DEGRADATION_INTERRUPT_ACK_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(100);
#[cfg(not(test))]
const DURABILITY_DEGRADATION_INTERRUPT_ACK_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(5);

/// How long an interrupt caller waits for the process-owned executor callback
/// before it reports the outcome unknown (the callback continues). Unit tests
/// use the same bound by default and shorten it per machine only where they
/// exercise a wedged callback (`set_user_interrupt_ack_timeout_for_test`).
const USER_INTERRUPT_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(test)]
const DIRECT_MEMBER_BIND_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
#[cfg(not(test))]
const DIRECT_MEMBER_BIND_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Cloneable proof that cancellation-safe machine cleanup can be dispatched.
///
/// Every RAII owner with asynchronous cleanup acquires and stores this before
/// it receives cleanup authority. Its `spawn` operation is therefore
/// infallible with respect to runtime initialization and never depends on the
/// caller's ambient runtime.
#[derive(Clone)]
struct MachineCleanupTaskSpawner {
    #[cfg(not(target_arch = "wasm32"))]
    handle: crate::tokio::runtime::Handle,
}

/// One process-owned convergence attempt for the exact durable runless
/// terminal carriers attached to a single driver incarnation.
///
/// The map entry is installed while that incarnation still owns M. The task
/// starts only after its caller releases every session/registration guard, so
/// arbitrary publication code cannot wedge replacement. Same-process retries
/// join `result_rx`; a cold machine has no process slot and reissues solely
/// from the durable outbox.
struct PendingRunlessTerminalPublicationDispatch {
    dispatch_id: uuid::Uuid,
    driver: SharedDriver,
    mutation_gate: SessionMutationGate,
    requested_generation: u64,
    result_rx: crate::tokio::sync::watch::Receiver<Option<Result<(), RuntimeDriverError>>>,
}

struct PendingSessionArchiveLeasePreparation {
    preparation_id: uuid::Uuid,
    completion_rx: crate::tokio::sync::watch::Receiver<Option<Result<(), String>>>,
}

struct PendingDirectMemberBindAdmission {
    admission_id: uuid::Uuid,
    requested: meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
    completion_rx: crate::tokio::sync::watch::Receiver<
        Option<
            Result<
                meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence,
                RuntimeDriverError,
            >,
        >,
    >,
}

/// Process-lifetime cleanup dispatcher for surface transactions that must
/// outlive the ambient Tokio runtime which opened them.
///
/// Tasks spawned here must not block a thread. The dispatcher is one small
/// fixed pool (two workers on native targets) shared by every session's
/// cleanup and user-interrupt callbacks: run synchronous work through
/// `spawn_blocking` or a dedicated thread and await it. Two tasks that block
/// their workers stall every session's cleanup, and each interrupt caller
/// then reports `InterruptDispatchOutcomeUnknown` when its acknowledgement
/// bound (5 s) elapses.
#[doc(hidden)]
#[derive(Clone)]
pub struct RuntimeCleanupTaskSpawner {
    inner: MachineCleanupTaskSpawner,
}

impl RuntimeCleanupTaskSpawner {
    pub fn acquire() -> Result<Self, RuntimeDriverError> {
        Ok(Self {
            inner: MachineCleanupTaskSpawner::acquire()?,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_detached<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.inner.spawn(future);
    }

    #[cfg(target_arch = "wasm32")]
    pub fn spawn_detached<F>(&self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        self.inner.spawn(future);
    }
}

/// What a ready-gated caller found once it held the session mutation gate.
enum DurabilityGateSettlement {
    /// The exact entry is durability-ready: the gate is the caller's.
    Ready(crate::tokio::sync::OwnedMutexGuard<()>),
    /// The entry was replaced while the caller waited: look it up again.
    Replaced,
    /// The entry is gone or degraded (its degraded settlement has run).
    Refused(RuntimeDriverError),
}

/// The typed refusal of a live wait for a session mutation gate that ended
/// on the session's degradation.
fn degraded_gate_wait_refusal(required: DurabilityReloadRequired) -> RuntimeDriverError {
    RuntimeDriverError::RecoveryRepairBlocked {
        evidence_digest: None,
        reason: required.to_string(),
    }
}

/// Take a session's driver without M, or end once the session degrades. A
/// boundary commit or run terminalization holds the driver together with M,
/// and one past its commit deadline keeps both until the store answers, so a
/// wait for the driver alone would wait for the store. The driver is polled
/// first: a free driver is taken even on a degraded session, and a session
/// without durable state waits as before.
async fn lock_driver_live<'a, T>(
    driver: &'a crate::tokio::sync::Mutex<T>,
    durability_health: Option<&DurabilityHealthHandle>,
) -> Result<crate::tokio::sync::MutexGuard<'a, T>, DurabilityReloadRequired> {
    let Some(health) = durability_health else {
        return Ok(driver.lock().await);
    };
    let degradation = health.subscribe_degradation();
    let mut lock = std::pin::pin!(driver.lock());
    let mut fired = std::pin::pin!(degradation.fired());
    crate::tokio::select! {
        biased;
        guard = &mut lock => Ok(guard),
        required = &mut fired => Err(required),
    }
}

/// Settle a held session mutation gate: hand it back for a durability-ready
/// entry, or detach the degraded entry and hard-cancel its exact run before
/// refusing. Shared by the ready-gated caller and the process-owned task a
/// waiter hands the settlement to when it leaves on the degradation signal.
async fn settle_durability_gate(
    shared: &MeerkatMachineShared,
    session_id: &SessionId,
    gate: &SessionMutationGate,
    gate_guard: crate::tokio::sync::OwnedMutexGuard<()>,
) -> DurabilityGateSettlement {
    let (reload_required, interrupt_handle, expected_run_id, detached_attachment, cleanup_spawner) = {
        let mut sessions = shared.sessions.write().await;
        let Some(entry) = sessions.get_mut(session_id) else {
            return DurabilityGateSettlement::Refused(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        if !entry.mutation_gate.same_gate(gate) {
            return DurabilityGateSettlement::Replaced;
        }
        match entry.require_durability_ready() {
            Ok(()) => return DurabilityGateSettlement::Ready(gate_guard),
            Err(reload_required) => {
                let interrupt_handle = entry.interrupt_handle();
                let expected_run_id = entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .state()
                    .current_run_id
                    .as_ref()
                    .and_then(crate::meerkat_machine::dsl_authority::current_run_id_from_dsl);
                // Acquire process-owned execution before consuming the
                // entry-local handle/attachment authority. A runtime
                // setup failure leaves that authority installed for a
                // later degraded retry instead of orphaning it.
                let cleanup_spawner = match (&interrupt_handle, &expected_run_id) {
                    (Some(_), Some(_)) => match MachineCleanupTaskSpawner::acquire() {
                        Ok(cleanup_spawner) => Some(cleanup_spawner),
                        Err(error) => {
                            tracing::warn!(
                                %session_id,
                                operation = reload_required.operation(),
                                reason = reload_required.reason(),
                                %error,
                                "could not acquire process-owned durability-degradation interrupt dispatcher; exact attachment retained for retry"
                            );
                            return DurabilityGateSettlement::Refused(
                                RuntimeDriverError::RecoveryRepairBlocked {
                                    evidence_digest: None,
                                    reason: reload_required.to_string(),
                                },
                            );
                        }
                    },
                    _ => None,
                };
                let detached_attachment = entry.take_runtime_loop_attachment();
                // Consume every entry-local interrupt source under M.
                // This is the dedup handoff: later degraded callers
                // see no handle and cannot spawn a second callback for
                // the same exact attachment/run.
                entry.provisional_interrupt_handle = None;
                (
                    reload_required,
                    interrupt_handle,
                    expected_run_id,
                    detached_attachment,
                    cleanup_spawner,
                )
            }
        }
    };

    // Dropping the exact attachment closes its wake/effect senders and
    // detaches it from the entry before the async interrupt callback.
    drop(detached_attachment);
    // The entry is already detached and every ready-gated successor
    // will observe ReloadRequired. Do not retain M across the external
    // interrupt callback: actor cancellation may itself need to finish
    // a machine-owned terminal callback.
    drop(gate_guard);
    if let (Some(interrupt_handle), Some(expected_run_id), Some(cleanup_spawner)) =
        (interrupt_handle, expected_run_id, cleanup_spawner)
    {
        let reason = format!(
            "durability reload required after `{}`: {}",
            reload_required.operation(),
            reload_required.reason()
        );
        let (ack_tx, ack_rx) = crate::tokio::sync::oneshot::channel();
        cleanup_spawner.spawn(async move {
            let mut first_ack = Some(ack_tx);
            let mut retry_delay = std::time::Duration::from_millis(100);
            loop {
                let result = match std::panic::AssertUnwindSafe(
                    interrupt_handle.hard_cancel_run_if_current(&expected_run_id, reason.clone()),
                )
                .catch_unwind()
                .await
                {
                    Ok(Ok(delivered)) => Ok(delivered),
                    Ok(Err(error)) => Err(format!(
                        "exact durability-degradation interrupt failed: {error}"
                    )),
                    Err(payload) => Err(format!(
                        "exact durability-degradation interrupt panicked: {}",
                        meerkat_core::panic_payload::panic_payload_detail(payload.as_ref())
                    )),
                };
                if let Some(ack_tx) = first_ack.take() {
                    let _ = ack_tx.send(result.clone());
                }
                if result.is_ok() {
                    break;
                }
                // ReloadRequired remains the durable shell authority
                // while this exact handle/run pair retries
                // process-owned. No successor can be admitted through
                // the degraded entry.
                crate::tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(std::time::Duration::from_secs(5));
            }
        });
        match crate::tokio::time::timeout(DURABILITY_DEGRADATION_INTERRUPT_ACK_TIMEOUT, ack_rx)
            .await
        {
            Ok(Ok(Ok(_))) => {}
            Ok(Ok(Err(error))) => {
                tracing::warn!(
                    %session_id,
                    operation = reload_required.operation(),
                    reason = reload_required.reason(),
                    %error,
                    "exact durability-degradation interrupt failed; process-owned retry retained"
                );
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    %session_id,
                    operation = reload_required.operation(),
                    reason = reload_required.reason(),
                    %error,
                    "exact durability-degradation interrupt task ended before acknowledgement"
                );
            }
            Err(_) => {
                tracing::warn!(
                    %session_id,
                    operation = reload_required.operation(),
                    reason = reload_required.reason(),
                    "exact durability-degradation interrupt exceeded acknowledgement budget; process-owned attempt retained"
                );
            }
        }
    }
    DurabilityGateSettlement::Refused(RuntimeDriverError::RecoveryRepairBlocked {
        evidence_digest: None,
        reason: reload_required.to_string(),
    })
}

/// A degraded settlement task's entry for its gate in
/// `MeerkatMachineShared::pending_degraded_gate_settlements`. The task owns it
/// and removes it when it ends on any outcome, or when it is dropped before it
/// runs.
struct PendingDegradedGateSettlement {
    shared: Arc<MeerkatMachineShared>,
    gate_address: usize,
}

impl PendingDegradedGateSettlement {
    /// List `gate`, or `None` when a settlement task already waits for it.
    fn claim(shared: &Arc<MeerkatMachineShared>, gate: &SessionMutationGate) -> Option<Self> {
        let gate_address = gate.address();
        shared
            .pending_degraded_gate_settlements
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(gate_address)
            .then(|| Self {
                shared: Arc::clone(shared),
                gate_address,
            })
    }
}

impl Drop for PendingDegradedGateSettlement {
    fn drop(&mut self) {
        self.shared
            .pending_degraded_gate_settlements
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.gate_address);
    }
}

impl MachineCleanupTaskSpawner {
    #[cfg(not(target_arch = "wasm32"))]
    fn acquire() -> Result<Self, RuntimeDriverError> {
        let slot = MACHINE_CLEANUP_RUNTIME.get_or_init(|| StdMutex::new(None));
        let mut runtime = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(runtime) = runtime.as_ref() {
            return Ok(Self {
                handle: runtime.handle().clone(),
            });
        }

        // Machine cleanup restores and reduces the same generated DSL state
        // as the host runtime and runs session and mob cleanup and custody
        // work for the whole process (the turns of sessions and mob members
        // run on the host's runtime), so its threads get the process-runtime
        // stack: 16 MiB, or what the host stack knobs set (see
        // `crate::host_stack`).
        let stack_bytes = crate::host_stack::process_runtime_stack_bytes().map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "failed to start machine cleanup runtime: {error}"
            ))
        })?;
        let candidate = crate::tokio::runtime::Builder::new_multi_thread()
            // Cleanup and ingress share this dispatcher. One owner's
            // synchronous poll must not monopolize every session's lane.
            .worker_threads(2)
            .thread_name("meerkat-machine-cleanup")
            .thread_stack_size(stack_bytes)
            .enable_all()
            .build()
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to start machine cleanup runtime: {error}"
                ))
            })?;
        let handle = candidate.handle().clone();
        *runtime = Some(candidate);
        Ok(Self { handle })
    }

    #[cfg(target_arch = "wasm32")]
    fn acquire() -> Result<Self, RuntimeDriverError> {
        Ok(Self {})
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn<F, T>(&self, future: F) -> crate::tokio::task::JoinHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.handle.spawn(future)
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn<F, T>(&self, future: F) -> crate::tokio::task::JoinHandle<T>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        crate::tokio::spawn(future)
    }
}

impl std::fmt::Debug for EnsureRuntimeExecutorAttachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Existing(witness) => formatter.debug_tuple("Existing").field(witness).finish(),
            Self::Pending(pending) => formatter.debug_tuple("Pending").field(pending).finish(),
        }
    }
}

/// Non-cloneable commit lease for one exact newly attached executor.
///
/// The runtime loop has completed startup reconciliation, but the machine
/// withholds serving publication and wake effects while this lease is armed.
/// A surface may publish sidecars tagged with [`Self::witness`] and then call
/// [`Self::commit`]. Dropping the lease transfers its mutation fence into an
/// independently owned exact-unregister saga, so caller cancellation cannot
/// strand a live executor or tear down a same-SessionId replacement.
#[must_use = "dropping a pending runtime attachment aborts that exact attachment"]
pub struct PendingRuntimeExecutorAttachment {
    machine: Arc<MeerkatMachine>,
    witness: RuntimeExecutorAttachmentWitness,
    mutation_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    /// Runless terminals this attachment committed under its mutation
    /// guard (settled interrupted runs, a replaced predecessor), told once
    /// that guard is released or handed off. Declared after the guard so
    /// that it drops after it.
    runless_terminal: RunlessTerminalAnnouncement,
    cleanup_spawner: MachineCleanupTaskSpawner,
    should_wake: bool,
    persist_lifecycle_on_commit: bool,
    armed: bool,
    /// Interrupted-run notices to deliver once this attachment serves.
    interrupted_tool_notices: Option<InterruptedToolNotices>,
}

/// Interrupted-run evidence whose model notice is still owed: settled runs
/// (inputs settled as interrupted) and moot runs (already completed) whose
/// process outlived them.
pub(crate) struct InterruptedToolNotices {
    pub(crate) evidence: Arc<dyn meerkat_core::tool_process::InterruptedToolEvidence>,
    pub(crate) calls: Vec<OwedInterruptedToolNotice>,
}

/// One piece of evidence owed to the model, with what settling it did to the
/// run and the run's user requests (recorded once, before the notice).
#[derive(Clone)]
pub(crate) struct OwedInterruptedToolNotice {
    pub(crate) call: meerkat_core::tool_process::InterruptedToolCall,
    pub(crate) disposition: meerkat_core::tool_process::InterruptedToolRunDisposition,
    pub(crate) requests: Vec<meerkat_core::tool_process::InterruptedRequest>,
}

/// Upper bound on the user request content one interrupted run copies into
/// its evidence. Requests past it are not restored; the notice names them.
const INTERRUPTED_REQUEST_CONTENT_BOUND: usize = 64 * 1024;

/// The transcript facts `for_input` stamps on a recovered input from its
/// admitted semantics, its interaction id included. They match what its run
/// stamped only when that run held this one input: a run that merged several
/// inputs stamped their merged identity, which can differ from this input's
/// own (possibly none). `None` for a row without stored semantics, which
/// keeps the prompt's own.
fn stamped_turn_metadata(
    state: &crate::input_state::InputState,
) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
    let input = state.persisted_input.as_ref()?;
    let semantics = state.runtime_semantics?;
    Some(crate::runtime_loop::for_input(input, semantics))
}

impl MeerkatMachine {
    /// Install trusted native ingress/policy composition on a storeless machine.
    /// Persistent profiles must be selected during fallible construction.
    /// No serialized input can select this component.
    pub fn with_native_work_authorization_host(
        self,
        host: Arc<dyn crate::input_authority::NativeWorkAuthorizationHost>,
    ) -> Result<Self, RuntimeDriverError> {
        self.require_storeless_profile_installation()?;
        self.install_native_work_authorization(|_| host)
    }

    fn require_storeless_profile_installation(&self) -> Result<(), RuntimeDriverError> {
        if self.store.is_some() {
            return Err(execution_custody_error(
                crate::store::RuntimeStoreExecutionCustodyError::Unsupported,
            ));
        }
        Ok(())
    }

    fn install_native_work_authorization(
        mut self,
        build_host: impl FnOnce(
            std::sync::Weak<MeerkatMachineShared>,
        ) -> Arc<dyn crate::input_authority::NativeWorkAuthorizationHost>,
    ) -> Result<Self, RuntimeDriverError> {
        {
            let shared =
                Arc::get_mut(&mut self.shared).ok_or_else(crate::input_authority::unavailable)?;
            if !shared.sessions.get_mut().is_empty()
                || shared.native_work_authorization_host.get().is_some()
            {
                return Err(crate::input_authority::unavailable());
            }
            shared.require_governed_execution_custody()?;
        }
        // Check exclusive installation before constructing either Weak owner.
        let host = build_host(Arc::downgrade(&self.shared));
        let attachment = credential_custody::NativeWorkAuthorizationAttachment::new(host, &self);
        self.native_work_authorization_host
            .set(attachment)
            .map_err(|_| crate::input_authority::unavailable())?;
        self.install_native_credential_observer()?;
        Ok(self)
    }

    /// Install the issuing consent owner's owner-loss settlement (consent
    /// owner loss): every registration that attaches a claim lineage owing a
    /// settlement (its grant excluded every earlier holder of the session's
    /// claim) has that owner settle what a lost owner left in the session's
    /// scope, under the registration's live reservation, before the session
    /// can admit anything. The composition installs the same approval owner
    /// that issues the consent, once, before any session is registered.
    /// Without one nothing is settled, so no registration ever records a
    /// settlement as complete.
    pub fn install_consent_owner_loss_settlement(
        &self,
        settlement: Arc<dyn meerkat_core::approval::ConsentOwnerLossSettlement>,
    ) -> Result<(), ConsentOwnerLossInstallRefused> {
        if self.consent_owner_loss_settlement.get().is_some() {
            return Err(ConsentOwnerLossInstallRefused::AlreadyInstalled);
        }
        let sessions = self
            .sessions
            .try_read()
            .map_err(|_| ConsentOwnerLossInstallRefused::SessionsRegistered)?;
        if !sessions.is_empty() {
            return Err(ConsentOwnerLossInstallRefused::SessionsRegistered);
        }
        self.consent_owner_loss_settlement
            .set(settlement)
            .map_err(|_| ConsentOwnerLossInstallRefused::AlreadyInstalled)?;
        drop(sessions);
        Ok(())
    }

    /// Whether the issuing consent owner's owner-loss settlement was
    /// installed. A composition observation, never a permission decision.
    pub fn has_consent_owner_loss_settlement(&self) -> bool {
        self.consent_owner_loss_settlement.get().is_some()
    }

    /// Whether the actual native owner was installed before this machine was
    /// shared. This is a composition observation, never a permission decision.
    pub fn has_native_work_authorization_host(&self) -> bool {
        self.native_work_authorization_host.get().is_some()
    }

    /// Install the trusted host's binder of governed peer deliveries
    /// ([`crate::input_authority::PeerIngressBinder`]): from now on a peer
    /// delivery whose sender the transport established is offered to it
    /// before admission, and admitted only as it binds it. The composition
    /// installs it once, after the native work authorization host and before
    /// any session is registered. Without a binder a governed runtime refuses
    /// peer deliveries as before.
    pub fn install_peer_ingress_binder(
        &self,
        binder: Arc<dyn crate::input_authority::PeerIngressBinder>,
    ) -> Result<(), PeerIngressBinderInstallRefused> {
        if self.native_work_authorization_host.get().is_none() {
            return Err(PeerIngressBinderInstallRefused::Ungoverned);
        }
        if self.peer_ingress_binder.get().is_some() {
            return Err(PeerIngressBinderInstallRefused::AlreadyInstalled);
        }
        let sessions = self
            .sessions
            .try_read()
            .map_err(|_| PeerIngressBinderInstallRefused::SessionsRegistered)?;
        if !sessions.is_empty() {
            return Err(PeerIngressBinderInstallRefused::SessionsRegistered);
        }
        self.peer_ingress_binder
            .set(binder)
            .map_err(|_| PeerIngressBinderInstallRefused::AlreadyInstalled)?;
        drop(sessions);
        Ok(())
    }

    /// Whether a governed peer delivery binder was installed. A composition
    /// observation, never a permission decision.
    pub fn has_peer_ingress_binder(&self) -> bool {
        self.peer_ingress_binder.get().is_some()
    }

    /// The installed peer delivery binder, on a governed runtime only.
    pub(crate) fn installed_peer_ingress_binder(
        &self,
    ) -> Option<Arc<dyn crate::input_authority::PeerIngressBinder>> {
        if self.native_work_authorization_host.get().is_none() {
            return None;
        }
        self.peer_ingress_binder.get().map(Arc::clone)
    }

    /// A governed persistent machine over `store` with the host `build`
    /// makes for it installed, as a governed open builds one, for tests
    /// whose host is a fixture.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn persistent_governed_with_host_for_test(
        store: Arc<dyn RuntimeStore>,
        blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
        build: impl FnOnce(&Self) -> Arc<dyn crate::input_authority::NativeWorkAuthorizationHost>,
    ) -> Result<Self, RuntimeDriverError> {
        let machine = Self::persistent_with_mode(store, blob_store, true)?;
        let host = build(&machine);
        machine.install_native_work_authorization(|_| host)
    }

    /// Mark `session_id`'s driver as served by an executor that supports
    /// governed work, as the runtime loop does when such an executor
    /// attaches, for tests that admit governed input with no executor.
    #[cfg(test)]
    pub(crate) async fn support_governed_work_for_test(&self, session_id: &SessionId) {
        let driver = {
            let sessions = self.sessions.read().await;
            Arc::clone(&sessions.get(session_id).expect("registered session").driver)
        };
        match &mut *driver.lock().await {
            DriverEntry::Ephemeral(driver) => {
                driver.set_executor_work_authorization_support(true);
            }
            DriverEntry::Persistent(driver) => {
                driver
                    .inner_mut()
                    .set_executor_work_authorization_support(true);
            }
        }
    }

    /// Reauthenticate a fresh UI request through the installed member work
    /// owner. An ungoverned runtime starts with an empty dispatch context;
    /// neither path retains the last agent run's authorization or review.
    pub fn prepare_tool_application(
        &self,
        control: Arc<meerkat_core::ToolApplicationControlRequest>,
    ) -> Result<meerkat_core::ToolDispatchContext, meerkat_core::OperationAuthorizationError> {
        control.revalidate()?;
        let Some(attachment) = self.native_work_authorization_host.get() else {
            return Ok(
                meerkat_core::ToolDispatchContext::default().with_tool_application_control(control)
            );
        };
        self.shared
            .require_governed_execution_custody()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        let authorization = attachment
            .host()
            .tool_application_authorization(Arc::clone(&control))?;
        Ok(meerkat_core::ToolDispatchContext::default()
            .with_work_authorization(Some(authorization))
            .with_tool_application_control(control))
    }

    /// Prepare through the actual installed native owner. The immutable
    /// process ingress must already carry authentic original/service evidence.
    pub fn prepare_context_append(
        &self,
        control: Arc<meerkat_core::service::SystemContextControlRequest>,
    ) -> Result<
        meerkat_core::service::PreparedSystemContextAppend,
        meerkat_core::OperationAuthorizationError,
    > {
        self.prepare_context_append_observed(control).result
    }

    /// Keep the exact owner preparation observation for control settlement,
    /// including a coherent denial. No missing owner is invented for audit.
    pub fn prepare_context_append_observed(
        &self,
        control: Arc<meerkat_core::service::SystemContextControlRequest>,
    ) -> meerkat_core::authorization::ObservedAuthorizationResult<
        meerkat_core::service::PreparedSystemContextAppend,
    > {
        let context = (|| {
            self.shared
                .require_governed_execution_custody()
                .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
            self.native_work_authorization_host
                .get()
                .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?
                .host()
                .context_control_authorization(Arc::clone(&control))
        })();
        let context = match context {
            Ok(context) => context,
            Err(error) => {
                return meerkat_core::authorization::ObservedAuthorizationResult::unobserved(Err(
                    error,
                ));
            }
        };
        meerkat_core::service::PreparedSystemContextAppend::prepare_observed(
            Arc::clone(&self.shared),
            control,
            context,
        )
    }

    pub fn owns_context_append(
        &self,
        prepared: &meerkat_core::service::PreparedSystemContextAppend,
    ) -> bool {
        self.has_native_work_authorization_host() && prepared.belongs_to(&self.shared)
    }

    /// Take the interrupted-run notices owed to `session_id`'s model, if any.
    pub(crate) async fn take_interrupted_tool_notices(
        &self,
        session_id: &SessionId,
    ) -> Option<InterruptedToolNotices> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        entry
            .interrupted_tool_notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Put back interrupted-run notices that could not be recorded yet.
    pub(crate) async fn restore_interrupted_tool_notices(
        &self,
        session_id: &SessionId,
        notices: InterruptedToolNotices,
    ) {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let mut slot = entry
                .interrupted_tool_notices
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_none() {
                *slot = Some(notices);
            }
        }
    }
}

impl InterruptedToolNotices {
    /// Group the owed notices by interrupted run: one typed
    /// `ToolProcessRecovery` notice per run, preceded by the run's user
    /// requests, with the evidence entries it settles.
    pub(crate) fn run_notices(
        &self,
    ) -> Vec<(meerkat_core::types::SystemNoticeRecord, Vec<uuid::Uuid>)> {
        let mut runs: Vec<RunId> = Vec::new();
        for owed in &self.calls {
            if !runs.contains(&owed.call.run_id) {
                runs.push(owed.call.run_id.clone());
            }
        }
        runs.into_iter()
            .map(|run_id| {
                let run_calls: Vec<&OwedInterruptedToolNotice> = self
                    .calls
                    .iter()
                    .filter(|owed| owed.call.run_id == run_id)
                    .collect();
                let blocks = run_calls
                    .iter()
                    .map(
                        |owed| meerkat_core::types::SystemNoticeBlock::ToolProcessInterrupted {
                            run_id: owed.call.run_id.clone(),
                            tool_call_id: owed.call.tool_call_id.clone(),
                            spawner: owed.call.spawner.clone(),
                            cessation: owed.call.cessation,
                            disposition: owed.disposition.clone(),
                        },
                    )
                    .collect();
                let requests = run_calls
                    .iter()
                    .find(|owed| !owed.requests.is_empty())
                    .map(|owed| {
                        owed.requests
                            .iter()
                            .map(|request| meerkat_core::types::UserMessage {
                                content: request.content.clone().into_blocks(),
                                render_metadata: request.render_metadata.clone(),
                                identity: request.identity.clone().with_run_id(run_id.clone()),
                                transcript_role: meerkat_core::types::TranscriptUserRole::default(),
                                created_at: request.created_at,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (
                    meerkat_core::types::SystemNoticeRecord {
                        requests,
                        notice: meerkat_core::types::SystemNoticeMessage::tool_process_interrupted(
                            blocks,
                        ),
                    },
                    run_calls.iter().map(|owed| owed.call.entry_id).collect(),
                )
            })
            .collect()
    }
}

/// Exact post-startup publication fence retained by a surface until its own
/// durable ownership row is committed.
///
/// The runtime loop remains mechanically pending and non-serving while the
/// same M guard is held. The surface must either activate its sidecar through
/// [`Self::commit_with`] or retire the exact attachment. Drop transfers
/// retirement to the owned cleanup runtime, so cancellation cannot publish a
/// dead host residency.
#[must_use = "dropping a retained runtime attachment aborts that exact attachment"]
pub struct CommittedRuntimeExecutorAttachmentPublicationLease {
    machine: Arc<MeerkatMachine>,
    witness: RuntimeExecutorAttachmentWitness,
    mutation_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    /// Runless terminals committed under the retained mutation guard (its
    /// pending attachment's, and recovered predecessor inputs), told once
    /// that guard is released or handed off. Declared after the guard so
    /// that it drops after it.
    runless_terminal: RunlessTerminalAnnouncement,
    cleanup_spawner: MachineCleanupTaskSpawner,
    should_wake: bool,
    persist_lifecycle_on_commit: bool,
    armed: bool,
}

/// Exact, cancellation-safe retirement intent for one committed executor
/// attachment.
///
/// The caller must acquire the service turn-finalization boundary before the
/// machine creates this lease. While armed, the retained machine mutation gate
/// prevents same-SessionId replacement before canonical unregister reaches
/// its attachment-local post-stop actor cleanup.
/// Dropping the lease starts the same exact retirement saga as [`Self::commit`]
/// so cancellation cannot strand the serving executor or its actor.
#[must_use = "dropping a prepared runtime attachment retirement starts exact retirement"]
pub struct PreparedRuntimeExecutorAttachmentRetirement {
    machine: Arc<MeerkatMachine>,
    witness: RuntimeExecutorAttachmentWitness,
    mutation_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    cleanup_spawner: MachineCleanupTaskSpawner,
    armed: bool,
}

/// Completion handle for an independently owned exact-attachment retirement.
///
/// Dropping this handle never cancels retirement. Callers that hold the
/// service turn-finalization boundary must release it before awaiting because
/// machine unregister reacquires that boundary for post-stop cleanup.
#[must_use = "drop is cancellation-safe, but await completion to observe retirement errors"]
pub struct RuntimeExecutorAttachmentRetirementCompletion {
    result_rx: crate::tokio::sync::oneshot::Receiver<Result<bool, RuntimeDriverError>>,
}

impl RuntimeExecutorAttachmentRetirementCompletion {
    fn new(
        result_rx: crate::tokio::sync::oneshot::Receiver<Result<bool, RuntimeDriverError>>,
    ) -> Self {
        Self { result_rx }
    }

    pub async fn wait(self) -> Result<bool, RuntimeDriverError> {
        self.result_rx.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned exact runtime attachment retirement ended without a result: {error}"
            ))
        })?
    }
}

impl PreparedRuntimeExecutorAttachmentRetirement {
    fn new(
        machine: Arc<MeerkatMachine>,
        witness: RuntimeExecutorAttachmentWitness,
        mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
        cleanup_spawner: MachineCleanupTaskSpawner,
    ) -> Self {
        Self {
            machine,
            witness,
            mutation_guard: Some(mutation_guard),
            cleanup_spawner,
            armed: true,
        }
    }

    pub fn witness(&self) -> &RuntimeExecutorAttachmentWitness {
        &self.witness
    }

    /// Transfer the retained mutation fence into the independently owned
    /// exact-unregister saga. This method is synchronous so the shared surface
    /// can release its service boundary before awaiting completion.
    pub fn commit(
        mut self,
    ) -> Result<RuntimeExecutorAttachmentRetirementCompletion, RuntimeDriverError> {
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "prepared exact attachment retirement lost its mutation fence".to_string(),
            )
        })?;
        let completion = self
            .machine
            .spawn_executor_attachment_retirement_with_guard(
                self.witness.clone(),
                guard,
                self.cleanup_spawner.clone(),
                UnregisterDisposition::STOP,
            );
        self.armed = false;
        Ok(completion)
    }

    /// [`Self::commit`], beginning the retirement's unregister with
    /// `queued_inputs`, its disposition for the inputs queued on the
    /// registration ([`UnregisterQueuedInputs`]). [`Self::commit`] ends them
    /// as stopped; a requeue keeps the eligible ones durably queued for the
    /// registration that replaces this one, as a graceful process shutdown
    /// does. A retirement whose unregister already began keeps the
    /// disposition it began with.
    ///
    /// # Errors
    /// `QueuedInputsRefused` for a disposition [`UnregisterQueuedInputs::check`]
    /// refuses: nothing is started, and the lease stays armed, so dropping it
    /// retires the attachment as [`Self::commit`] does.
    pub fn commit_with_queued_inputs(
        mut self,
        queued_inputs: UnregisterQueuedInputs,
    ) -> Result<RuntimeExecutorAttachmentRetirementCompletion, RuntimeDriverError> {
        let queued_inputs = queued_inputs
            .recorded()
            .map_err(RuntimeDriverError::QueuedInputsRefused)?;
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "prepared exact attachment retirement lost its mutation fence".to_string(),
            )
        })?;
        let completion = self
            .machine
            .spawn_executor_attachment_retirement_with_guard(
                self.witness.clone(),
                guard,
                self.cleanup_spawner.clone(),
                queued_inputs,
            );
        self.armed = false;
        Ok(completion)
    }

    /// Complete exact attachment retirement while the caller retains the
    /// service turn-finalization boundary that preceded this lease's M guard.
    /// The attachment-local actor cleanup therefore runs in B -> M -> R order;
    /// canonical unregister observes it complete and cannot re-enter B.
    pub async fn commit_under_runtime_turn_finalization_boundary(
        mut self,
    ) -> Result<bool, RuntimeDriverError> {
        self.machine
            .complete_executor_attachment_cleanup_under_runtime_turn_boundary(&self.witness)
            .await?;
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "prepared exact attachment retirement lost its mutation fence".to_string(),
            )
        })?;
        self.armed = false;
        self.machine
            .unregister_executor_attachment_if_current_with_guard(
                self.witness.clone(),
                RunlessTerminalAnnouncement::default(),
                guard,
                UnregisterDisposition::STOP,
            )
            .await
    }
}

impl Drop for PreparedRuntimeExecutorAttachmentRetirement {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(guard) = self.mutation_guard.take() else {
            return;
        };
        let _completion = self
            .machine
            .spawn_executor_attachment_retirement_with_guard(
                self.witness.clone(),
                guard,
                self.cleanup_spawner.clone(),
                UnregisterDisposition::STOP,
            );
        self.armed = false;
    }
}

impl std::fmt::Debug for PendingRuntimeExecutorAttachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingRuntimeExecutorAttachment")
            .field("witness", &self.witness)
            .field("should_wake", &self.should_wake)
            .field("armed", &self.armed)
            .finish()
    }
}

/// How long a start that settled an expired kept run keeps publishing the
/// run's carriers and retrying its terminal while its attachment comes to
/// serve: one deadline for the whole detached task.
const SETTLED_RUN_TERMINAL_PUBLICATION_BOUND: std::time::Duration =
    std::time::Duration::from_secs(60);

/// The runtime-termination reason a kept run settled past its stop's max age
/// carries to its interaction terminals.
const EXPIRED_KEPT_RUN_TERMINATION_REASON: &str =
    "a kept run outlived the stop's max age and was settled instead of resumed";

/// The runtime-termination reason a kept governed input with no way back to
/// its controller client, or a kept run it is the first input of, carries to
/// its interaction terminals.
const KEPT_GOVERNED_INPUT_TERMINATION_REASON: &str =
    "a kept governed input's controller cannot be supplied again without a fresh authentication";

impl PendingRuntimeExecutorAttachment {
    /// The driver and completion registry of this exact pending (non-serving)
    /// attachment, under its retained mutation guard.
    async fn exact_pending_driver(
        &self,
        operation: &str,
    ) -> Result<
        (
            Arc<crate::tokio::sync::Mutex<driver::DriverEntry>>,
            Arc<crate::tokio::sync::Mutex<crate::completion::CompletionRegistry>>,
        ),
        RuntimeDriverError,
    > {
        if self.mutation_guard.is_none() {
            return Err(RuntimeDriverError::Internal(format!(
                "pending attachment lost its mutation fence before {operation}"
            )));
        }
        let sessions = self.machine.sessions.read().await;
        let entry = sessions.get(self.witness.session_id()).ok_or_else(|| {
            RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "session {} disappeared before {operation}",
                    self.witness.session_id()
                ),
            }
        })?;
        let exact_pending = entry.epoch_id == self.witness.epoch_id
            && matches!(
                &entry.attachment_slot,
                RuntimeLoopAttachmentSlot::Pending(attachment)
                    if attachment.id == self.witness.attachment_id
                        && !attachment.wake_tx.is_closed()
                        && !attachment.effect_tx.is_closed()
            );
        if !exact_pending {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "pending attachment for session {} changed before {operation}",
                    self.witness.session_id()
                ),
            });
        }
        Ok((entry.driver.clone(), entry.completions.clone()))
    }

    /// Recovered queued inputs whose last run is one of `runs`, per run in
    /// admission order, as the typed inputs the settlement records. User
    /// requests that never reached the committed transcript (no recorded
    /// boundary: a durably applied input records the boundary that owns it)
    /// are copied with their original transcript facts, up to
    /// [`INTERRUPTED_REQUEST_CONTENT_BOUND`] bytes of content per run.
    async fn interrupted_run_inputs(
        &self,
        runs: &[RunId],
    ) -> Result<Vec<(RunId, meerkat_core::tool_process::InterruptedRunInput)>, RuntimeDriverError>
    {
        use meerkat_core::tool_process::{
            InterruptedInputKind, InterruptedRequest, InterruptedRunInput,
        };

        if runs.is_empty() {
            return Ok(Vec::new());
        }
        let (driver, _) = self.exact_pending_driver("interrupted-run lookup").await?;
        let driver = driver.lock().await;
        let mut queued: Vec<(Option<u64>, RunId, crate::input_state::StoredInputState)> =
            driver
                .as_driver()
                .active_input_ids()
                .into_iter()
                .filter_map(|input_id| {
                    let run_id = driver.input_last_run_id(&input_id)?;
                    if !runs.contains(&run_id) {
                        return None;
                    }
                    // Only queued inputs are settled (abandoned); others are left
                    // as is and not counted.
                    let stored = driver.as_driver().stored_input_state(&input_id)?;
                    (stored.seed.phase == crate::input_state::InputLifecycleState::Queued)
                        .then_some((stored.seed.admission_sequence, run_id, stored))
                })
                .collect();
        queued.sort_by_key(|(admission, _, _)| *admission);
        let mut copied: Vec<(RunId, usize)> = Vec::new();
        Ok(queued
            .into_iter()
            .map(|(_, run_id, stored)| {
                let applied = stored.seed.last_boundary_sequence.is_some();
                let stamped = stamped_turn_metadata(&stored.state);
                let (kind, request) = match stored.state.persisted_input {
                    Some(crate::input::Input::Prompt(prompt)) => {
                        let bytes = serde_json::to_vec(&prompt.content)
                            .map(|encoded| encoded.len())
                            .unwrap_or(usize::MAX);
                        let used = copied
                            .iter()
                            .find(|(run, _)| run == &run_id)
                            .map_or(0, |(_, used)| *used);
                        let fits = used
                            .checked_add(bytes)
                            .is_some_and(|total| total <= INTERRUPTED_REQUEST_CONTENT_BOUND);
                        let request = (!applied && fits).then(|| {
                            match copied.iter_mut().find(|(run, _)| run == &run_id) {
                                Some((_, used)) => *used += bytes,
                                None => copied.push((run_id.clone(), bytes)),
                            }
                            let metadata =
                                stamped.unwrap_or_else(|| prompt.turn_metadata.unwrap_or_default());
                            InterruptedRequest {
                                content: prompt.content,
                                created_at: prompt.header.timestamp,
                                identity: metadata.transcript_identity,
                                render_metadata: metadata.render_metadata,
                            }
                        });
                        (InterruptedInputKind::Prompt, request)
                    }
                    Some(crate::input::Input::Peer(_)) => (InterruptedInputKind::Peer, None),
                    Some(crate::input::Input::FlowStep(_)) => {
                        (InterruptedInputKind::FlowStep, None)
                    }
                    Some(crate::input::Input::ExternalEvent(_)) => {
                        (InterruptedInputKind::ExternalEvent, None)
                    }
                    Some(crate::input::Input::Continuation(_)) => {
                        (InterruptedInputKind::Continuation, None)
                    }
                    Some(crate::input::Input::Operation(_)) => {
                        (InterruptedInputKind::Operation, None)
                    }
                    None => (InterruptedInputKind::Unknown, None),
                };
                (run_id, InterruptedRunInput { kind, request })
            })
            .collect())
    }

    /// Settle, before this attachment serves, the recovered inputs of runs
    /// that were in flight when the previous host stopped abruptly and whose
    /// tool processes host process custody proved had started.
    ///
    /// Recovery normalizes such inputs back to `Queued` and would replay them,
    /// repeating a tool effect that may already have happened. Each recovered
    /// `Queued` input whose last run is in `runs` is abandoned through the
    /// machine's `AbandonInput` with the typed
    /// [`crate::input_state::InputAbandonReason::ToolProcessInterrupted`] and
    /// durably committed, silently (no receipt, no interaction terminal), like
    /// a member host's recovered predecessor inputs. The retained mutation
    /// guard and the pending (non-serving) slot make the driver's input set
    /// complete: no successor request can be admitted yet. Returns the
    /// abandoned inputs with their runs.
    pub async fn abandon_interrupted_run_inputs(
        &mut self,
        runs: &[RunId],
    ) -> Result<Vec<(InputId, RunId)>, RuntimeDriverError> {
        if runs.is_empty() {
            return Ok(Vec::new());
        }
        let (driver, completions) = self
            .exact_pending_driver("interrupted-run settlement")
            .await?;
        let abandoned = {
            let mut driver = driver.lock().await;
            let candidates: Vec<(InputId, RunId)> = driver
                .as_driver()
                .active_input_ids()
                .into_iter()
                .filter_map(|input_id| {
                    let run_id = driver.input_last_run_id(&input_id)?;
                    runs.contains(&run_id).then_some((input_id, run_id))
                })
                .collect();
            let mut abandoned = Vec::with_capacity(candidates.len());
            for (input_id, run_id) in candidates {
                if driver
                    .abandon_queued_input(
                        &input_id,
                        crate::input_state::InputAbandonReason::ToolProcessInterrupted,
                    )
                    .await?
                {
                    // Committed: told once this attachment releases its
                    // mutation guard, even if a later input fails here.
                    self.runless_terminal.owe();
                    abandoned.push((input_id, run_id));
                } else {
                    // Not queued: recovery did not hand it back for replay, so
                    // there is nothing to settle for it.
                    tracing::warn!(
                        session_id = %self.witness.session_id(),
                        %input_id,
                        %run_id,
                        "recovered input of an interrupted run was not queued at settlement; left as is"
                    );
                }
            }
            abandoned
        };
        if !abandoned.is_empty() {
            completions.lock().await.fail_inputs(
                abandoned
                    .iter()
                    .map(|(input_id, _)| input_id.clone())
                    .collect::<Vec<_>>(),
                crate::completion::CompletionWaitError::AttachmentReplaced,
            );
        }
        Ok(abandoned)
    }

    /// Adopt, before this attachment can serve, the terminals a store-owned
    /// recovery wrote outside this registration's driver after the driver
    /// recovered its inputs at registration
    /// (`DriverEntry::adopt_durable_consumed_inputs`). A materialization's own
    /// durable-tail recovery runs between the two: it closes the tail it
    /// finds and consumes the inputs the tail's run cursor binds, in the
    /// store. On WholeBlob nothing settles the tail before registration, so
    /// the driver still holds such an input as it recovered it (a staged input
    /// rolled back to its queue), and without this its first lap would stage
    /// the input again from that row, write over the terminal row, and run its
    /// content a second time. Runs before every other settlement here, which
    /// would otherwise end an input the store already ended. Returns the
    /// adopted inputs.
    pub(crate) async fn adopt_recovered_input_terminals(
        &mut self,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        let (driver, completions) = self
            .exact_pending_driver("recovered input terminal adoption")
            .await?;
        let adopted = driver
            .lock()
            .await
            .adopt_durable_consumed_inputs(Some(&completions))
            .await?;
        if !adopted.is_empty() {
            tracing::info!(
                session_id = %self.witness.session_id(),
                inputs = adopted.len(),
                "adopted the input terminals a recovery wrote after registration"
            );
        }
        Ok(adopted)
    }

    /// End, before this attachment can serve, every input an earlier
    /// graceful stop kept queued that is now older than that stop's max age
    /// (`crate::input_state::stop_requeue_expired` at `now`). Each is
    /// abandoned through the machine's `AbandonInput` with the typed
    /// [`crate::input_state::InputAbandonReason::RequeueExpired`], durably
    /// committed with its payload retired, so it never runs. The retained
    /// mutation guard and the pending (non-serving) slot keep the input set
    /// complete. Every attachment passes here before its serving gate, so no
    /// path serves a kept input that has not been checked. Returns the
    /// expired inputs.
    ///
    /// A run the same stop kept open (marked resumable again by this
    /// registration, its inputs still bound to it) is checked first, as a
    /// whole: when any of its inputs is over the stop's max age, the run is
    /// settled with the same reason (`DriverEntry::settle_resumable_run`)
    /// instead of resumed. Its inputs are Staged or Applied, never queued,
    /// so the per-input sweep would not see them.
    pub(crate) async fn settle_expired_stop_requeued_inputs(
        &mut self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        let (driver, completions) = self
            .exact_pending_driver("requeue expiry settlement")
            .await?;
        // The expired run whose tail repair requires the session to reload.
        let mut reload_for_run = None;
        // The expired run whose tail is repaired once the queued inputs are
        // swept.
        let mut settled_run = None;
        // The runtime-termination carriers the expired run's settlement
        // committed, and the run's own terminal, published once the driver
        // is released.
        let mut settled_batches = Vec::new();
        let mut settled_terminal = None;
        let expired = {
            let mut driver = driver.lock().await;
            let mut expired = Vec::new();
            if let Some(run_id) = driver.resumable_run_id()
                && driver.resumable_run_stop_requeue_expired(&run_id, now)
            {
                let crate::meerkat_machine::driver::SettledResumableRun {
                    abandoned: settled,
                    batches,
                    terminal,
                } = driver
                    .settle_resumable_run_with_terminals(
                        &run_id,
                        crate::input_state::InputAbandonReason::RequeueExpired,
                        EXPIRED_KEPT_RUN_TERMINATION_REASON,
                    )
                    .await?;
                settled_batches = batches;
                settled_terminal = terminal;
                tracing::info!(
                    session_id = %self.witness.session_id(),
                    %run_id,
                    inputs = settled.len(),
                    "a run a graceful stop kept open outlived the stop's max age; settled instead of resumed"
                );
                if !settled.is_empty() {
                    // Committed: told once this attachment releases its
                    // mutation guard, as for an expired queued input.
                    self.runless_terminal.owe();
                }
                expired.extend(settled);
                settled_run = Some(run_id);
            }
            let candidates =
                crate::input_state::expired_stop_requeued_inputs(driver.as_driver(), now);
            expired.reserve(candidates.len());
            for input_id in candidates {
                if driver
                    .abandon_queued_input(
                        &input_id,
                        crate::input_state::InputAbandonReason::RequeueExpired,
                    )
                    .await?
                {
                    // Committed: told once this attachment releases its
                    // mutation guard, even if a later input fails here.
                    self.runless_terminal.owe();
                    expired.push(input_id);
                } else {
                    tracing::warn!(
                        session_id = %self.witness.session_id(),
                        %input_id,
                        "an input a stop kept queued was not queued at its expiry settlement; left as is"
                    );
                }
            }
            // The settled run never resumes, so its provisional tail is
            // repaired as an interrupted run, after the abandonment
            // committed: its rows stay in the transcript, and the session's
            // next run commits its first round over the repaired body (the
            // session layer resynchronizes the seeded actor from it at its
            // next step). After the queued sweep, so an expired queued input
            // naming an earlier run is terminal by then and cannot hold the
            // repair.
            if let Some(run_id) = settled_run
                && driver
                    .repair_settled_run_tail(self.witness.session_id(), &run_id)
                    .await
            {
                reload_for_run = Some(run_id);
            }
            expired
        };
        if !expired.is_empty() {
            // As for an interrupted run's inputs: the old input is durably
            // ended, and a retry is a new request.
            completions.lock().await.fail_inputs(
                expired.clone(),
                crate::completion::CompletionWaitError::AttachmentReplaced,
            );
        }
        // A tail the repair left standing would refuse the session's next
        // run's first round commit, so the session reloads at once, and this
        // attachment does not serve. A crash right after the settlement
        // leaves the same state for the next registration's recovery.
        if let Some(run_id) = reload_for_run {
            let reason = format!("expired run {run_id}'s provisional tail still stands");
            self.machine
                .require_session_durability_reload(
                    self.witness.session_id(),
                    "expired_resumable_run_tail_repair",
                    reason.clone(),
                )
                .await;
            return Err(RuntimeDriverError::Internal(format!(
                "{reason}; the session reloads before it serves"
            )));
        }
        // The attachment serves. This runs after the runtime loop's startup
        // drain, so the carriers the settlement committed, and then the
        // run's own terminal, are published here, through the publisher the
        // attachment already carries. Detached: the session actor answers a
        // publication only once this attachment is served, and this
        // attachment is not served until this returns. A failed or missing
        // publication leaves each directed outbox as its durable retry
        // anchor for the next drain. The run's terminal is not durable, so it
        // is retried for a bounded time while the attachment comes to serve
        // (a surface may publish its exact actor witness only as the
        // attachment commits), and is lost after that.
        if !settled_batches.is_empty() || settled_terminal.is_some() {
            let publication_handle = self
                .machine
                .sessions
                .read()
                .await
                .get(self.witness.session_id())
                .and_then(RuntimeSessionEntry::publication_handle);
            let session_id = self.witness.session_id().clone();
            let driver = Arc::clone(&driver);
            let completions = Arc::clone(&completions);
            // One deadline bounds the whole task, the carriers' publication
            // included, so it ends even if that publication never answers.
            let deadline =
                meerkat_core::time_compat::Instant::now() + SETTLED_RUN_TERMINAL_PUBLICATION_BOUND;
            drop(crate::tokio::spawn(async move {
                if let Err(error) =
                    crate::control_plane::publish_and_resolve_runless_runtime_terminations_before(
                        &driver,
                        Some(&completions),
                        publication_handle.as_deref(),
                        &settled_batches,
                        EXPIRED_KEPT_RUN_TERMINATION_REASON,
                        Some(deadline),
                    )
                    .await
                {
                    tracing::warn!(
                        %session_id,
                        %error,
                        "the expired kept run's terminals were not published; their outboxes stay for the next drain"
                    );
                }
                if let Some(terminal) = settled_terminal.as_ref() {
                    crate::control_plane::publish_settled_run_terminal_before(
                        publication_handle.as_deref(),
                        terminal,
                        Some(deadline),
                    )
                    .await;
                }
            }));
        }
        Ok(expired)
    }

    /// End, before this attachment can serve, every kept governed input with
    /// no way back to its controller client: an input this registration
    /// recovered from the store that was admitted with an authority
    /// association of its own (a governed prompt, for example), not as a
    /// resume of retained work, whose host is asked for the client again. A
    /// controller client never survives the store, and the host contract can
    /// supply such an input's client again only with a fresh authentication
    /// of its caller, so it could never run: each run of it would fail before
    /// the model is called until its attempts ran out, and while it waited
    /// the whole machine would refuse controller grant changes, credential
    /// publications and auth lease releases. Each queued one is abandoned
    /// through the machine's `AbandonInput` as a refused resume
    /// ([`crate::input_state::InputAbandonReason::ResumeRefused`]), in its own
    /// durable commit with the runless runtime-termination carrier it is owed.
    ///
    /// A run this registration keeps resumable whose first input (the input
    /// whose client the resumed run executes with) is such an input is settled
    /// the same way first, as a whole, with its carriers in the same commit,
    /// instead of stopping the runtime loop when it would resume. Every
    /// attachment passes here before its serving gate, after the expired
    /// requeue settlement. Returns the ended inputs.
    pub(crate) async fn settle_kept_governed_inputs_without_controller(
        &mut self,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        let (driver, completions) = self
            .exact_pending_driver("kept governed input settlement")
            .await?;
        let mut reload_for_run = None;
        let mut settled_batches = Vec::new();
        let mut settled_terminal = None;
        let mut settled_inputs = Vec::new();
        let ended = {
            let mut driver = driver.lock().await;
            let mut ended = Vec::new();
            let mut settled_run = None;
            if let Some(run_id) = driver.resumable_run_id()
                && driver.resumed_run_first_input_lacks_controller(&run_id)
            {
                let crate::meerkat_machine::driver::SettledResumableRun {
                    abandoned,
                    batches,
                    terminal,
                } = driver
                    .settle_resumable_run_with_terminals(
                        &run_id,
                        crate::input_state::InputAbandonReason::ResumeRefused,
                        KEPT_GOVERNED_INPUT_TERMINATION_REASON,
                    )
                    .await?;
                settled_batches = batches;
                settled_terminal = terminal;
                tracing::warn!(
                    session_id = %self.witness.session_id(),
                    %run_id,
                    inputs = abandoned.len(),
                    "a kept run's first input is kept governed work whose controller cannot be supplied again; settled as a refused resume instead of resumed"
                );
                if !abandoned.is_empty() {
                    // Committed: told once this attachment releases its
                    // mutation guard.
                    self.runless_terminal.owe();
                }
                ended.extend(abandoned);
                settled_run = Some(run_id);
            }
            let candidates = driver.queued_kept_governed_inputs_without_controller();
            if !candidates.is_empty() {
                let runless_terminal = &mut self.runless_terminal;
                let outcome = driver
                    .settle_refused_queued_resumes(
                        &candidates,
                        KEPT_GOVERNED_INPUT_TERMINATION_REASON,
                        || runless_terminal.owe(),
                    )
                    .await;
                tracing::warn!(
                    session_id = %self.witness.session_id(),
                    inputs = outcome.settled.len(),
                    "kept governed inputs whose controller cannot be supplied again ended as refused resumes before the attachment serves"
                );
                ended.extend(outcome.settled.iter().map(|(input_id, _)| input_id.clone()));
                settled_inputs = outcome.settled;
                if let Some(error) = outcome.failure {
                    return Err(error);
                }
            }
            // The settled run never resumes, so its provisional tail is
            // repaired as an interrupted run's, after the abandonment
            // committed, as for an expired kept run.
            if let Some(run_id) = settled_run
                && driver
                    .repair_settled_run_tail(self.witness.session_id(), &run_id)
                    .await
            {
                reload_for_run = Some(run_id);
            }
            ended
        };
        if !ended.is_empty() {
            // The old input is durably ended; a retry is a new request with a
            // fresh authentication of its caller.
            completions.lock().await.fail_inputs(
                ended.clone(),
                crate::completion::CompletionWaitError::AttachmentReplaced,
            );
        }
        if let Some(run_id) = reload_for_run {
            let reason = format!("refused kept run {run_id}'s provisional tail still stands");
            self.machine
                .require_session_durability_reload(
                    self.witness.session_id(),
                    "refused_kept_run_tail_repair",
                    reason.clone(),
                )
                .await;
            return Err(RuntimeDriverError::Internal(format!(
                "{reason}; the session reloads before it serves"
            )));
        }
        // Published detached, through the publisher the attachment already
        // carries, as an expired kept run's are: the session actor answers a
        // publication only once this attachment is served.
        if !settled_batches.is_empty() || settled_terminal.is_some() || !settled_inputs.is_empty() {
            let publication_handle = self
                .machine
                .sessions
                .read()
                .await
                .get(self.witness.session_id())
                .and_then(RuntimeSessionEntry::publication_handle);
            let session_id = self.witness.session_id().clone();
            let driver = Arc::clone(&driver);
            let completions = Arc::clone(&completions);
            let deadline =
                meerkat_core::time_compat::Instant::now() + SETTLED_RUN_TERMINAL_PUBLICATION_BOUND;
            drop(crate::tokio::spawn(async move {
                if let Err(error) =
                    crate::control_plane::publish_and_resolve_runless_runtime_terminations_before(
                        &driver,
                        Some(&completions),
                        publication_handle.as_deref(),
                        &settled_batches,
                        KEPT_GOVERNED_INPUT_TERMINATION_REASON,
                        Some(deadline),
                    )
                    .await
                {
                    tracing::warn!(
                        %session_id,
                        %error,
                        "the refused kept run's terminals were not published; their outboxes stay for the next drain"
                    );
                }
                for (input_id, directed_owner) in &settled_inputs {
                    if let Err(error) =
                        crate::control_plane::publish_and_resolve_runless_runtime_termination_before(
                            &driver,
                            Some(&completions),
                            publication_handle.as_deref(),
                            std::slice::from_ref(input_id),
                            directed_owner.as_ref(),
                            KEPT_GOVERNED_INPUT_TERMINATION_REASON,
                            Some(deadline),
                        )
                        .await
                    {
                        tracing::warn!(
                            %session_id,
                            %input_id,
                            %error,
                            "a refused kept governed input's terminal was not published; its outbox stays for the next drain"
                        );
                    }
                }
                if let Some(terminal) = settled_terminal.as_ref() {
                    crate::control_plane::publish_settled_run_terminal_before(
                        publication_handle.as_deref(),
                        terminal,
                        Some(deadline),
                    )
                    .await;
                }
            }));
        }
        Ok(ended)
    }

    /// Settle the session's interrupted-run evidence before serving, in a
    /// crash-safe order under the retained mutation guard:
    ///
    /// 1. find the recovered inputs of every evidenced run;
    /// 2. durably mark pending evidence whose run has such inputs as settled,
    ///    with the run's input count (the request plus inputs absorbed into
    ///    the run while it ran) and its user requests, captured before the
    ///    inputs and their payloads are abandoned;
    /// 3. abandon the inputs of every settled run (idempotent: a crash between
    ///    2 and 3 abandons them at the next materialization);
    /// 4. owe the model one notice per settled run, delivered after commit
    ///    preceded by the run's user requests, which never reached the
    ///    transcript because the run never committed.
    ///
    /// Pending evidence whose run has no recovered input is moot: the run had
    /// completed. A process that merely completed in it (a completed-tool
    /// marker) is acknowledged silently; any other (for example a background
    /// job killed by recovery) is owed a notice saying the run had completed.
    async fn settle_interrupted_tool_evidence(
        &mut self,
        evidence: Arc<dyn meerkat_core::tool_process::InterruptedToolEvidence>,
    ) -> Result<(), RuntimeDriverError> {
        use meerkat_core::tool_process::{
            InterruptedRunInputs, InterruptedToolRunDisposition, InterruptedToolSettlement,
            ToolProcessCessation,
        };

        let evidence_error = |error: meerkat_core::tool_process::InterruptedToolEvidenceError| {
            RuntimeDriverError::Internal(error.to_string())
        };
        let calls = evidence.interrupted_calls().await.map_err(evidence_error)?;
        if calls.is_empty() {
            return Ok(());
        }
        let mut runs: Vec<RunId> = Vec::new();
        for call in &calls {
            if !runs.contains(&call.run_id) {
                runs.push(call.run_id.clone());
            }
        }
        let in_flight = self.interrupted_run_inputs(&runs).await?;
        let run_inputs = |run_id: &RunId| InterruptedRunInputs {
            inputs: in_flight
                .iter()
                .filter(|(run, _)| run == run_id)
                .map(|(_, input)| input.clone())
                .collect(),
        };
        // A run whose provisional tail the store holds is committed or
        // continued by durable-tail recovery (which may run after this
        // attachment, when the host builds the agent lazily). Settling it here
        // would capture and abandon inputs that recovery then commits anyway,
        // or that the resumed run still holds, so its evidence is left: a
        // resumed run's round reads it itself
        // (`MeerkatMachine::resumed_run_interrupted_tool_calls`) and
        // acknowledges it once the run has ended, and any other run's is
        // settled at a later materialization, where the run is no longer in
        // flight.
        let provisional_run = self
            .machine
            .provisional_tail_run(self.witness.session_id())
            .await?;
        let mut owed = Vec::new();
        let mut silently_moot = Vec::new();
        let mut settled_runs: Vec<RunId> = Vec::new();
        for run_id in &runs {
            if provisional_run.as_ref() == Some(run_id) {
                tracing::info!(
                    session_id = %self.witness.session_id(),
                    %run_id,
                    "interrupted run has a provisional tail; leaving its evidence to durable-tail recovery"
                );
                continue;
            }
            let run_calls: Vec<&meerkat_core::tool_process::InterruptedToolCall> =
                calls.iter().filter(|call| &call.run_id == run_id).collect();
            let recorded_inputs = run_calls
                .iter()
                .find_map(|call| call.settlement.settled_inputs().cloned());
            let settled = match recorded_inputs {
                Some(settled) => {
                    // Settled by an earlier materialization; finish it.
                    let pending: Vec<uuid::Uuid> = run_calls
                        .iter()
                        .filter(|call| call.settlement == InterruptedToolSettlement::Pending)
                        .map(|call| call.entry_id)
                        .collect();
                    if !pending.is_empty() {
                        evidence
                            .mark_inputs_settled(&pending, &settled)
                            .await
                            .map_err(evidence_error)?;
                    }
                    Some(settled)
                }
                None => {
                    let settled = run_inputs(run_id);
                    if settled.inputs.is_empty() {
                        None
                    } else {
                        // Durable before the inputs (and their payloads) are
                        // abandoned below.
                        let entries: Vec<uuid::Uuid> =
                            run_calls.iter().map(|call| call.entry_id).collect();
                        evidence
                            .mark_inputs_settled(&entries, &settled)
                            .await
                            .map_err(evidence_error)?;
                        Some(settled)
                    }
                }
            };
            match settled {
                Some(settled) => {
                    settled_runs.push(run_id.clone());
                    owed.extend(run_calls.into_iter().map(|call| OwedInterruptedToolNotice {
                        call: call.clone(),
                        disposition: InterruptedToolRunDisposition::InputsSettled {
                            inputs: settled.count(),
                            unrestored: settled.unrestored(),
                        },
                        requests: settled.requests().cloned().collect(),
                    }));
                }
                None => {
                    for call in run_calls {
                        if call.cessation == ToolProcessCessation::ExitedBeforeCommit {
                            // The run committed the tool's result normally.
                            silently_moot.push(call.entry_id);
                        } else {
                            // Owed unless the transcript already answers the
                            // call (a resumed run's round answered it before
                            // the run ended): recording the notice leaves
                            // such a call out, so its evidence is acknowledged
                            // without telling the model twice.
                            owed.push(OwedInterruptedToolNotice {
                                call: call.clone(),
                                disposition: InterruptedToolRunDisposition::RunCompleted,
                                requests: Vec::new(),
                            });
                        }
                    }
                }
            }
        }
        if !silently_moot.is_empty() {
            evidence
                .acknowledge(&silently_moot)
                .await
                .map_err(evidence_error)?;
        }
        let abandoned = self.abandon_interrupted_run_inputs(&settled_runs).await?;
        if !abandoned.is_empty() {
            tracing::warn!(
                session_id = %self.witness.session_id(),
                abandoned_inputs = abandoned.len(),
                interrupted_runs = settled_runs.len(),
                "settled recovered inputs of runs whose tool processes were interrupted by an abrupt host stop"
            );
        }
        if !owed.is_empty() {
            self.interrupted_tool_notices = Some(InterruptedToolNotices {
                evidence,
                calls: owed,
            });
            // Wake the loop at commit so the notices are recorded right away,
            // even for an idle session with no queued input.
            self.should_wake = true;
        }
        Ok(())
    }

    fn new(
        machine: Arc<MeerkatMachine>,
        witness: RuntimeExecutorAttachmentWitness,
        mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
        cleanup_spawner: MachineCleanupTaskSpawner,
        should_wake: bool,
        persist_lifecycle_on_commit: bool,
    ) -> Self {
        let runless_terminal = machine.runless_terminal_announcement();
        Self {
            machine,
            witness,
            mutation_guard: Some(mutation_guard),
            runless_terminal,
            cleanup_spawner,
            should_wake,
            persist_lifecycle_on_commit,
            armed: true,
            interrupted_tool_notices: None,
        }
    }

    pub fn witness(&self) -> &RuntimeExecutorAttachmentWitness {
        &self.witness
    }

    #[doc(hidden)]
    pub fn cleanup_task_spawner(&self) -> RuntimeCleanupTaskSpawner {
        RuntimeCleanupTaskSpawner {
            inner: self.cleanup_spawner.clone(),
        }
    }

    pub async fn commit(self) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError> {
        self.commit_with_predecessor_mode(|_| Ok(()), false).await
    }

    /// Commit this exact attachment as the successor to an explicitly retired
    /// predecessor in the same process-local runtime entry.
    ///
    /// The machine publishes the successor and, while still retaining the
    /// session mutation gate, fails waiters for every active predecessor input
    /// with
    /// [`crate::completion::CompletionWaitError::AttachmentReplaced`]. The
    /// predecessor's queued work is durably cancelled; an explicit retry is a
    /// new request owned by the successor. Fresh materialization must use
    /// [`Self::commit`] instead.
    pub async fn commit_replacing_predecessor(
        self,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError> {
        self.commit_with_predecessor_mode(|_| Ok(()), true).await
    }

    /// Commit the exact attachment and run one synchronous surface publication
    /// hook at the same linearization point, before the machine slot becomes
    /// observable as serving and before queued work is released.
    ///
    /// The hook must not call back into `MeerkatMachine`; it is intended for
    /// infallible local publication such as flipping an already-installed
    /// sidecar's active bit at the same linearization point.
    pub async fn commit_with<F>(
        self,
        on_committed: F,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>
            + Send
            + 'static,
    {
        self.commit_with_predecessor_mode(on_committed, false).await
    }

    /// Commit a replacement attachment and synchronously activate one
    /// preinstalled, witness-tagged sidecar at the serving linearization point.
    ///
    /// This is the replacement sibling of [`Self::commit_with`]. The hook must
    /// remain infallible/local and must not call back into `MeerkatMachine`;
    /// asynchronous map publication belongs outside M. Before the hook runs,
    /// every request owned by the predecessor is terminalized and fenced from
    /// the replacement attachment.
    pub async fn commit_with_replacing_predecessor<F>(
        self,
        on_committed: F,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>
            + Send
            + 'static,
    {
        self.commit_with_predecessor_mode(on_committed, true).await
    }

    async fn commit_with_predecessor_mode<F>(
        self,
        on_committed: F,
        replaces_predecessor: bool,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>
            + Send
            + 'static,
    {
        let cleanup_spawner = self.cleanup_spawner.clone();
        let completion = cleanup_spawner.spawn(async move {
            let mut pending = self;
            let result = pending
                .try_commit_with(on_committed, false, replaces_predecessor)
                .await;
            match result {
                Ok(witness) => Ok(witness),
                Err(commit_error) => match pending.abort().await {
                    Ok(()) => Err(commit_error),
                    Err(cleanup_error) => Err(RuntimeDriverError::Internal(format!(
                        "{commit_error}; exact attachment cleanup also failed: {cleanup_error}"
                    ))),
                },
            }
        });
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned runtime attachment commit ended without a result: {error}"
            ))
        })?
    }

    /// Attempt the final attachment publication while retaining this lease's
    /// exact mutation fence on rejection.
    ///
    /// This is the boundary-owned transaction seam. The caller already owns
    /// the service turn-finalization boundary, so a failed attempt can invoke
    /// [`Self::abort_under_runtime_turn_finalization_boundary`] without ever
    /// opening a B/M gap. Cancellation leaves this lease armed; its `Drop`
    /// starts ordinary exact cleanup after the caller's boundary is released.
    /// When `replaces_predecessor` is true, every active predecessor input is
    /// durably cancelled and its waiter receives
    /// [`crate::completion::CompletionWaitError::AttachmentReplaced`] before B
    /// becomes visible. No queued request is transferred between attachments.
    pub async fn try_commit_with_under_runtime_turn_finalization_boundary<F>(
        &mut self,
        replaces_predecessor: bool,
        on_committed: F,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>,
    {
        self.try_commit_with(on_committed, false, replaces_predecessor)
            .await
    }

    /// Retain durable lifecycle commit, this exact M guard, and the
    /// runtime-loop serving token for a later surface publication decision.
    /// The sidecar and runtime loop remain non-admitting until the returned
    /// lease is committed.
    pub async fn try_commit_with_retained_publication_lease_under_runtime_turn_finalization_boundary(
        &mut self,
    ) -> Result<CommittedRuntimeExecutorAttachmentPublicationLease, RuntimeDriverError> {
        let witness = self.try_commit_with(|_| Ok(()), true, false).await?;
        let mutation_guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "committed attachment lost its retained publication mutation fence".to_string(),
            )
        })?;
        // `try_commit_with` disarms Pending on success; transfer the exact
        // guard into the publication lease instead of releasing it, and with
        // it the runless terminals still owed until that guard's release.
        Ok(CommittedRuntimeExecutorAttachmentPublicationLease {
            machine: Arc::clone(&self.machine),
            witness,
            mutation_guard: Some(mutation_guard),
            runless_terminal: std::mem::take(&mut self.runless_terminal),
            cleanup_spawner: self.cleanup_spawner.clone(),
            should_wake: self.should_wake,
            persist_lifecycle_on_commit: self.persist_lifecycle_on_commit,
            armed: true,
        })
    }

    async fn try_commit_with<F>(
        &mut self,
        on_committed: F,
        retain_mutation_guard: bool,
        replaces_predecessor: bool,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>,
    {
        let witness = self
            .try_commit_publication(on_committed, retain_mutation_guard, replaces_predecessor)
            .await?;
        #[cfg(any(test, feature = "test-support"))]
        if self.interrupted_tool_notices.is_some() {
            self.machine
                .run_attachment_interrupted_notice_handoff_test_hook(witness.session_id())
                .await;
        }
        if let Some(notices) = self.interrupted_tool_notices.take() {
            // Owed to the model, recorded at the loop's next pre-dequeue
            // position so the notice joins the next real turn's transcript
            // without a turn of its own.
            let sessions = self.machine.sessions.read().await;
            if let Some(entry) = sessions.get(witness.session_id()) {
                *entry
                    .interrupted_tool_notices
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notices);
                // The publication may have woken the loop, which runs on its
                // own runtime and can spend that wake on a pre-dequeue check
                // made before this hand-off, or not woken it at all: a
                // replacement that fenced its predecessor's inputs has no
                // queued work to wake for. Wake the exact attached loop now
                // that the notices are in place, so an idle session records
                // them now, not at its next input. A retained publication's
                // slot is still pending here; its lease wakes the loop when it
                // commits, after this hand-off.
                if entry.epoch_id == witness.epoch_id
                    && let RuntimeLoopAttachmentSlot::Attached(attachment) = &entry.attachment_slot
                    && attachment.id == witness.attachment_id
                {
                    let _ = attachment.wake_tx.try_send(());
                }
            }
        }
        Ok(witness)
    }

    async fn try_commit_publication<F>(
        &mut self,
        on_committed: F,
        retain_mutation_guard: bool,
        replaces_predecessor: bool,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>,
    {
        let guard = self.mutation_guard.as_ref().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "pending runtime attachment lost its mutation fence before commit".to_string(),
            )
        })?;
        if retain_mutation_guard && replaces_predecessor {
            return Err(RuntimeDriverError::Internal(
                "predecessor request terminalization belongs to final serving publication"
                    .to_string(),
            ));
        }
        let predecessor_request_state = if replaces_predecessor {
            let sessions = self.machine.sessions.read().await;
            let entry =
                sessions
                    .get(self.witness.session_id())
                    .ok_or(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })?;
            let exact_pending = entry.epoch_id == self.witness.epoch_id
                && matches!(
                    &entry.attachment_slot,
                    RuntimeLoopAttachmentSlot::Pending(attachment)
                        if attachment.id == self.witness.attachment_id
                            && !attachment.wake_tx.is_closed()
                            && !attachment.effect_tx.is_closed()
                );
            if !exact_pending {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "replacement attachment for session {} changed before waiter fencing",
                        self.witness.session_id()
                    ),
                });
            }
            Some((
                entry.driver.clone(),
                entry.completions.clone(),
                entry.publication_handle(),
            ))
        } else {
            None
        };
        let mut predecessor_terminal_publication = None;
        if let Some((driver, completions, publication_handle)) = predecessor_request_state {
            const REPLACED_REASON: &str =
                "executor attachment was replaced before request completion";
            let (predecessor_input_ids, directed_batches) = {
                let mut driver = driver.lock().await;
                // Inputs a store-owned durable-tail recovery consumed are not
                // pending: adopt them before this sweep names its batch.
                driver
                    .adopt_durable_consumed_inputs(Some(&completions))
                    .await?;
                let predecessor_input_ids = driver.as_driver().active_input_ids();
                if predecessor_input_ids.is_empty() {
                    (predecessor_input_ids, Vec::new())
                } else {
                    // The outbox candidates and Cancelled input terminals are
                    // persisted by one driver checkpoint. Replacement may
                    // later fail, but A's requests remain validly terminal and
                    // a future attachment can replay any unpublished terminal.
                    let prepared = driver.prepare_runless_runtime_terminated_interaction_outboxes(
                        &predecessor_input_ids,
                        REPLACED_REASON.to_string(),
                    )?;
                    if let Err(error) = driver
                        .abandon_pending_inputs(crate::input_state::InputAbandonReason::Cancelled)
                        .await
                    {
                        driver.rollback_prepared_runless_interaction_terminal_outboxes(prepared);
                        return Err(error);
                    }
                    // Told once M is released or handed off, whether or not
                    // this replacement then commits.
                    self.runless_terminal.owe();
                    // Only a directed batch has a terminal to publish: its
                    // durable outbox. Every waiter fails below either way.
                    let directed_batches = crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared)
                        .into_iter()
                        .filter(crate::meerkat_machine::driver::RunlessTerminalBatch::is_directed)
                        .collect::<Vec<_>>();
                    (predecessor_input_ids, directed_batches)
                }
            };
            if !predecessor_input_ids.is_empty() {
                // M has been retained since B was installed Pending, so this
                // exact snapshot is the full A-era recipient set: no B waiter
                // can register yet. Fail only those recipients and stage
                // their already-durable terminal outboxes for publication
                // after M is released, without allowing B to report A's work
                // as a successful completion.
                completions.lock().await.fail_inputs(
                    predecessor_input_ids.iter().cloned(),
                    crate::completion::CompletionWaitError::AttachmentReplaced,
                );
                self.should_wake = false;
                if !directed_batches.is_empty() {
                    predecessor_terminal_publication =
                        Some((driver, publication_handle, directed_batches));
                }
            }
        }
        let result = self
            .machine
            .commit_pending_executor_attachment(
                &self.witness,
                guard,
                self.should_wake,
                self.persist_lifecycle_on_commit,
                retain_mutation_guard,
                on_committed,
            )
            .await;
        if result.is_ok() {
            if !retain_mutation_guard {
                self.mutation_guard.take();
                // M is released: tell the runless terminals committed
                // under it (a retained guard carries them into its lease).
                self.runless_terminal.release();
            }
            self.armed = false;
            if let Some((driver, publication_handle, directed_batches)) =
                predecessor_terminal_publication
            {
                // External publication must never pin M or delay B's serving
                // linearization. The terminal/input rows above are already
                // durable; this bounded best-effort handoff records receipts
                // after M is released. Failure leaves the exact outbox for a
                // later attachment's ordinary startup recovery.
                let _terminal_publication = self.cleanup_spawner.spawn(async move {
                    let deadline = Instant::now() + std::time::Duration::from_secs(5);
                    if let Err(error) = crate::control_plane::publish_and_resolve_runless_runtime_terminations_before(
                        &driver,
                        None,
                        publication_handle.as_deref(),
                        &directed_batches,
                        "executor attachment was replaced before request completion",
                        Some(deadline),
                    )
                    .await
                    {
                        tracing::warn!(
                            %error,
                            "replacement terminal publication remains durably pending"
                        );
                    }
                });
            }
        }
        result
    }

    pub async fn abort(mut self) -> Result<(), RuntimeDriverError> {
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "pending runtime attachment lost its mutation fence before abort".to_string(),
            )
        })?;
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        // M moves into the owned abort, and the runless terminals committed
        // under it move with it: told once the abort has released M, before
        // it awaits the teardown, even if this caller is cancelled first.
        let runless_terminal = std::mem::take(&mut self.runless_terminal);
        let completion = self.cleanup_spawner.spawn(async move {
            machine
                .abort_pending_executor_attachment(witness, runless_terminal, guard)
                .await
        });
        self.armed = false;
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned runtime attachment abort ended without a result: {error}"
            ))
        })?
    }

    /// Abort while the caller owns the stable session turn boundary.
    ///
    /// The exact mutation guard is transferred to the process-owned unregister
    /// saga. This method returns after handoff so arbitrary post-stop cleanup
    /// runs only after the caller releases B and cannot wedge that caller.
    ///
    /// Handoff is NOT settlement: the teardown may still be running, or fail,
    /// after this returns. Callers that must report settlement use
    /// [`Self::abort_under_runtime_turn_finalization_boundary_with_completion`]
    /// and await the returned completion once they have released B.
    pub async fn abort_under_runtime_turn_finalization_boundary(
        mut self,
    ) -> Result<(), RuntimeDriverError> {
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "pending runtime attachment lost its mutation fence before boundary-owned abort"
                    .to_string(),
            )
        })?;
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        let runless_terminal = std::mem::take(&mut self.runless_terminal);
        self.cleanup_spawner.spawn(async move {
            let aborted = machine
                .abort_pending_executor_attachment(witness.clone(), runless_terminal, guard)
                .await;
            if let Err(error) = aborted {
                tracing::warn!(
                    session_id = %witness.session_id(),
                    %error,
                    "process-owned boundary-aware pending attachment abort failed"
                );
            }
        });
        self.armed = false;
        Ok(())
    }

    /// Boundary-aware abort that hands back the teardown's exact completion.
    ///
    /// Identical handoff to
    /// [`Self::abort_under_runtime_turn_finalization_boundary`] — the mutation
    /// guard moves into the process-owned unregister saga and post-stop
    /// cleanup only runs once the caller releases B — except that the caller
    /// keeps a joinable completion. Await it AFTER releasing the boundary: the
    /// teardown reacquires that boundary, so awaiting while still holding it
    /// deadlocks.
    pub fn abort_under_runtime_turn_finalization_boundary_with_completion(
        mut self,
    ) -> Result<PendingExecutorAttachmentAbortCompletion, RuntimeDriverError> {
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "pending runtime attachment lost its mutation fence before boundary-owned abort"
                    .to_string(),
            )
        })?;
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        let session_id = witness.session_id().clone();
        let runless_terminal = std::mem::take(&mut self.runless_terminal);
        let completion = self.cleanup_spawner.spawn(async move {
            machine
                .abort_pending_executor_attachment(witness, runless_terminal, guard)
                .await
        });
        self.armed = false;
        Ok(PendingExecutorAttachmentAbortCompletion {
            session_id,
            completion,
        })
    }
}

/// Joinable completion for one boundary-aware pending-attachment teardown.
///
/// The teardown owns the exact mutation guard and runs in the process-owned
/// cleanup runtime, so dropping this handle does not cancel it — it only
/// discards the proof. A caller that reports settlement must
/// [`Self::wait`] instead, after releasing the turn-finalization boundary.
#[must_use = "a scheduled pending-attachment teardown is not settled until awaited"]
pub struct PendingExecutorAttachmentAbortCompletion {
    session_id: SessionId,
    completion: crate::tokio::task::JoinHandle<Result<(), RuntimeDriverError>>,
}

impl PendingExecutorAttachmentAbortCompletion {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Await the exact teardown outcome. Call only after the caller's
    /// turn-finalization boundary has been released.
    pub async fn wait(self) -> Result<(), RuntimeDriverError> {
        let session_id = self.session_id;
        self.completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned pending attachment teardown for '{session_id}' ended without a result: {error}"
            ))
        })?
    }
}

impl std::fmt::Debug for PendingExecutorAttachmentAbortCompletion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingExecutorAttachmentAbortCompletion")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl CommittedRuntimeExecutorAttachmentPublicationLease {
    pub fn witness(&self) -> &RuntimeExecutorAttachmentWitness {
        &self.witness
    }

    #[doc(hidden)]
    pub fn cleanup_task_spawner(&self) -> RuntimeCleanupTaskSpawner {
        RuntimeCleanupTaskSpawner {
            inner: self.cleanup_spawner.clone(),
        }
    }

    /// Terminalize work recovered from an interrupted predecessor before this
    /// replacement attachment is allowed to serve.
    ///
    /// This is the narrow member-host revival seam. The retained mutation gate
    /// and the pending serving barrier make the driver's active-input snapshot
    /// the complete predecessor-owned request set: no successor request can be
    /// admitted yet. The inputs are durably abandoned and any process-local
    /// waiters fail mechanically with `AttachmentReplaced`, but no interaction
    /// terminal outbox is created. A remote controller therefore follows its
    /// existing typed timeout path instead of observing a terminal fabricated
    /// by replacement attachment B.
    pub async fn abandon_recovered_predecessor_inputs(
        &mut self,
    ) -> Result<usize, RuntimeDriverError> {
        let _guard = self.mutation_guard.as_ref().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "retained attachment lost its mutation fence before predecessor abandonment"
                    .to_string(),
            )
        })?;
        let (driver, completions) = {
            let sessions = self.machine.sessions.read().await;
            let entry = sessions.get(self.witness.session_id()).ok_or_else(|| {
                RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "recovered predecessor session {} disappeared before abandonment",
                        self.witness.session_id()
                    ),
                }
            })?;
            let exact_pending = entry.epoch_id == self.witness.epoch_id
                && matches!(
                    &entry.attachment_slot,
                    RuntimeLoopAttachmentSlot::Pending(attachment)
                        if attachment.id == self.witness.attachment_id
                            && !attachment.wake_tx.is_closed()
                            && !attachment.effect_tx.is_closed()
                );
            if !exact_pending {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "recovered predecessor attachment for session {} changed before abandonment",
                        self.witness.session_id()
                    ),
                });
            }
            (entry.driver.clone(), entry.completions.clone())
        };

        let predecessor_input_ids = {
            let mut driver = driver.lock().await;
            // Inputs a store-owned durable-tail recovery consumed are not
            // pending: adopt them before this sweep names its inputs.
            driver
                .adopt_durable_consumed_inputs(Some(&completions))
                .await?;
            let predecessor_input_ids = driver.as_driver().active_input_ids();
            if predecessor_input_ids.is_empty() {
                return Ok(0);
            }
            let abandoned = driver
                .abandon_pending_inputs(crate::input_state::InputAbandonReason::Cancelled)
                .await?;
            if abandoned > 0 {
                // Told once this lease releases or hands off M, even if a
                // check below fails.
                self.runless_terminal.owe();
            }
            if abandoned != predecessor_input_ids.len() {
                return Err(RuntimeDriverError::Internal(format!(
                    "recovered predecessor abandonment for session {} terminalized {abandoned} of {} active inputs",
                    self.witness.session_id(),
                    predecessor_input_ids.len()
                )));
            }
            for input_id in &predecessor_input_ids {
                let stored = driver
                    .as_driver()
                    .stored_input_state(input_id)
                    .ok_or_else(|| {
                        RuntimeDriverError::Internal(format!(
                            "recovered predecessor input {input_id} disappeared after abandonment"
                        ))
                    })?;
                if !matches!(
                    stored.seed.terminal_outcome,
                    Some(crate::input_state::InputTerminalOutcome::Abandoned {
                        reason: crate::input_state::InputAbandonReason::Cancelled,
                    })
                ) || stored.state.interaction_terminal_outbox.is_some()
                {
                    return Err(RuntimeDriverError::Internal(format!(
                        "recovered predecessor input {input_id} did not reach silent Cancelled abandonment"
                    )));
                }
            }
            predecessor_input_ids
        };
        let abandoned = predecessor_input_ids.len();
        completions.lock().await.fail_inputs(
            predecessor_input_ids,
            crate::completion::CompletionWaitError::AttachmentReplaced,
        );
        Ok(abandoned)
    }

    /// Publish the surface's final local state while the exact pending
    /// attachment still owns M, then make the attachment serving.
    pub async fn commit_with<F>(
        mut self,
        on_committed: F,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>,
    {
        let guard = self.mutation_guard.as_ref().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "retained attachment publication lost its mutation fence".to_string(),
            )
        })?;
        let witness = self
            .machine
            .commit_retained_executor_attachment_publication(
                &self.witness,
                guard,
                self.should_wake,
                self.persist_lifecycle_on_commit,
                move |witness, _| on_committed(witness),
            )
            .await?;
        self.mutation_guard.take();
        self.runless_terminal.release();
        self.armed = false;
        Ok(witness)
    }

    /// Atomically publish the exact host residency and a surface sidecar at
    /// the retained M linearization point, then make the runtime loop serving.
    pub async fn commit_with_member_residency<F>(
        &mut self,
        residency_update: MemberResidencyUpdate,
        incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        tracked_turn_journal: Option<Arc<dyn crate::member_observation::TrackedTurnJournal>>,
        on_committed: F,
    ) -> Result<(RuntimeExecutorAttachmentWitness, MemberResidencyPublication), RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>,
    {
        residency_update.validate_for_retained_attachment(
            &self.machine,
            &self.witness,
            &incarnation,
        )?;
        let guard = self.mutation_guard.as_ref().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "retained host publication lost its mutation fence".to_string(),
            )
        })?;
        let mut staged_residency_publication = None;
        let commit = self
            .machine
            .commit_retained_executor_attachment_publication(
                &self.witness,
                guard,
                self.should_wake,
                self.persist_lifecycle_on_commit,
                |witness, session_mutation_gate| {
                    on_committed(witness)?;
                    staged_residency_publication =
                        Some(residency_update.stage_under_retained_mutation_gate(
                            incarnation,
                            tracked_turn_journal,
                            session_mutation_gate,
                        )?);
                    Ok(())
                },
            )
            .await;
        let witness = match commit {
            Ok(witness) => witness,
            Err(error) => {
                // Roll residency back to VacantPlaced while this lease still
                // retains the exact M guard. The staged value also retains the
                // stable residency-slot guard until rollback is complete.
                drop(staged_residency_publication);
                return Err(error);
            }
        };
        let residency_publication = staged_residency_publication
            .take()
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "retained host publication for session {} committed without residency",
                    self.witness.session_id()
                ))
            })?
            .finalize()?;
        self.mutation_guard.take();
        self.runless_terminal.release();
        self.armed = false;
        Ok((witness, residency_publication))
    }

    /// Transfer this exact retained attachment into the canonical unregister
    /// saga without waiting for service-owned post-stop cleanup.
    ///
    /// A caller that already owns the service turn-finalization boundary must
    /// release that boundary before awaiting the returned completion. The
    /// machine first commits Draining and terminalizes the executor; only then
    /// does its attachment-local cleanup callback reacquire the service
    /// boundary and remove the actor/sidecar assembly.
    pub fn begin_abort(
        mut self,
    ) -> Result<RuntimeExecutorAttachmentRetirementCompletion, RuntimeDriverError> {
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "committed publication lease lost its mutation fence before abort".to_string(),
            )
        })?;
        // M moves into the owned retirement, and the runless terminals
        // committed under it move with it: told once the retirement has
        // released M, before it awaits the teardown, not when this lease
        // returns.
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        let runless_terminal = std::mem::take(&mut self.runless_terminal);
        let (result_tx, result_rx) = crate::tokio::sync::oneshot::channel();
        self.cleanup_spawner.spawn(async move {
            let retired = machine
                .unregister_executor_attachment_if_current_with_guard(
                    witness.clone(),
                    runless_terminal,
                    guard,
                    UnregisterDisposition::STOP,
                )
                .await;
            if let Err(error) = &retired {
                tracing::warn!(
                    session_id = %witness.session_id(),
                    %error,
                    "owned exact runtime attachment retirement failed"
                );
            }
            let _ = result_tx.send(retired);
        });
        self.armed = false;
        Ok(RuntimeExecutorAttachmentRetirementCompletion::new(
            result_rx,
        ))
    }

    /// Retire this exact retained attachment while the surface retains B.
    pub async fn abort_under_runtime_turn_finalization_boundary(
        mut self,
    ) -> Result<bool, RuntimeDriverError> {
        self.machine
            .complete_executor_attachment_cleanup_under_runtime_turn_boundary(&self.witness)
            .await?;
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "committed publication lease lost its mutation fence before abort".to_string(),
            )
        })?;
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        let runless_terminal = std::mem::take(&mut self.runless_terminal);
        let completion = self.cleanup_spawner.spawn(async move {
            machine
                .unregister_executor_attachment_if_current_with_guard(
                    witness,
                    runless_terminal,
                    guard,
                    UnregisterDisposition::STOP,
                )
                .await
        });
        self.armed = false;
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned committed attachment abort ended without a result: {error}"
            ))
        })?
    }
}

impl Drop for CommittedRuntimeExecutorAttachmentPublicationLease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(guard) = self.mutation_guard.take() else {
            return;
        };
        self.armed = false;
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        let runless_terminal = std::mem::take(&mut self.runless_terminal);
        self.cleanup_spawner.spawn(async move {
            let retired = machine
                .unregister_executor_attachment_if_current_with_guard(
                    witness.clone(),
                    runless_terminal,
                    guard,
                    UnregisterDisposition::STOP,
                )
                .await;
            if let Err(error) = retired {
                tracing::warn!(
                    session_id = %witness.session_id(),
                    %error,
                    "drop-time committed attachment publication abort failed"
                );
            }
        });
    }
}

impl Drop for PendingRuntimeExecutorAttachment {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(guard) = self.mutation_guard.take() else {
            return;
        };
        self.armed = false;
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        let runless_terminal = std::mem::take(&mut self.runless_terminal);
        self.cleanup_spawner.spawn(async move {
            let aborted = machine
                .abort_pending_executor_attachment(witness.clone(), runless_terminal, guard)
                .await;
            if let Err(error) = aborted {
                tracing::warn!(
                    session_id = %witness.session_id(),
                    %error,
                    "drop-time exact runtime attachment abort failed"
                );
            }
        });
    }
}

impl std::fmt::Debug for PreparedSessionMaterialization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSessionMaterialization")
            .field("session_id", self.bindings.session_id())
            .field("epoch_id", self.bindings.epoch_id())
            .field("claim_id", &self.claim_id)
            .field("armed", &self.armed)
            .finish()
    }
}

impl std::fmt::Debug for PreparedAttachedSessionActorRecovery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedAttachedSessionActorRecovery")
            .field("session_id", self.bindings.session_id())
            .field("epoch_id", self.bindings.epoch_id())
            .field("witness", &self.witness)
            .field("armed", &self.armed)
            .finish()
    }
}

impl PreparedAttachedSessionActorRecovery {
    fn new(
        bindings: meerkat_core::SessionRuntimeBindings,
        witness: RuntimeExecutorAttachmentWitness,
        claim_id: uuid::Uuid,
        claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        previous_phase: crate::RuntimeActorMaterializationClaimPhase,
        mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Self {
        Self {
            bindings,
            witness,
            claim_id,
            claim_state,
            previous_phase,
            mutation_guard: Some(mutation_guard),
            armed: true,
        }
    }

    pub fn bindings(&self) -> &meerkat_core::SessionRuntimeBindings {
        &self.bindings
    }

    pub fn bindings_clone(&self) -> meerkat_core::SessionRuntimeBindings {
        self.bindings.clone()
    }

    pub fn witness(&self) -> &RuntimeExecutorAttachmentWitness {
        &self.witness
    }

    /// Replace the retained terminal-publication capability for the actor
    /// reconstructed under this exact serving attachment.
    ///
    /// Publication handles deliberately snapshot one service-actor
    /// incarnation. Actor-only recovery therefore has to install B's handle
    /// while this lease still holds the attachment's M; an already-issued A
    /// handle must remain fenced to A and can only fail closed after A is
    /// revoked. The durable outbox then retries through the newly retained B
    /// handle without resolving a live actor by logical session id.
    pub async fn replace_publication_handle_for_recovered_actor(
        &self,
        publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    ) -> Result<(), RuntimeDriverError> {
        if !self.armed || self.mutation_guard.is_none() {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "attached actor-recovery lease is no longer active".to_string(),
            });
        }
        let machine =
            self.witness
                .machine
                .upgrade()
                .ok_or_else(|| RuntimeDriverError::StaleAuthority {
                    reason: "attached actor-recovery machine no longer exists".to_string(),
                })?;
        let mut sessions = machine.sessions.write().await;
        let entry =
            sessions
                .get_mut(self.witness.session_id())
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
        let exact_attachment = entry.epoch_id == self.witness.epoch_id
            && Arc::ptr_eq(&entry.materialization_claim_state, &self.claim_state)
            && entry.generated_executor_registration_active()
            && matches!(
                &entry.attachment_slot,
                RuntimeLoopAttachmentSlot::Attached(attachment)
                    if attachment.id == self.witness.attachment_id
                        && !attachment.wake_tx.is_closed()
                        && !attachment.effect_tx.is_closed()
            );
        let exact_actor_commit = self
            .claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .exact_claim_is(
                self.claim_id,
                &[crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit],
            );
        if !exact_attachment || !exact_actor_commit {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "actor recovery for session {} lost its exact attachment before publication capability replacement",
                    self.bindings.session_id()
                ),
            });
        }
        entry.publication_handle = Some(publication_handle);
        Ok(())
    }

    /// Commit successful actor reconstruction while preserving the same exact
    /// serving executor attachment.
    pub fn commit_actor(&mut self) -> Result<(), RuntimeDriverError> {
        if !self.armed {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "attached actor-recovery lease is no longer active".to_string(),
            });
        }
        let changed = {
            let mut state = self
                .claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.exact_claim_is(
                self.claim_id,
                &[crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit],
            ) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "actor recovery for session {} did not retain its exact materialization claim",
                        self.bindings.session_id()
                    ),
                });
            }
            state.current = None;
            state.phase = crate::RuntimeActorMaterializationClaimPhase::RetainedActor;
            Arc::clone(&state.changed)
        };
        changed.notify_waiters();
        self.armed = false;
        self.mutation_guard.take();
        Ok(())
    }
}

impl Drop for PreparedAttachedSessionActorRecovery {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let changed = {
            let mut state = self
                .claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.current == Some(self.claim_id) {
                state.current = None;
                state.phase = if state.phase
                    == crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit
                {
                    crate::RuntimeActorMaterializationClaimPhase::RetainedActor
                } else {
                    self.previous_phase
                };
                Some(Arc::clone(&state.changed))
            } else {
                None
            }
        };
        if let Some(changed) = changed {
            changed.notify_waiters();
        }
        self.armed = false;
        self.mutation_guard.take();
    }
}

impl PreparedSessionMaterialization {
    fn new(
        machine: Arc<MeerkatMachine>,
        bindings: meerkat_core::SessionRuntimeBindings,
        claim_id: uuid::Uuid,
        inserted_registration: bool,
        cleanup_spawner: MachineCleanupTaskSpawner,
    ) -> Result<Self, RuntimeDriverError> {
        let authority = bindings
            .__runtime_authority()
            .downcast_ref::<crate::SessionRuntimeBindingsAuthority>()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "prepared materialization lacks MeerkatMachine authority".to_string(),
            })?;
        if authority.materialization_claim_id != Some(claim_id) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "prepared materialization binding lost its exact claim".to_string(),
            });
        }
        let claim_state = Arc::clone(&authority.materialization_claim_state);
        Ok(Self {
            machine,
            bindings,
            claim_id,
            claim_state,
            inserted_registration,
            cleanup_spawner,
            archived_resume_authorization_issued: false,
            armed: true,
        })
    }

    pub fn bindings(&self) -> &meerkat_core::SessionRuntimeBindings {
        &self.bindings
    }

    /// Whether this materialization's own prepare inserted the session's
    /// registration. `false` when it found a registration already current,
    /// such as one a restore prepared before this revival: rolling this
    /// materialization back leaves that registration in place. Fixed at the
    /// prepare; an executor attaching later does not change it.
    pub fn inserted_registration(&self) -> bool {
        self.inserted_registration
    }

    pub fn bindings_clone(&self) -> meerkat_core::SessionRuntimeBindings {
        self.bindings.clone()
    }

    pub fn session_id(&self) -> &SessionId {
        self.bindings.session_id()
    }

    #[doc(hidden)]
    pub fn cleanup_task_spawner(&self) -> RuntimeCleanupTaskSpawner {
        RuntimeCleanupTaskSpawner {
            inner: self.cleanup_spawner.clone(),
        }
    }

    /// Install the reverse composition endpoint using this exact exclusive
    /// materialization claim. Routed placement inputs cannot acquire endpoint
    /// custody: an old consumer must never claim a same-ID replacement entry.
    /// This is mechanical delivery custody, not a placement or readiness verdict.
    pub async fn install_composition_signal_dispatcher(
        &self,
        dispatcher: composition::MeerkatCompositionSignalDispatcher,
    ) -> Result<(), RuntimeDriverError> {
        let _mutation_guard = self
            .machine
            .lock_current_durability_ready_session_mutation_gate(self.session_id())
            .await?;
        let authority = crate::validated_session_runtime_bindings_authority(&self.bindings)
            .map_err(|error| RuntimeDriverError::StaleAuthority {
                reason: error.to_string(),
            })?;
        let mut sessions = self.machine.sessions.write().await;
        let entry = sessions.get_mut(self.session_id()).ok_or_else(|| {
            RuntimeDriverError::StaleAuthority {
                reason: "composition materialization registration disappeared".to_string(),
            }
        })?;
        if !self.armed
            || entry.epoch_id != *self.bindings.epoch_id()
            || !Arc::ptr_eq(&entry.materialization_claim_state, &self.claim_state)
            || !Arc::ptr_eq(&entry.dsl_authority, &authority.dsl_authority)
            || !Arc::ptr_eq(&entry.handle_teardown_gate, &authority.teardown_gate)
            || entry.physical_attachment_is_live()
            || !self
                .claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .exact_claim_is(
                    self.claim_id,
                    &[
                        crate::RuntimeActorMaterializationClaimPhase::Prepared,
                        crate::RuntimeActorMaterializationClaimPhase::Staged,
                    ],
                )
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "composition endpoint requires the exact unserved materialization claim"
                    .to_string(),
            });
        }
        if let Some(error) = entry.dsl_mutation_blocked_by_unregister(self.session_id()) {
            return Err(error);
        }
        if entry
            .composition_signal_dispatcher
            .as_ref()
            .is_some_and(|current| {
                !Arc::ptr_eq(current, &dispatcher) && current.transport_is_closed() != Some(true)
            })
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "prepared composition cannot replace an open or unknown delivery owner"
                    .into(),
            });
        }
        entry.composition_signal_dispatcher = Some(dispatcher);
        entry.composition_materialization_claim_id = Some(self.claim_id);
        Ok(())
    }

    /// Consume an exact preparation claim for an unbound recovery endpoint.
    /// Generated binding absence is checked in the same critical section as
    /// installation, so a concurrent placement cannot change the target.
    pub async fn commit_unbound_composition_endpoint(
        &mut self,
        dispatcher: composition::MeerkatCompositionSignalDispatcher,
    ) -> Result<(), RuntimeDriverError> {
        let _guard = self
            .machine
            .lock_current_durability_ready_session_mutation_gate(self.session_id())
            .await?;
        let authority = crate::validated_session_runtime_bindings_authority(&self.bindings)
            .map_err(|error| RuntimeDriverError::StaleAuthority {
                reason: error.to_string(),
            })?;
        let mut sessions = self.machine.sessions.write().await;
        let entry = sessions.get_mut(self.session_id()).ok_or_else(|| {
            RuntimeDriverError::StaleAuthority {
                reason: "composition recovery registration disappeared".into(),
            }
        })?;
        if let Some(error) = entry.dsl_mutation_blocked_by_unregister(self.session_id()) {
            return Err(error);
        }
        // Synchronous actor materialization also takes DSL before claim.
        let dsl = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut claim = entry
            .materialization_claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = dsl.state();
        if !self.armed
            || entry.epoch_id != *self.bindings.epoch_id()
            || !Arc::ptr_eq(&entry.materialization_claim_state, &self.claim_state)
            || !Arc::ptr_eq(&entry.dsl_authority, &authority.dsl_authority)
            || !Arc::ptr_eq(&entry.handle_teardown_gate, &authority.teardown_gate)
            || entry.physical_attachment_is_live()
            || entry.provisional_materialization_claim_id.is_some()
            || state.active_runtime_id.is_some()
            || state.active_fence_token.is_some()
            || state.active_runtime_generation.is_some()
            || dispatcher.transport_is_closed() != Some(false)
            || entry
                .composition_signal_dispatcher
                .as_ref()
                .is_some_and(|current| {
                    !Arc::ptr_eq(current, &dispatcher)
                        && current.transport_is_closed() != Some(true)
                })
            || !claim.exact_claim_is(
                self.claim_id,
                &[crate::RuntimeActorMaterializationClaimPhase::Prepared],
            )
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "composition recovery requires its exact unbound prepared claim and delivery custody".into(),
            });
        }
        entry.composition_signal_dispatcher = Some(dispatcher);
        entry.composition_materialization_claim_id = None;
        claim.current = None;
        claim.phase = crate::RuntimeActorMaterializationClaimPhase::Vacant;
        claim.rollback_registration_available = false;
        claim.changed.notify_waiters();
        self.armed = false;
        Ok(())
    }

    /// Whether this lease still owns the exact process-local materialization
    /// claim that fences same-session actor creation and executor attachment.
    #[must_use]
    pub async fn owns_current_materialization_claim(&self) -> bool {
        self.registration_witness().await.is_some()
    }

    /// The exact registration this materialization claim owns, as its opaque
    /// witness; `None` once the claim no longer owns the current entry. The
    /// mutation gate is released before returning, so consumers revalidate the
    /// witness at use and never substitute a session-id lookup for it.
    pub async fn registration_witness(&self) -> Option<RuntimeSessionRegistrationWitness> {
        if !self.armed {
            return None;
        }
        let _mutation_guard = self
            .machine
            .lock_current_durability_ready_session_mutation_gate(self.session_id())
            .await
            .ok()?;
        let authority = crate::validated_session_runtime_bindings_authority(&self.bindings).ok()?;
        let sessions = self.machine.sessions.read().await;
        sessions
            .get(self.session_id())
            .filter(|entry| {
                entry.epoch_id == *self.bindings.epoch_id()
                    && Arc::ptr_eq(&entry.materialization_claim_state, &self.claim_state)
                    && Arc::ptr_eq(&entry.dsl_authority, &authority.dsl_authority)
                    && Arc::ptr_eq(&entry.handle_teardown_gate, &authority.teardown_gate)
                    && !entry.physical_attachment_is_live()
                    && self
                        .claim_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .exact_claim_is(
                            self.claim_id,
                            &[
                                crate::RuntimeActorMaterializationClaimPhase::Prepared,
                                crate::RuntimeActorMaterializationClaimPhase::Staged,
                                crate::RuntimeActorMaterializationClaimPhase::ActorCreating,
                                crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit,
                                crate::RuntimeActorMaterializationClaimPhase::Aborting,
                            ],
                        )
            })
            .map(|entry| {
                RuntimeSessionRegistrationWitness::new(
                    Arc::downgrade(&self.machine.shared),
                    self.session_id().clone(),
                    entry.epoch_id.clone(),
                    entry.mutation_gate.downgrade(),
                )
            })
    }

    /// Prove that one atomic resume observation is the current durable
    /// lifecycle realization owned by this exact Prepared claim.
    ///
    /// Session/document authority remains the caller's concern. The runtime
    /// owns lifecycle normalization, so callers must not duplicate its state
    /// transition table in shell code. This check retains the machine mutation
    /// gate from the exact in-process claim proof through the store read.
    #[must_use]
    pub async fn owns_resume_lifecycle_transition(
        &self,
        expected: &crate::store::RuntimeSessionResumeObservation,
        current: &crate::store::RuntimeSessionResumeObservation,
    ) -> bool {
        if !self.armed
            || expected.runtime_id() != current.runtime_id()
            || current.runtime_id()
                != &crate::identifiers::LogicalRuntimeId::for_session(self.session_id())
        {
            return false;
        }
        let expected_supervisor = match expected.lifecycle() {
            crate::store::MachineLifecycleObservation::Missing => {
                &crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt
            }
            crate::store::MachineLifecycleObservation::Decoded { record, .. }
                if matches!(
                    record.runtime_state(),
                    Some(
                        RuntimeState::Idle
                            | RuntimeState::Attached
                            | RuntimeState::Running
                            | RuntimeState::Stopped
                            | RuntimeState::Retired
                    )
                ) =>
            {
                record.supervisor_authority()
            }
            crate::store::MachineLifecycleObservation::Decoded { .. }
            | crate::store::MachineLifecycleObservation::Unsupported { .. }
            | crate::store::MachineLifecycleObservation::Malformed { .. } => return false,
        };
        let crate::store::MachineLifecycleObservation::Decoded { record, .. } = current.lifecycle()
        else {
            return false;
        };
        let prepared_epoch = self.bindings.epoch_id().to_string();
        if record.runtime_state() != Some(RuntimeState::Idle)
            || record.binding().agent_runtime_id().is_some()
            || record.binding().fence_token().is_some()
            || record.binding().runtime_generation().is_some()
            || record
                .binding()
                .runtime_epoch_id()
                .is_some_and(|epoch| epoch != prepared_epoch)
            || record.run().current_run_id().is_some()
            || record.run().pre_run_phase().is_some()
            || record.unregister_progress().is_some()
            || record.supervisor_authority() != expected_supervisor
        {
            return false;
        }
        let Some(_mutation_guard) = self
            .machine
            .lock_current_durability_ready_session_mutation_gate(self.session_id())
            .await
            .ok()
        else {
            return false;
        };
        let Ok(authority) = crate::validated_session_runtime_bindings_authority(&self.bindings)
        else {
            return false;
        };
        let claim_is_current = {
            let sessions = self.machine.sessions.read().await;
            sessions.get(self.session_id()).is_some_and(|entry| {
                entry.epoch_id == *self.bindings.epoch_id()
                    && Arc::ptr_eq(&entry.materialization_claim_state, &self.claim_state)
                    && Arc::ptr_eq(&entry.dsl_authority, &authority.dsl_authority)
                    && Arc::ptr_eq(&entry.handle_teardown_gate, &authority.teardown_gate)
                    && !entry.physical_attachment_is_live()
                    && crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(
                        &entry
                            .dsl_authority
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                    ) == RuntimeState::Idle
                    && self
                        .claim_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .exact_claim_is(
                            self.claim_id,
                            &[
                                crate::RuntimeActorMaterializationClaimPhase::Prepared,
                                crate::RuntimeActorMaterializationClaimPhase::Staged,
                                crate::RuntimeActorMaterializationClaimPhase::ActorCreating,
                                crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit,
                                crate::RuntimeActorMaterializationClaimPhase::Aborting,
                            ],
                        )
            })
        };
        if !claim_is_current {
            return false;
        }
        let Some(store) = self.machine.store.as_ref() else {
            return false;
        };
        store
            .load_session_resume_observation(current.runtime_id())
            .await
            .is_ok_and(|observed| &observed == current)
    }

    /// Mint the non-cloneable authorization for actor construction at an
    /// exact Archived+Retired revival midpoint.
    ///
    /// The returned token is useful only while this prepared lease still owns
    /// the same claim. Consumption revalidates the current machine entry,
    /// provisional cleanup installation, and exact Retired phase under the
    /// session mutation gate.
    pub fn archived_resume_authorization(
        &mut self,
    ) -> Result<ArchivedSessionActorMaterializationAuthorization, RuntimeDriverError> {
        if !self.armed {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "prepared materialization for session {} no longer owns archived-resume authority",
                    self.session_id()
                ),
            });
        }
        if self.archived_resume_authorization_issued {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "prepared materialization for session {} already issued its one-shot archived-resume authorization",
                    self.session_id()
                ),
            });
        }
        let authority = self
            .bindings
            .__runtime_authority()
            .downcast_ref::<crate::SessionRuntimeBindingsAuthority>()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "prepared archived-resume materialization lacks machine authority"
                    .to_string(),
            })?;
        if authority.materialization_claim_id != Some(self.claim_id)
            || !Arc::ptr_eq(&authority.materialization_claim_state, &self.claim_state)
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "prepared archived-resume claim for session {} is no longer exact",
                    self.session_id()
                ),
            });
        }
        let authorization = ArchivedSessionActorMaterializationAuthorization {
            machine: Arc::clone(&self.machine),
            session_id: self.session_id().clone(),
            epoch_id: self.bindings.epoch_id().clone(),
            claim_id: self.claim_id,
            claim_state: Arc::clone(&self.claim_state),
            dsl_authority: Arc::clone(&authority.dsl_authority),
            teardown_gate: Arc::clone(&authority.teardown_gate),
        };
        self.archived_resume_authorization_issued = true;
        Ok(authorization)
    }

    /// Acquire the exact post-actor commit lease for archived revival.
    /// Actor construction must already have consumed the one-shot
    /// authorization and committed the exact claim into
    /// `ActorMaterializedPendingCommit`.
    pub async fn acquire_archived_resume_commit_lease(
        &mut self,
    ) -> Result<PreparedArchivedResumeCommitLease, RuntimeDriverError> {
        if !self.armed || !self.archived_resume_authorization_issued {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "prepared materialization for session {} does not own post-actor archived-resume authority",
                    self.session_id()
                ),
            });
        }
        let mutation_guard = self
            .machine
            .lock_current_durability_ready_session_mutation_gate(self.session_id())
            .await
            .map_err(|error| match error {
                RuntimeDriverError::NotReady { .. } => RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "archived-resume session {} disappeared before commit lease acquisition",
                        self.session_id()
                    ),
                },
                error => error,
            })?;
        let exact = {
            let sessions = self.machine.sessions.read().await;
            sessions.get(self.session_id()).is_some_and(|entry| {
                entry.epoch_id == *self.bindings.epoch_id()
                    && Arc::ptr_eq(&entry.materialization_claim_state, &self.claim_state)
                    && !entry.physical_attachment_is_live()
                    && matches!(
                        entry.control_snapshot().phase,
                        RuntimeState::Retired | RuntimeState::Idle
                    )
                    && self
                        .claim_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .exact_claim_is(
                            self.claim_id,
                            &[crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit],
                        )
            })
        };
        if !exact {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "archived-resume session {} lost its exact quiescent post-actor claim",
                    self.session_id()
                ),
            });
        }
        Ok(PreparedArchivedResumeCommitLease {
            machine: Arc::clone(&self.machine),
            session_id: self.session_id().clone(),
            epoch_id: self.bindings.epoch_id().clone(),
            claim_id: self.claim_id,
            claim_state: Arc::clone(&self.claim_state),
            _mutation_guard: mutation_guard,
        })
    }

    /// Attach the executor that consumes this exact actor-materialization
    /// claim. Generic machine attachment is intentionally unable to cross an
    /// outstanding prepare/actor-create transaction; this method transfers
    /// that authority to the returned pending attachment lease.
    pub async fn ensure_executor_attachment<F>(
        &mut self,
        executor_factory: F,
    ) -> Result<EnsureRuntimeExecutorAttachment, RuntimeDriverError>
    where
        F: FnOnce(
                RuntimeExecutorAttachmentWitness,
            ) -> Box<dyn meerkat_core::lifecycle::CoreExecutor>
            + Send
            + 'static,
    {
        self.ensure_executor_attachment_with_boundary_mode(executor_factory, false)
            .await
    }

    /// Attach the executor while the shared session facade already owns the
    /// service turn-finalization boundary.
    ///
    /// Startup rejection must use the non-reentrant post-stop cleanup path in
    /// this mode. Otherwise an unserved attachment can begin ordinary cleanup
    /// while retaining M and deadlock trying to reacquire the caller's B.
    #[doc(hidden)]
    pub async fn ensure_executor_attachment_under_runtime_turn_finalization_boundary<F>(
        &mut self,
        executor_factory: F,
    ) -> Result<EnsureRuntimeExecutorAttachment, RuntimeDriverError>
    where
        F: FnOnce(
                RuntimeExecutorAttachmentWitness,
            ) -> Box<dyn meerkat_core::lifecycle::CoreExecutor>
            + Send
            + 'static,
    {
        self.ensure_executor_attachment_with_boundary_mode(executor_factory, true)
            .await
    }

    async fn ensure_executor_attachment_with_boundary_mode<F>(
        &mut self,
        executor_factory: F,
        turn_finalization_boundary_already_held: bool,
    ) -> Result<EnsureRuntimeExecutorAttachment, RuntimeDriverError>
    where
        F: FnOnce(
                RuntimeExecutorAttachmentWitness,
            ) -> Box<dyn meerkat_core::lifecycle::CoreExecutor>
            + Send
            + 'static,
    {
        if !self.armed {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "prepared materialization for session {} no longer owns attachment authority",
                    self.session_id()
                ),
            });
        }
        let expected_claim = RuntimeExecutorAttachmentMaterializationClaim {
            claim_id: self.claim_id,
            claim_state: Arc::clone(&self.claim_state),
            epoch_id: self.bindings.epoch_id().clone(),
        };
        let outcome = self
            .machine
            .ensure_session_with_executor_factory_for_materialization(
                self.session_id().clone(),
                expected_claim,
                turn_finalization_boundary_already_held,
                executor_factory,
            )
            .await?;
        if matches!(&outcome, EnsureRuntimeExecutorAttachment::Pending(_)) {
            // The exact machine attachment now owns rollback. Its publication
            // cleared this claim synchronously, so dropping this shell must not
            // start a second cleanup saga. Interrupted-run evidence was settled
            // when the machine created the pending attachment.
            self.armed = false;
        }
        Ok(outcome)
    }

    fn request_abort(&self) -> bool {
        let mut state = self
            .claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.exact_claim_is(
            self.claim_id,
            &[
                crate::RuntimeActorMaterializationClaimPhase::Prepared,
                crate::RuntimeActorMaterializationClaimPhase::Staged,
                crate::RuntimeActorMaterializationClaimPhase::ActorCreating,
                crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit,
                crate::RuntimeActorMaterializationClaimPhase::Aborting,
            ],
        ) {
            return false;
        }
        state.phase = crate::RuntimeActorMaterializationClaimPhase::Aborting;
        true
    }

    /// Roll back this exact materialization now. A pre-existing runtime entry is
    /// preserved after its exact provisional cleanup; an entry inserted for this
    /// transaction is fully unregistered with the Keep disposition: every
    /// non-terminal input its registration recovered from the store stays
    /// exactly as the store holds it, with no requeue mark, for the next
    /// registration to recover again. A refused materialization (a durable tail
    /// held for recovery, a failed agent build) so ends nothing a later start
    /// would run. An input admitted while the registration stood still ends as
    /// stopped.
    ///
    /// The machine decides whether the keep stands: a registration that
    /// served live work (an input admitted to it, a resubmission
    /// deduplicated onto an input it holds, or a host's recycle, whose
    /// recovery mixes live inputs into the recovered ones) unregisters with
    /// Stop instead and ends every non-terminal input, so a turn whose caller
    /// saw the materialization refused never runs on a later start.
    pub async fn rollback_now(&mut self) -> Result<bool, RuntimeDriverError> {
        self.rollback_now_with_turn_finalization_boundary(false, UnregisterDisposition::KEEP)
            .await
    }

    /// Exact rollback for a caller that already owns the persistent service's
    /// turn-finalization boundary.
    ///
    /// This takes the session's live lifecycle lease while the caller holds
    /// that boundary. A concurrent raw unregister's saga holds the lease
    /// into a surface cleanup that may take the boundary, so a caller whose
    /// cleanup handle can do that (one with a published actor) must release
    /// the boundary and call [`Self::rollback_now`] instead.
    pub async fn rollback_now_under_turn_finalization_boundary(
        &mut self,
    ) -> Result<bool, RuntimeDriverError> {
        self.rollback_now_with_turn_finalization_boundary(true, UnregisterDisposition::KEEP)
            .await
    }

    async fn rollback_now_with_turn_finalization_boundary(
        &mut self,
        turn_finalization_boundary_already_held: bool,
        queued_inputs: UnregisterDisposition,
    ) -> Result<bool, RuntimeDriverError> {
        if !self.armed {
            return Ok(false);
        }
        if !self.request_abort() {
            self.armed = false;
            return Ok(false);
        }
        let rolled_back = self
            .machine
            .abort_prepared_session_materialization_claim(
                self.bindings.session_id(),
                self.claim_id,
                Some(self.bindings.epoch_id()),
                Some(&self.claim_state),
                turn_finalization_boundary_already_held,
                queued_inputs,
            )
            .await?;
        self.armed = false;
        Ok(rolled_back)
    }

    /// Publish a staged-session owner. The staged registry must subsequently
    /// retain the exact bindings and call exact rollback when it abandons the
    /// slot; actor construction may later consume the same claim.
    pub async fn commit_staged(&mut self) -> Result<(), RuntimeDriverError> {
        self.machine
            .commit_prepared_session_materialization_staged(&self.bindings)
            .await?;
        self.armed = false;
        Ok(())
    }

    /// Retain a successfully created actor before executor attachment. The
    /// provisional cleanup stays on the machine entry for raw unregister, while
    /// this transaction can no longer roll the registration back.
    pub async fn commit_actor_unattached(&mut self) -> Result<(), RuntimeDriverError> {
        self.machine
            .commit_prepared_session_actor_unattached(&self.bindings)
            .await?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for PreparedSessionMaterialization {
    fn drop(&mut self) {
        if !self.armed || !self.request_abort() {
            return;
        }
        let machine = Arc::clone(&self.machine);
        let session_id = self.bindings.session_id().clone();
        let epoch_id = self.bindings.epoch_id().clone();
        let claim_id = self.claim_id;
        let claim_state = Arc::clone(&self.claim_state);
        self.cleanup_spawner.spawn(async move {
            if let Err(error) = machine
                .abort_prepared_session_materialization_claim(
                    &session_id,
                    claim_id,
                    Some(&epoch_id),
                    Some(&claim_state),
                    false,
                    // As rollback_now: the machine stops every input instead
                    // once the registration served live work.
                    UnregisterDisposition::KEEP,
                )
                .await
            {
                tracing::warn!(
                    %session_id,
                    %error,
                    "drop-time exact prepared materialization rollback failed"
                );
            }
        });
    }
}

pub(super) struct PendingPreparedMaterialization {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    claim_id: uuid::Uuid,
    cleanup_spawner: MachineCleanupTaskSpawner,
    claim_state_slot: Arc<
        std::sync::Mutex<
            Option<Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>>,
        >,
    >,
    armed: bool,
}

impl PendingPreparedMaterialization {
    pub(super) fn new(
        machine: Arc<MeerkatMachine>,
        session_id: SessionId,
        claim_id: uuid::Uuid,
    ) -> Result<Self, RuntimeDriverError> {
        Ok(Self {
            machine,
            session_id,
            claim_id,
            cleanup_spawner: MachineCleanupTaskSpawner::acquire()?,
            claim_state_slot: Arc::new(std::sync::Mutex::new(None)),
            armed: true,
        })
    }

    fn cleanup_spawner(&self) -> MachineCleanupTaskSpawner {
        self.cleanup_spawner.clone()
    }

    pub(super) fn claim_state_slot(
        &self,
    ) -> Arc<
        std::sync::Mutex<
            Option<Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>>,
        >,
    > {
        Arc::clone(&self.claim_state_slot)
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingPreparedMaterialization {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let claim_state = self
            .claim_state_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(claim_state) = claim_state {
            let mut state = claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.current == Some(self.claim_id)
                && state.phase != crate::RuntimeActorMaterializationClaimPhase::RetainedActor
            {
                state.phase = crate::RuntimeActorMaterializationClaimPhase::Aborting;
            }
        }
        let machine = Arc::clone(&self.machine);
        let session_id = self.session_id.clone();
        let claim_id = self.claim_id;
        self.cleanup_spawner.spawn(async move {
            if let Err(error) = machine
                .abort_prepared_session_materialization_claim(
                    &session_id,
                    claim_id,
                    None,
                    None,
                    false,
                    UnregisterDisposition::KEEP,
                )
                .await
            {
                tracing::warn!(
                    %session_id,
                    %error,
                    "cancelled binding preparation exact rollback failed"
                );
            }
        });
    }
}

/// Exact same-session mutation lease used to commit a direct SessionService
/// turn without reacquiring the machine gate while the session recovery gate
/// is held.
///
/// The session layer acquires this only after releasing its execution-time
/// recovery guard, then reacquires recovery in the canonical
/// machine-mutation -> recovery order for snapshot commit/checkpoint.
pub struct MachineServiceTurnCommitLease {
    session_id: SessionId,
    run_id: RunId,
    driver: SharedDriver,
    _mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

/// Exact runtime-entry identity captured before a direct SessionService turn.
/// It carries no lock; terminal commit later acquires mutation authority only
/// if this same driver is still current.
pub struct MachineServiceTurnIdentity {
    session_id: SessionId,
    driver: SharedDriver,
}

impl MachineServiceTurnCommitLease {
    /// Exact machine-owned run whose terminal this lease may commit.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

impl MachineSessionArchiveLease {
    /// Whether archive preparation inserted the captured in-memory
    /// registration from durable authority.
    pub fn owns_recovered_registration(&self) -> bool {
        self.recovered_registration_for_archive
    }

    /// Whether this recovered archive registration originated from durable
    /// Retired/Destroyed authority.
    pub fn originated_from_quiescent_archive(&self) -> bool {
        self.recovered_from_quiescent_archive
    }

    /// Exact runtime lifecycle observed while this lease owns the session's
    /// mutation gate.
    pub async fn runtime_state(&self) -> RuntimeState {
        self.driver.lock().await.runtime_state()
    }

    /// Whether the captured runtime half has already reached the durable
    /// archive terminal. The lease owns the session mutation gate, so this
    /// observation cannot race a lifecycle transition.
    pub async fn runtime_is_retired(&self) -> bool {
        self.runtime_state().await == RuntimeState::Retired
    }

    /// Whether this exact runtime attachment supplied a live publication
    /// capability. Archive must not invoke that public callback while this
    /// lease still owns the session mutation gate.
    pub fn has_attached_publication_handle(&self) -> bool {
        self.publication_handle.is_some()
    }
}

/// Capability bundle for an attached runtime loop.
///
/// Keep all loop-related handles together so "attached vs detached" cannot
/// drift into partially-populated shell state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeLoopAttachmentId(uuid::Uuid);

impl RuntimeLoopAttachmentId {
    fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// Mechanical proof captured while M is held for one executor-effect
/// dispatch. A callback may release M, but only this exact attachment may
/// receive the resulting effect after revalidation.
struct RuntimeEffectDispatchAttachmentWitness {
    mutation_gate: SessionMutationGate,
    driver: SharedDriver,
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
    attachment_id: RuntimeLoopAttachmentId,
    effect_tx: mpsc::Sender<crate::effect::RuntimeEffect>,
}

/// Exact executor attachment captured before a cooperative live-boundary
/// callback. Publication resumes only after every attachment-local component
/// is revalidated.
struct RuntimeLiveBoundaryAttachmentWitness {
    mutation_gate: SessionMutationGate,
    driver: SharedDriver,
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
    attachment_id: RuntimeLoopAttachmentId,
    boundary_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>,
}

/// Best-effort actor projection for one already-durable input. This authority
/// is deliberately separate from admission: the caller may return as soon as
/// this non-cloneable plan is transferred to process-owned execution.
struct RuntimeAcceptedLiveBoundaryPlan {
    witness: RuntimeLiveBoundaryAttachmentWitness,
    input_id: InputId,
    publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
}

struct RuntimeAcceptedBoundaryCancelPlan {
    witness: RuntimeEffectDispatchAttachmentWitness,
    boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
    pending_dispatch: PendingBoundaryCancelDispatchGuard,
    expected_run_id: RunId,
    projected_effect: crate::effect::ProjectedRuntimeEffect,
    dispatch_generation: u64,
    dispatch_lifecycle_phase: dsl::MeerkatPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryCancelDispatchState {
    Current,
    ConsumedConverged,
    SameGenerationOrphan,
    Superseded,
}

struct RuntimeEffectDispatchMemberAuthority {
    lease: MemberEffectAuthorityLease,
    expected_member: Option<meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation>,
}

/// Exact-generation compensation for a boundary dispatch abandoned before
/// enqueue. It can only clear the generation it captured.
struct PendingBoundaryCancelDispatchGuard {
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
    dispatch_generation: u64,
    armed: bool,
}

impl PendingBoundaryCancelDispatchGuard {
    fn new(
        dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        dispatch_generation: u64,
    ) -> Self {
        Self {
            dsl_authority,
            dispatch_generation,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingBoundaryCancelDispatchGuard {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = MeerkatMachine::abort_dsl_boundary_cancel_dispatch_if_current(
                &self.dsl_authority,
                self.dispatch_generation,
            )
        {
            tracing::error!(
                dispatch_generation = self.dispatch_generation,
                error = %error,
                "failed to compensate an abandoned boundary-cancel dispatch"
            );
        }
    }
}

/// Mechanical fallback for a durably accepted input whose later publication
/// step exits early. The wake channel belongs to the captured attachment.
struct AcceptedIngressFallbackWakeGuard {
    wake_tx: Option<mpsc::Sender<()>>,
    armed: bool,
}

impl AcceptedIngressFallbackWakeGuard {
    fn new(wake_tx: Option<mpsc::Sender<()>>, armed: bool) -> Self {
        Self { wake_tx, armed }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AcceptedIngressFallbackWakeGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(wake_tx) = self.wake_tx.as_ref()
        {
            let _ = wake_tx.try_send(());
        }
    }
}

struct RuntimeLoopAttachment {
    id: RuntimeLoopAttachmentId,
    wake_tx: mpsc::Sender<()>,
    effect_tx: mpsc::Sender<crate::effect::RuntimeEffect>,
    serving_release: Option<crate::runtime_loop::RuntimeLoopServingRelease>,
    boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
    interrupt_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>>,
    loop_handle: tokio::task::JoinHandle<()>,
}

struct PendingUserInterruptDispatch {
    dispatch_id: uuid::Uuid,
    expected_run_id: RunId,
    attachment_id: Option<RuntimeLoopAttachmentId>,
    provisional_materialization_claim_id: Option<uuid::Uuid>,
    interrupt_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>,
    expected_member: Option<meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation>,
    result_rx: crate::tokio::sync::watch::Receiver<Option<Result<bool, RuntimeDriverError>>>,
}

/// Mechanical runtime-loop channel slot.
enum RuntimeLoopAttachmentSlot {
    Empty,
    Pending(RuntimeLoopAttachment),
    Attached(RuntimeLoopAttachment),
}

impl RuntimeSessionEntry {
    /// The winning publication of this registration, or an accepted reuse
    /// of it, completes its owner-loss reservation, which the issuing consent
    /// owner settled: the lineage's settlement is complete and the entry
    /// keeps the settled proof. A preparation abandoned before publication,
    /// or a candidate another registration published first, drops its
    /// reservation instead, which returns the proof to the lineage for the
    /// registration that does publish.
    fn complete_owner_loss(
        &mut self,
        reservation: Option<meerkat_core::session_hosting::OwnerLossReservation>,
    ) {
        if let Some(reservation) = reservation {
            self.owner_loss_proof = Some(reservation.complete());
        }
    }

    /// Wake materialization-claim release waiters because this registration
    /// is leaving the registry (its claim can no longer be held).
    /// The registration left the machine: retire its agent binding for good
    /// and wake materialization waiters.
    fn notify_registration_removed(&self) {
        self.agent_binding.retire();
        self.tool_surface_gate.retire();
        self.mcp_observation.retire();
        self.materialization_claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .changed
            .notify_waiters();
    }
}

impl RuntimeSessionEntry {
    fn require_durability_ready(&self) -> Result<(), DurabilityReloadRequired> {
        match self.durability_health.as_ref() {
            Some(health) => health.require_ready(),
            None => Ok(()),
        }
    }

    /// Whether this is a predecessor settlement's internal registration.
    fn is_predecessor_settlement(&self) -> bool {
        matches!(
            self.mechanical_registration,
            Some(MechanicalRegistration::PredecessorSettlement)
        )
    }

    fn dsl_mutation_blocked_by_unregister(
        &self,
        session_id: &SessionId,
    ) -> Option<RuntimeDriverError> {
        if self.is_predecessor_settlement() {
            // The settlement's internal registration never serves: every
            // path that could make it live is refused while the settlement
            // owns it, and it leaves only through the settlement's exact
            // removal. Its own steps take M through the settlement and apply
            // transitions on the driver directly.
            return Some(RuntimeDriverError::UnregisterInProgress {
                runtime_id: self.runtime_id.clone(),
            });
        }
        if self.pending_unregister_finalization.is_some() {
            return Some(RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                reason: format!(
                    "session {session_id} retains an ambiguous unregister finalization; retry unregister before applying any other lifecycle mutation"
                ),
            });
        }
        self.handle_teardown_gate
            .is_closed()
            .then(|| RuntimeDriverError::ValidationFailed {
                reason: format!("session {session_id} is a teardown-only unregister retry anchor"),
            })
    }

    fn registration_blocked_by_unregister(
        &self,
        session_id: &SessionId,
    ) -> Option<RuntimeDriverError> {
        if let Some(error) = self.dsl_mutation_blocked_by_unregister(session_id) {
            return Some(error);
        }
        if self.unregister_coordinator.is_some() {
            return Some(RuntimeDriverError::NotReady {
                state: self.control_snapshot().phase,
            });
        }
        let Some(coordinator) = self.runtime_stop_cleanup_coordinator.as_ref() else {
            return None;
        };
        match coordinator.result_rx.borrow().clone() {
            None => Some(RuntimeDriverError::RuntimeStopInProgress {
                runtime_id: self.runtime_id.clone(),
            }),
            Some(Ok(())) => None,
            Some(Err(error)) => Some(error),
        }
    }

    fn control_snapshot(&self) -> crate::driver::ephemeral::RuntimeControlProjection {
        self.control_projection
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner().clone()
            })
    }

    fn sync_control_projection_from_dsl_authority(&self) {
        let next = {
            let authority = self
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::driver::ephemeral::RuntimeControlProjection {
                phase: crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(
                    &authority,
                ),
                current_run_id:
                    crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority),
                pre_run_phase: crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(
                    &authority,
                ),
            }
        };
        *self
            .control_projection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }

    fn physical_attachment_is_live(&self) -> bool {
        match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Pending(attachment)
            | RuntimeLoopAttachmentSlot::Attached(attachment) => {
                !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed()
            }
            RuntimeLoopAttachmentSlot::Empty => false,
        }
    }

    fn attachment_is_live(&self) -> bool {
        matches!(
            &self.attachment_slot,
            RuntimeLoopAttachmentSlot::Attached(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed()
        )
    }

    fn generated_executor_registration_active(&self) -> bool {
        let authority = self
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(
            authority.state().registration_phase,
            dsl::RegistrationPhase::Active
        )
    }

    /// [`Self::generated_executor_registration_active`], asked by a caller that
    /// is not allowed to park.
    ///
    /// `None` means the authority could not be read right now. That is a fact
    /// about the observation, not about the session, and the caller decides
    /// what to do with it. A plain `lock()` here would let a session wedged
    /// while holding this authority wedge its observer too, which is exactly
    /// the failure the health probe exists to report rather than join; the same
    /// reasoning as `run_progress.rs`'s `try_lock` on this authority.
    ///
    /// A poisoned authority is read through rather than treated as unreadable:
    /// the registration phase behind it is still the last value some panicking
    /// writer left, and dropping the reading is the silent direction.
    fn try_generated_executor_registration_active(&self) -> Option<bool> {
        let authority = match self.dsl_authority.try_lock() {
            Ok(authority) => authority,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        Some(matches!(
            authority.state().registration_phase,
            dsl::RegistrationPhase::Active
        ))
    }

    /// True when this entry's runtime loop is a corpse: generated registration
    /// still claims a live executor, but the shell backing it cannot run.
    ///
    /// Stage A (below) is deliberately lock-free. A session that is RUNNING -
    /// however busy, however deep in a turn - fails the slot/channel/join test
    /// without touching a single per-entry mutex, so load alone can never make
    /// it a candidate. That is what keeps this probe from crying wolf under
    /// contention, and it is a structural property rather than a tuning choice.
    ///
    /// What load does not cover, stated plainly rather than claimed away:
    /// **ordinary teardown reaches Stage A too.** When a session shuts down
    /// normally the loop task exits, its receivers drop, the channels report
    /// closed and the join handle reports finished - which is the same
    /// observation a corpse produces, because it is the same underlying fact.
    /// Stage A cannot separate them and does not try. Stage B is what separates
    /// them, by asking the one remaining question: does anything still claim
    /// this dead shell? A completed teardown answers no. A scrape that lands
    /// inside the teardown window, after the task has exited but before the
    /// teardown watcher has cleared the registration, is answered yes and is
    /// counted. That reading is wrong, it is bounded by the width of that
    /// window, it is published at `Degraded` rather than `Unhealthy`, and it
    /// clears on the next scrape. The alternative - suppressing the count until
    /// some settling delay elapses - would trade a self-clearing amber blip for
    /// a probe that goes quiet exactly when a shell stays dead.
    fn runtime_loop_is_dead_shell(&self) -> bool {
        // Stage A: no locks. An empty slot is not a corpse even under `Active`
        // registration; that is the legitimate pre-spawn state that
        // `generated_executor_registration_has_viable_attachment` also admits.
        let dead_shell = match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Pending(attachment)
            | RuntimeLoopAttachmentSlot::Attached(attachment) => {
                attachment.wake_tx.is_closed()
                    || attachment.effect_tx.is_closed()
                    || attachment.loop_handle.is_finished()
            }
            RuntimeLoopAttachmentSlot::Empty => false,
        };
        if !dead_shell {
            return false;
        }
        // Stage B, candidates only.
        //   Some(true)  -> registration still claims this dead shell: a corpse.
        //   Some(false) -> teardown already owns it; not an incident.
        //   None        -> the authority could not be read.
        //
        // Why `None` counts, rather than propagating an unreadable the way the
        // session-registry read above this does. The two look like the same
        // situation and are not.
        //
        // Stage A ALREADY ESTABLISHED THE FAULT, and established it without
        // taking a lock: this shell cannot run. That observation does not
        // depend on the authority read and is not in doubt here. So counting a
        // `None` is not the "assert a fault nobody observed" move that the
        // `Unreadable` vocabulary exists to prevent one layer up - the fault
        // was observed, lock-free, before this line was reached. The only thing
        // left unresolved is the narrower question of whether anything still
        // claims the corpse.
        //
        // Given that, the direction is chosen rather than defaulted.
        // `unwrap_or(false)` would drop precisely the reading that a wedged
        // holder of this authority denies us, on a session whose loop is
        // already dead - which is the exact shape of the incident this probe
        // was added to make visible, silently discarded. `unwrap_or(true)`
        // over-counts instead, in the same bounded way the teardown window
        // described on this function's doc over-counts: `Degraded`, per scrape,
        // self-clearing.
        //
        // And propagating unreadable from HERE would be strictly worse than
        // either. This is a per-ENTRY lock miss inside a fold over every
        // registered session; turning it into a whole-dimension `Unreadable`
        // would let one uncooperative entry blank the reading for every other
        // session on the host, at the moment a wedged holder exists - i.e. the
        // dimension would go dark exactly when it matters. The registry read in
        // `dead_runtime_loop_session_count` propagates unreadable because it
        // loses the WHOLE population; this loses one entry's tiebreak.
        self.try_generated_executor_registration_active()
            .unwrap_or(true)
    }

    /// Recompute this session's staged -> executing window health from
    /// machine truth.
    ///
    /// Nothing is latched: the armed window supplies only the run identity,
    /// the staging instant, and the turn-start gate, and the verdict is
    /// re-derived from `dsl_authority` per read via the same classification
    /// the staged-run watchdog uses. The authority read inside is a
    /// `try_lock` (see `AuthorityRunExecutionProgress::observe`), so a wedged
    /// holder of the authority - the prime suspect for the very wedge this
    /// observes - cannot park the probe; it surfaces as
    /// [`crate::run_progress::RunStartHealth::Unreadable`] instead.
    fn run_start_health(&self) -> crate::run_progress::RunStartHealth {
        crate::run_progress::observe_run_start_window(
            &self.run_start_window,
            &self.dsl_authority,
            crate::run_progress::RUN_EXECUTION_START_NOTICE,
        )
    }

    /// Recompute this session's parked-queued-work health from machine lane
    /// truth plus the ledger's per-input clock.
    ///
    /// Nothing is latched and no new state exists for this probe at all: both
    /// halves of the predicate live in structures this entry already owns
    /// (the generated authority for lane truth, the driver's ledger for
    /// timestamps), each read with a non-blocking try in strict sequence. See
    /// [`crate::run_progress::observe_parked_queued_work`] for the locking
    /// story and the void condition.
    fn parked_queued_work_health(&self) -> crate::run_progress::ParkedQueuedWorkHealth {
        crate::run_progress::observe_parked_queued_work(
            &self.dsl_authority,
            &self.driver,
            crate::run_progress::QUEUED_INPUT_START_NOTICE,
            chrono::Utc::now(),
        )
    }

    fn generated_executor_registration_has_viable_attachment(&self) -> bool {
        self.generated_executor_registration_active()
            && match &self.attachment_slot {
                RuntimeLoopAttachmentSlot::Empty => true,
                RuntimeLoopAttachmentSlot::Pending(_) | RuntimeLoopAttachmentSlot::Attached(_) => {
                    self.physical_attachment_is_live()
                }
            }
    }

    fn close_handle_teardown_gate(&self) {
        let _guard = self
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.handle_teardown_gate.close();
        // Every detached session-owned handle fails closed from here, and so
        // does the registration's agent binding and tool-surface gate.
        self.agent_binding.retire();
        self.tool_surface_gate.retire();
        self.mcp_observation.retire();
    }

    /// True while the runtime-loop executor registration is `Active` *or*
    /// `Draining`. The drain window (`BeginUnregisterSession` → final
    /// `UnregisterSession`) keeps the session registered so the in-flight run
    /// can still commit and resolve its completion waiters; the runtime-loop
    /// driver-authority gate must therefore admit `Draining`, while the
    /// registration *claim* check stays `Active`-only (no new attachment may be
    /// granted inside the drain window).
    fn generated_executor_registration_active_or_draining(&self) -> bool {
        let authority = self
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(
            authority.state().registration_phase,
            dsl::RegistrationPhase::Active | dsl::RegistrationPhase::Draining
        )
    }

    fn generated_service_turn_binding_open(&self, session_id: &SessionId) -> bool {
        let authority = self
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        self.handle_teardown_gate.is_open()
            && state.session_id.as_ref() == Some(&dsl::SessionId::from_domain(session_id))
            && state.registration_phase != dsl::RegistrationPhase::Draining
    }

    fn generated_stop_deferred(&self) -> bool {
        self.dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .runtime_stop_deferred
    }

    fn stage_generated_executor_registration_claim(
        &self,
        session_id: &SessionId,
    ) -> Result<StagedSessionDslInput, String> {
        let staged = MeerkatMachine::stage_dsl_transition_on_authority(
            &self.dsl_authority,
            dsl::MeerkatMachineInput::EnsureSessionWithExecutor {
                session_id: dsl::SessionId::from_domain(session_id),
            },
            "EnsureSessionWithExecutor",
        )?;
        if self.generated_executor_registration_active() {
            Ok(staged)
        } else {
            let mut authority = self
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            authority.restore_snapshot(staged.previous_snapshot);
            Err("generated MeerkatMachine did not grant active executor registration".into())
        }
    }

    fn stage_generated_executor_exit_observation(&self) -> Result<StagedSessionDslInput, String> {
        MeerkatMachine::stage_runtime_owner_dsl_transition_on_authority(
            &self.dsl_authority,
            crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::RuntimeExecutorExited,
        )
    }

    /// Returns `true` only if the executor is fully attached with live channels.
    /// Used by internal publish logic within `ensure_session_with_executor`.
    fn has_live_attachment(&self) -> bool {
        self.attachment_is_live()
    }

    // Attachment publishes the complete independent capability set in one
    // atomic slot; bundling it earlier would create a second authority shape.
    #[allow(clippy::too_many_arguments)]
    fn attach_runtime_loop(
        &mut self,
        id: RuntimeLoopAttachmentId,
        wake_tx: mpsc::Sender<()>,
        effect_tx: mpsc::Sender<crate::effect::RuntimeEffect>,
        serving_release: crate::runtime_loop::RuntimeLoopServingRelease,
        boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
        interrupt_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>>,
        publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        post_stop_cleanup_handle: Option<
            Arc<dyn meerkat_core::lifecycle::CoreExecutorPostStopCleanupHandle>,
        >,
        machine_managed_post_stop_unregister: bool,
        expected_materialization_claim: Option<&RuntimeExecutorAttachmentMaterializationClaim>,
        spawned_loop: crate::runtime_loop::SpawnedRuntimeLoop,
    ) -> Result<(), crate::runtime_loop::SpawnedRuntimeLoop> {
        if spawned_loop.startup_authority_transfer.is_some()
            || spawned_loop.serving_release.is_some()
        {
            return Err(spawned_loop);
        }
        let claim_changed = {
            let mut state = self
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match expected_materialization_claim {
                Some(expected)
                    if Arc::ptr_eq(
                        &self.materialization_claim_state,
                        &expected.claim_state,
                    ) && state.exact_claim_is(
                        expected.claim_id,
                        &[crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit],
                    ) => {}
                Some(_) => return Err(spawned_loop),
                None
                    if state.current.is_none()
                        && matches!(
                            state.phase,
                            crate::RuntimeActorMaterializationClaimPhase::Vacant
                                | crate::RuntimeActorMaterializationClaimPhase::RetainedActor
                        ) => {}
                None => return Err(spawned_loop),
            }
            let Some(next_legacy_generation) = state.legacy_capability_generation.checked_add(1)
            else {
                return Err(spawned_loop);
            };
            state.legacy_capability_generation = next_legacy_generation;
            // A `RetainedActor` claim has no current id; its release to
            // Vacant is still a claim change that release waiters observe.
            let claim_changed = state.current.take().is_some()
                || state.phase != crate::RuntimeActorMaterializationClaimPhase::Vacant;
            state.phase = crate::RuntimeActorMaterializationClaimPhase::Vacant;
            state.rollback_registration_available = false;
            claim_changed.then(|| Arc::clone(&state.changed))
        };
        if let Some(changed) = claim_changed {
            changed.notify_waiters();
        }
        let crate::runtime_loop::SpawnedRuntimeLoop {
            loop_handle,
            teardown_slot,
            startup: _,
            startup_authority_transfer: _,
            serving_release: _,
        } = spawned_loop;
        self.provisional_interrupt_handle = None;
        self.provisional_materialization_claim_id = None;
        self.runtime_stop_cleanup_coordinator = None;
        self.runtime_loop_teardown = Some(Arc::clone(&teardown_slot));
        self.publication_handle = publication_handle;
        self.post_stop_cleanup_handle = post_stop_cleanup_handle;
        self.post_stop_cleanup_attachment_id = machine_managed_post_stop_unregister.then_some(id);
        self.post_stop_cleanup_complete = !machine_managed_post_stop_unregister;
        self.post_stop_cleanup_gate = Arc::new(Mutex::new(()));
        self.attachment_slot = RuntimeLoopAttachmentSlot::Pending(RuntimeLoopAttachment {
            id,
            wake_tx,
            effect_tx,
            serving_release: Some(serving_release),
            boundary_handle,
            interrupt_handle,
            loop_handle,
        });
        Ok(())
    }

    fn owns_runtime_loop_attachment(&self, expected: RuntimeLoopAttachmentId) -> bool {
        matches!(
            &self.attachment_slot,
            RuntimeLoopAttachmentSlot::Pending(attachment)
                | RuntimeLoopAttachmentSlot::Attached(attachment)
                if attachment.id == expected
        )
    }

    /// This registration's current use ([`RuntimeSessionRegistrationUse`]).
    /// The caller holds the session map lock.
    fn registration_use(&self) -> RuntimeSessionRegistrationUse {
        RuntimeSessionRegistrationUse(
            self.registration_use
                .load(std::sync::atomic::Ordering::Acquire),
        )
    }

    /// Hand this registration to a new user: stamp it with `stamp`, a fresh
    /// value of the machine-wide sequence. The caller holds the session map
    /// lock.
    fn stamp_adoption(&self, stamp: u64) {
        self.registration_use
            .store(stamp, std::sync::atomic::Ordering::Release);
    }

    /// Whether no runtime loop can hold this registration's mutation fence:
    /// no attachment is pending or committed and no runtime-loop teardown
    /// slot exists (no loop was ever attached, or its teardown completed).
    fn no_runtime_loop_holds_mutation_fence(&self) -> bool {
        matches!(self.attachment_slot, RuntimeLoopAttachmentSlot::Empty)
            && self.runtime_loop_teardown.is_none()
    }

    /// Whether this registration's runtime-loop teardown is still the one a
    /// reload-required discard was admitted with: the exact teardown handoff
    /// slot, or, for a registration no runtime loop held, still none.
    fn runtime_loop_teardown_is_as_admitted(
        &self,
        admitted: Option<&Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
    ) -> bool {
        match (self.runtime_loop_teardown.as_ref(), admitted) {
            (Some(current), Some(admitted)) => Arc::ptr_eq(current, admitted),
            (None, None) => self.no_runtime_loop_holds_mutation_fence(),
            _ => false,
        }
    }

    fn live_attachment_id(&self) -> Option<RuntimeLoopAttachmentId> {
        match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Pending(attachment)
            | RuntimeLoopAttachmentSlot::Attached(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed() =>
            {
                Some(attachment.id)
            }
            _ => None,
        }
    }

    /// Detach the runtime-loop channels, returning the loop's `JoinHandle` so a
    /// caller can await its quiescence.
    ///
    /// Dropping the returned `wake_tx`/`effect_tx` (held inside the attachment)
    /// closes the loop's receivers, which drives the loop through its canonical
    /// `StopRuntimeExecutor` + `RuntimeExecutorExited` exit. The slot is left
    /// `Empty`. Returns `None` when no loop is attached.
    fn take_runtime_loop_attachment(&mut self) -> Option<RuntimeLoopAttachment> {
        match std::mem::replace(&mut self.attachment_slot, RuntimeLoopAttachmentSlot::Empty) {
            RuntimeLoopAttachmentSlot::Pending(attachment) => Some(attachment),
            RuntimeLoopAttachmentSlot::Attached(attachment) => Some(attachment),
            RuntimeLoopAttachmentSlot::Empty => None,
        }
    }

    fn clear_dead_attachment(&mut self) -> bool {
        if matches!(
            self.attachment_slot,
            RuntimeLoopAttachmentSlot::Pending(_) | RuntimeLoopAttachmentSlot::Attached(_)
        ) && !self.physical_attachment_is_live()
        {
            self.attachment_slot = RuntimeLoopAttachmentSlot::Empty;
            return true;
        }
        false
    }

    fn retire_completed_runtime_stop_after_revival(
        &mut self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        if !matches!(self.attachment_slot, RuntimeLoopAttachmentSlot::Empty) {
            return Err(RuntimeDriverError::Internal(format!(
                "revived session {session_id} still carries a runtime-loop attachment"
            )));
        }
        match self.runtime_stop_cleanup_coordinator.as_ref() {
            None if self.runtime_loop_teardown.is_none() => return Ok(()),
            None => {
                return Err(RuntimeDriverError::Internal(format!(
                    "revived session {session_id} carries a teardown slot without its stop coordinator"
                )));
            }
            Some(coordinator) => {
                let coordinator_slot_is_current = match (
                    coordinator.teardown_slot.as_ref(),
                    self.runtime_loop_teardown.as_ref(),
                ) {
                    (Some(coordinator_slot), Some(current_slot)) => {
                        Arc::ptr_eq(coordinator_slot, current_slot)
                    }
                    (None, None) => true,
                    _ => false,
                };
                if !coordinator_slot_is_current {
                    return Err(RuntimeDriverError::Internal(format!(
                        "revived session {session_id} carries a stale runtime-stop teardown owner"
                    )));
                }
                match coordinator.result_rx.borrow().clone() {
                    Some(Ok(())) => {}
                    None => {
                        return Err(RuntimeDriverError::RuntimeStopInProgress {
                            runtime_id: self.runtime_id.clone(),
                        });
                    }
                    Some(Err(error)) => return Err(error),
                }
            }
        }
        self.runtime_stop_cleanup_coordinator = None;
        self.runtime_loop_teardown = None;
        Ok(())
    }

    fn wake_sender(&self) -> Option<mpsc::Sender<()>> {
        match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Attached(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed() =>
            {
                Some(attachment.wake_tx.clone())
            }
            _ => None,
        }
    }

    fn effect_sender(&self) -> Option<mpsc::Sender<crate::effect::RuntimeEffect>> {
        match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Attached(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed() =>
            {
                Some(attachment.effect_tx.clone())
            }
            _ => None,
        }
    }

    fn boundary_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>> {
        match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Attached(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed() =>
            {
                attachment.boundary_handle.clone()
            }
            _ => None,
        }
    }

    fn interrupt_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>> {
        match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Attached(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed() =>
            {
                attachment.interrupt_handle.clone()
            }
            _ => self.provisional_interrupt_handle.clone(),
        }
    }

    fn publication_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>> {
        self.publication_handle.clone()
    }

    fn install_provisional_interrupt_handle(
        &mut self,
        claim_id: uuid::Uuid,
        handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>,
    ) {
        if !self.physical_attachment_is_live() {
            self.provisional_materialization_claim_id = Some(claim_id);
            self.provisional_interrupt_handle = Some(handle);
        }
    }

    fn install_provisional_post_stop_cleanup_handle(
        &mut self,
        claim_id: uuid::Uuid,
        handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPostStopCleanupHandle>,
    ) {
        if !self.physical_attachment_is_live() {
            self.provisional_materialization_claim_id = Some(claim_id);
            self.post_stop_cleanup_handle = Some(handle);
            self.post_stop_cleanup_attachment_id = Some(RuntimeLoopAttachmentId::new());
            self.post_stop_cleanup_complete = false;
            self.post_stop_cleanup_gate = Arc::new(Mutex::new(()));
        }
    }
}

impl MeerkatMachine {
    pub(crate) async fn post_commit_hooks_for_session(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<meerkat_core::PostCommitHookDispatcher>> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .map(|entry| Arc::clone(&entry.post_commit_hooks))
    }

    /// How many registered sessions currently refuse execution because their
    /// durability gate demands a cold reload.
    ///
    /// `None` means the session registry could not be read at the moment of the
    /// probe, which is a fact about the observation and not about the host. A
    /// caller that publishes health must not translate it into a number, and
    /// must not let it read as "nobody looked at this dimension" either: this
    /// probe *did* run and *did* fail to observe, which is its own fact and one
    /// that leaves the caller with no basis for reporting the host healthy.
    ///
    /// Two failure directions are chosen deliberately, in opposite directions,
    /// and both are load-bearing:
    ///
    /// - **Storeless sessions are never counted.** They carry no durability
    ///   contract, so there is nothing about them this number could be wrong
    ///   about. Counting them would make an ephemeral host permanently amber.
    /// - **A poisoned gate is read through, not skipped.** `require_ready`
    ///   absorbs poison with `into_inner`; a session whose durability state was
    ///   last written by a panicking task is exactly the session an operator
    ///   wants counted.
    ///
    /// `n > 0` is `Degraded`, not `Unhealthy`: a reload-required session is
    /// fenced and recoverable by a cold install, and the host still serves
    /// every other session.
    ///
    /// Not `async` on purpose. Every lock this touches is a non-blocking try or
    /// a short synchronous one, so the signature itself proves the probe cannot
    /// park behind the wedge it is trying to report.
    pub fn reload_required_session_count(&self) -> Option<usize> {
        let sessions = self.sessions.try_read().ok()?;
        Some(
            sessions
                .values()
                .filter(|entry| {
                    entry
                        .durability_health
                        .as_ref()
                        .is_some_and(|health| health.require_ready().is_err())
                })
                .count(),
        )
    }

    /// How many registered sessions still claim a runtime loop that is dead.
    ///
    /// A dead loop is indistinguishable from an idle one to every other read
    /// path on this type, which is why this exists: without it, a session whose
    /// executor task is gone looks exactly like a quiet healthy session.
    ///
    /// `None` carries the same meaning and the same obligation as on
    /// [`Self::reload_required_session_count`]: the registry was not readable,
    /// so a health caller must publish that failed read as such rather than
    /// publish a zero or stay silent.
    ///
    /// See `RuntimeSessionEntry::runtime_loop_is_dead_shell` for why the
    /// healthy path is lock-free and the candidate path uses `try_lock`.
    ///
    /// Not `async` on purpose, for the same reason as its sibling.
    pub fn dead_runtime_loop_session_count(&self) -> Option<usize> {
        let sessions = self.sessions.try_read().ok()?;
        Some(
            sessions
                .values()
                .filter(|entry| entry.runtime_loop_is_dead_shell())
                .count(),
        )
    }

    /// How many registered sessions hold a staged run that is overdue to
    /// begin executing.
    ///
    /// "Overdue" is recomputed from machine truth on every call and means
    /// exactly what the staged-run watchdog's notice tier means: the run was
    /// staged more than `run_progress::RUN_EXECUTION_START_NOTICE`
    /// ago, machine authority still reports it current with its primitive
    /// un-applied, and the loop signalled its turn start so the phase read
    /// describes this run. The wire claim and the existing watchdog log line
    /// therefore cannot disagree about what "overdue" means. Runs whose
    /// execution start is honestly unobservable (the appends-empty and
    /// retired-drain classes, or an unbound runtime) are never counted - the
    /// escalation bound cannot arm on them and the phase says nothing.
    ///
    /// `None` means the count was not established: the session registry was
    /// not readable, or a past-bound window's authority could not be read
    /// without blocking and no other session was positively overdue. The
    /// per-entry miss propagates here - unlike
    /// `RuntimeSessionEntry::runtime_loop_is_dead_shell`, which counts its
    /// per-entry miss - because the two misses sit on opposite sides of an
    /// established fact. A dead shell is proven lock-free before its
    /// authority is consulted; the try_lock there only resolves who owns the
    /// corpse. Here nothing is proven without the authority: a past-bound
    /// window is also what a healthy long turn looks like, so counting an
    /// unread entry would assert a wedge nobody observed. And an unread
    /// authority on a past-bound window is not noise to average away - the
    /// wedged pre-apply consumer this dimension exists to see holds exactly
    /// that mutex, so "could not look" must reach the operator rather than
    /// roll up as `Ok`. When some other session IS positively overdue, the
    /// observed fault wins: the count publishes and the unread entry waits
    /// for the next scrape.
    ///
    /// Not `async` on purpose, for the same reason as its siblings: every
    /// lock this touches is a non-blocking try or a short synchronous one, so
    /// the signature itself proves the probe cannot park behind the wedge it
    /// is trying to report.
    pub fn overdue_run_start_session_count(&self) -> Option<usize> {
        let sessions = self.sessions.try_read().ok()?;
        let mut overdue = 0_usize;
        let mut window_unreadable = false;
        for entry in sessions.values() {
            match entry.run_start_health() {
                crate::run_progress::RunStartHealth::Overdue => overdue += 1,
                crate::run_progress::RunStartHealth::Unreadable => window_unreadable = true,
                crate::run_progress::RunStartHealth::Clear => {}
            }
        }
        if overdue == 0 && window_unreadable {
            None
        } else {
            Some(overdue)
        }
    }

    /// How many registered sessions hold queued work they have parked: input
    /// in the machine's queued phase older than
    /// `run_progress::QUEUED_INPUT_START_NOTICE`, with the executor
    /// registration `Active` and no run in flight.
    ///
    /// This is the PRE-staging census - the class where a session resumes
    /// cleanly, keeps its loop alive, and simply never starts the work it was
    /// handed, which under `queue_mode: fifo` is a total outage for that
    /// session. Its post-staging sibling is
    /// [`Self::overdue_run_start_session_count`]; between them the pipeline
    /// from admission to first primitive application is covered. What NEITHER
    /// sees is a turn that begins and then produces nothing after
    /// `PrimitiveApplied` - mid-turn progress is a separate dimension with no
    /// probe yet, and no caller may read this count as covering it.
    ///
    /// `None` carries the same meaning and obligation as on its siblings: the
    /// registry, a session's authority, or a session's driver ledger was not
    /// readable, and nothing was positively parked - so a health caller must
    /// publish the failed read rather than a zero nobody measured. The
    /// per-entry miss propagates (rather than counting, as the dead-shell
    /// probe does) for the same reason as the run-start census: nothing here
    /// is established lock-free first, and a queued input is also what a
    /// healthy just-admitted input looks like, so counting an unread entry
    /// would assert a wedge nobody observed. When some other session IS
    /// positively parked, the observed fault wins and publishes.
    ///
    /// Not `async` on purpose, like its siblings: every lock this touches is
    /// a non-blocking try (including the driver's `tokio` mutex, whose
    /// `try_lock` is synchronous), so the signature proves the probe cannot
    /// park behind the wedge it is trying to report.
    pub fn parked_queued_input_session_count(&self) -> Option<usize> {
        let sessions = self.sessions.try_read().ok()?;
        let mut parked = 0_usize;
        let mut entry_unreadable = false;
        for entry in sessions.values() {
            match entry.parked_queued_work_health() {
                crate::run_progress::ParkedQueuedWorkHealth::Parked(_) => parked += 1,
                crate::run_progress::ParkedQueuedWorkHealth::Unreadable => entry_unreadable = true,
                crate::run_progress::ParkedQueuedWorkHealth::Clear => {}
            }
        }
        if parked == 0 && entry_unreadable {
            None
        } else {
            Some(parked)
        }
    }

    #[cfg(test)]
    pub(crate) async fn model_routing_handle_for_test(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::handles::RuntimeModelRoutingHandle>> {
        let (dsl_authority, handle_teardown_gate, durability_health, visibility_owner) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            (
                Arc::clone(&entry.dsl_authority),
                Arc::clone(&entry.handle_teardown_gate),
                entry.durability_health.clone(),
                Arc::clone(&entry.tool_visibility_owner),
            )
        };
        Some(Arc::new(
            crate::handles::RuntimeModelRoutingHandle::new_with_visibility_owner(
                Arc::new(
                    crate::handles::HandleDslAuthority::from_shared_with_runtime_gates(
                        dsl_authority,
                        handle_teardown_gate,
                        durability_health,
                    ),
                ),
                visibility_owner,
            ),
        ))
    }

    /// A copy of the current entry's mutation gate (M), which the caller
    /// takes through one of its accessors and holds across the full DSL-stage
    /// -> driver-mutate -> DSL-sync span. `None` if the session is not
    /// registered.
    async fn session_mutation_gate(&self, session_id: &SessionId) -> Option<SessionMutationGate> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| entry.mutation_gate.clone())
    }

    /// Take the current entry's mutation gate for live work, following a
    /// replaced entry to its new gate; `Ok(None)` once the session is not
    /// registered. Once the session degrades while another holder keeps the
    /// gate, the wait ends with the degradation's cause (see
    /// [`Self::lock_live_session_mutation_gate`]). Unlike the ready-gated
    /// wait, a free gate of a degraded entry is returned without settling it.
    async fn lock_current_live_session_mutation_gate(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<crate::tokio::sync::OwnedMutexGuard<()>>, DurabilityReloadRequired> {
        loop {
            let Some(gate) = self.session_mutation_gate(session_id).await else {
                return Ok(None);
            };
            let gate_guard = self
                .lock_live_session_mutation_gate(session_id, &gate)
                .await?;
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(None);
            };
            if entry.mutation_gate.same_gate(&gate) {
                return Ok(Some(gate_guard));
            }
        }
    }

    /// Take the current entry's mutation gate however long its holder keeps
    /// it, following a replaced entry to its new gate; `None` once the session
    /// is not registered. `reason` says why no holder the session's
    /// degradation can strand stands in the way.
    async fn lock_current_settled_session_mutation_gate(
        &self,
        session_id: &SessionId,
        reason: SettledGateWait,
    ) -> Option<crate::tokio::sync::OwnedMutexGuard<()>> {
        loop {
            let gate = self.session_mutation_gate(session_id).await?;
            let gate_guard = gate.lock_settled(reason).await;
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            if entry.mutation_gate.same_gate(&gate) {
                return Some(gate_guard);
            }
        }
    }

    /// Take `gate`, a copy of `session_id`'s mutation gate, for live work
    /// (see [`SessionMutationGate::lock_live`]).
    async fn lock_live_session_mutation_gate(
        &self,
        session_id: &SessionId,
        gate: &SessionMutationGate,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, DurabilityReloadRequired> {
        #[cfg(any(test, feature = "test-support"))]
        if gate.ends_on_degradation() {
            self.note_mutation_gate_wait_for_test(session_id, gate);
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = session_id;
        gate.lock_live().await
    }

    /// Take `gate`, a copy of `session_id`'s mutation gate, as `wait` says:
    /// live (see [`Self::lock_live_session_mutation_gate`]) or settled for its
    /// reason, which always takes it.
    #[cfg(feature = "live")]
    async fn lock_session_mutation_gate_as(
        &self,
        session_id: &SessionId,
        gate: &SessionMutationGate,
        wait: SessionMutationGateWait,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, DurabilityReloadRequired> {
        match wait {
            SessionMutationGateWait::Live => {
                self.lock_live_session_mutation_gate(session_id, gate).await
            }
            SessionMutationGateWait::Settled(reason) => Ok(gate.lock_settled(reason).await),
        }
    }

    /// Acquire the existing same-session mutation gate and require the exact
    /// persistent shell to remain durability-ready before returning it.
    ///
    /// A degraded entry is mechanically detached while the gate is held, so no
    /// successor mutation can race a still-running executor. The hard-cancel
    /// callback runs only after the session-map lock is released. Recovery and
    /// unregister paths that replace or tear down the entry intentionally use
    /// the lower-level gate above.
    ///
    /// A waiter parked behind a holder of the gate (for example a post-stop
    /// fence the exited runtime loop retains after a failed boundary commit)
    /// does not wait for it once the session degrades: it returns the typed
    /// `RecoveryRepairBlocked` and hands the degraded settlement (detach and
    /// exact hard-cancel) to a process-owned task that takes the gate when it
    /// frees. The gate is polled first, so a free gate settles inline as
    /// before.
    async fn lock_current_durability_ready_session_mutation_gate(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        loop {
            let gate = self.session_mutation_gate(session_id).await.ok_or(
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                },
            )?;
            let gate_guard = match self
                .lock_live_session_mutation_gate(session_id, &gate)
                .await
            {
                Ok(guard) => guard,
                Err(required) => {
                    // Had it taken the gate, this waiter would have settled
                    // the degraded entry itself.
                    self.hand_off_degraded_gate_settlement(session_id, gate);
                    return Err(degraded_gate_wait_refusal(required));
                }
            };
            match settle_durability_gate(&self.shared, session_id, &gate, gate_guard).await {
                DurabilityGateSettlement::Ready(guard) => return Ok(guard),
                DurabilityGateSettlement::Replaced => continue,
                DurabilityGateSettlement::Refused(error) => return Err(error),
            }
        }
    }

    /// Run the degraded settlement for a waiter that left on the degradation
    /// signal: a process-owned task takes the gate once its holder releases it
    /// and settles exactly as a ready-gated caller would, with the same
    /// detach and hard-cancel dedup. One task per gate: while one waits, the
    /// waiters that leave after it spawn nothing, since its settlement is
    /// theirs too. Without a cleanup runtime the next ready-gated caller
    /// settles inline instead.
    ///
    /// The other callers that leave a degraded session without settling it
    /// hand off the same way: an input admission whose store write degraded
    /// the session (past its deadline, or answered with an error), and the
    /// reload-required discard of a registration whose runtime loop is still
    /// attached.
    fn hand_off_degraded_gate_settlement(&self, session_id: &SessionId, gate: SessionMutationGate) {
        #[cfg(test)]
        self.test_degraded_gate_settlement_hand_offs
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let spawner = match MachineCleanupTaskSpawner::acquire() {
            Ok(spawner) => spawner,
            Err(error) => {
                tracing::warn!(
                    %session_id,
                    %error,
                    "could not hand off the durability-degraded gate settlement; the next ready-gated caller settles it"
                );
                return;
            }
        };
        let Some(pending) = PendingDegradedGateSettlement::claim(&self.shared, &gate) else {
            return;
        };
        #[cfg(test)]
        self.test_degraded_gate_settlement_spawns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let shared = Arc::clone(&self.shared);
        let session_id = session_id.clone();
        drop(spawner.spawn(async move {
            let _pending = pending;
            let gate_guard = gate.lock_settled(SettledGateWait::DegradedSettlement).await;
            let _ = settle_durability_gate(&shared, &session_id, &gate, gate_guard).await;
        }));
    }

    /// Deterministically pause the runtime loop's next terminal run commit
    /// of `session_id` before it takes the session driver (the turn is
    /// terminal in its live agent, its run input applied and unconsumed).
    /// The first receiver resolves when the commit reaches the gate; sending
    /// on (or dropping) the returned sender lets it go on. Exposed only by
    /// test builds.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_runtime_loop_before_terminal_commit_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_runtime_loop_before_terminal_commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "runtime-loop terminal-commit test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn run_runtime_loop_before_terminal_commit_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_runtime_loop_before_terminal_commit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    /// Deterministically pause the runtime loop's next terminal run commit
    /// of `session_id` after the store commit landed and before the
    /// committed boundary is acknowledged to the session executor. The
    /// turn-finalization boundary stays held. The first receiver resolves
    /// when the commit reaches the gate; sending on (or dropping) the
    /// returned sender lets it go on. Exposed only by test builds.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_runtime_loop_before_boundary_acknowledgement_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_runtime_loop_before_boundary_acknowledgement
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "runtime-loop boundary-acknowledgement test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn run_runtime_loop_before_boundary_acknowledgement_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_runtime_loop_before_boundary_acknowledgement
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    /// Deterministically pause the next executor attachment commit of
    /// `session_id` that owes interrupted-run notices, after its publication
    /// (the runtime loop serves and was woken) and before the notices are
    /// handed to the session. The first receiver resolves when the commit
    /// reaches the gate; sending on (or dropping) the returned sender lets it
    /// go on. Exposed only by test builds.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_attachment_interrupted_notice_handoff_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_attachment_interrupted_notice_handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "attachment interrupted-notice hand-off test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn run_attachment_interrupted_notice_handoff_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_attachment_interrupted_notice_handoff
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    /// Deterministically pause `session_id`'s runtime loop at its next
    /// pre-dequeue check that finds no owed interrupted-run notice, with the
    /// turn-finalization boundary held. The first receiver resolves when the
    /// loop reaches the gate; sending on (or dropping) the returned sender
    /// lets it go on. Exposed only by test builds.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_runtime_loop_after_empty_interrupted_notice_check_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_runtime_loop_after_empty_interrupted_notice_check
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "runtime-loop empty interrupted-notice check test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn run_runtime_loop_after_empty_interrupted_notice_check_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_runtime_loop_after_empty_interrupted_notice_check
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    /// Runtime-loop parks on held run starts (#1500). Test support: wait for
    /// the park as a positive event instead of a quiet period.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn run_start_held_parks(&self) -> crate::tokio::sync::watch::Receiver<u64> {
        self.test_run_start_held_parks.subscribe()
    }

    /// Whether `session_id`'s run starts are held (#1500). Test support.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn run_starts_held_for_test(&self, session_id: &SessionId) -> Option<bool> {
        self.session_dsl_state(session_id)
            .await
            .ok()
            .map(|state| !state.run_start_holds.is_empty())
    }

    /// Boundary cancels dispatched to an executor's boundary handle (#1471).
    /// Test support: wait for the dispatch as a positive event.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn boundary_cancel_dispatches(&self) -> crate::tokio::sync::watch::Receiver<u64> {
        self.test_boundary_cancel_dispatches.subscribe()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn note_run_start_held_park(&self) {
        self.test_run_start_held_parks
            .send_modify(|parks| *parks = parks.wrapping_add(1));
    }

    /// Record whether `session_id`'s runtime loop is parked: set as it awaits
    /// its next wake with no buffered wake or effect, cleared the moment any
    /// of them fires. Test support.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn note_runtime_loop_parked(&self, session_id: &SessionId, parked: bool) {
        self.test_runtime_loop_parked.send_if_modified(|sessions| {
            if parked {
                sessions.insert(session_id.clone())
            } else {
                sessions.remove(session_id)
            }
        });
    }

    /// Sessions whose runtime loop is parked: it awaits its next wake with
    /// none buffered, so input admitted now without a wake stays queued
    /// unless something genuinely wakes the loop. Test support: a positive
    /// event, never a state poll, which cannot see a wake buffered while the
    /// loop was busy. A loop can park while a run is in flight (it awaits the
    /// run's effect), so a test pairs the park with the settled phase.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn runtime_loop_parked(
        &self,
    ) -> crate::tokio::sync::watch::Receiver<std::collections::HashSet<SessionId>> {
        self.test_runtime_loop_parked.subscribe()
    }

    /// Buffer one wake for `session_id`'s runtime loop without admitting
    /// input, as a wake sent while the loop is busy is buffered. Test
    /// support for the parked-wait regression.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn buffer_runtime_loop_wake_for_test(&self, session_id: &SessionId) -> bool {
        let wake_tx = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .and_then(|entry| entry.wake_sender())
        };
        wake_tx.is_some_and(|wake_tx| wake_tx.try_send(()).is_ok())
    }

    /// Delay every [`Self::hold_run_starts`] by `delay` before it takes the
    /// session's mutation gate, standing in for a hold that waits on a busy
    /// session (a running turn's commit holds that gate); `None` clears it.
    /// Test builds only.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn delay_hold_run_starts_for_test(&self, delay: Option<std::time::Duration>) {
        self.run_start_holds_for_test().delay = delay;
    }

    /// Fail every [`Self::hold_run_starts`] of `session_id`, after its delay,
    /// as a member's hold can fail; `None` clears it. Test builds only.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn fail_hold_run_starts_for_test(&self, session_id: Option<SessionId>) {
        self.run_start_holds_for_test().failing_session = session_id;
    }

    /// The most [`Self::hold_run_starts`] calls in flight at once since the
    /// last take, which restarts the count. Test builds only.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn take_max_in_flight_hold_run_starts_for_test(&self) -> usize {
        let mut seam = self.run_start_holds_for_test();
        let in_flight = seam.in_flight;
        std::mem::replace(&mut seam.max_in_flight, in_flight)
    }

    /// Enter [`Self::hold_run_starts`]'s test seam: the hold counts as in
    /// flight until the returned guard drops, waits the delay
    /// [`Self::delay_hold_run_starts_for_test`] set, and fails when
    /// [`Self::fail_hold_run_starts_for_test`] named its session.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn enter_hold_run_starts_for_test(
        &self,
        session_id: &SessionId,
    ) -> Result<RunStartHoldInFlightForTest<'_>, RuntimeDriverError> {
        let (delay, fails) = {
            let mut seam = self.run_start_holds_for_test();
            seam.in_flight += 1;
            seam.max_in_flight = seam.max_in_flight.max(seam.in_flight);
            (
                seam.delay,
                seam.failing_session.as_ref() == Some(session_id),
            )
        };
        let in_flight = RunStartHoldInFlightForTest { machine: self };
        if let Some(delay) = delay {
            crate::tokio::time::sleep(delay).await;
        }
        if fails {
            return Err(RuntimeDriverError::Internal(format!(
                "injected run-start hold failure for session {session_id}"
            )));
        }
        Ok(in_flight)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn run_start_holds_for_test(&self) -> std::sync::MutexGuard<'_, RunStartHoldsForTest> {
        self.test_run_start_holds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Deterministically pause the runtime loop after its ready-effect drain
    /// and before queue authority is acquired. Exposed only by test builds so
    /// cross-crate integration tests can admit a complete same-boundary batch.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_runtime_loop_before_queue_authority_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_runtime_loop_before_queue_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "runtime-loop queue-authority test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    /// Deterministically pause the runtime loop after it staged a run and
    /// signalled its turn start, immediately before the executor's `apply`
    /// (#1471). Test builds only.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_runtime_loop_before_executor_apply_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_runtime_loop_before_executor_apply
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "runtime-loop executor-apply test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    /// Deterministically pause the runtime loop's next lap that stages a
    /// fresh batch after it read the batch's durable rows and before its
    /// staging commit. Test builds only.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_runtime_loop_before_batch_staging_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_runtime_loop_before_batch_staging
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "runtime-loop batch-staging test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn run_runtime_loop_before_batch_staging_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_runtime_loop_before_batch_staging
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn run_runtime_loop_before_executor_apply_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_runtime_loop_before_executor_apply
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    /// Deterministically hold the next scoped context binding of
    /// `session_id` immediately before its driver takes the session
    /// document's machine authority ([`Self::bind_scoped_context_within`]):
    /// its registration, mutation gate and driver are held, and its binder's
    /// final decision has not run. The receiver resolves when the binding
    /// arrives there; sending on or dropping the sender releases it. Test
    /// builds only.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_scoped_binding_before_machine_authority_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut hook = self
            .test_scoped_binding_before_machine_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "scoped binding machine-authority test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    /// The binding's side of
    /// [`Self::arm_scoped_binding_before_machine_authority_test_hook`]: a
    /// synchronous wait, as the wait for the machine authority is.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn run_scoped_binding_before_machine_authority_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_scoped_binding_before_machine_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn run_runtime_loop_before_queue_authority_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_runtime_loop_before_queue_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    /// Test-support: arm the runtime loop's staged -> executing bound for one
    /// session at `bound` instead of the production hour (`None` restores
    /// it). Lets downstream lanes force the real `ExecutorNotProgressing`
    /// teardown through a full stack whose executor never begins its turn.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn set_run_execution_start_bound_for_test(
        &self,
        session_id: &SessionId,
        bound: Option<std::time::Duration>,
    ) {
        crate::run_progress::override_run_execution_start_bound_for_session(session_id, bound);
    }

    /// Pause one exact ReloadRequired discard after its cold successor is
    /// current but before the detached coordinator acknowledges success.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_reload_required_discard_after_successor_publication_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_reload_required_discard_after_successor_publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "ReloadRequired successor-publication test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    /// Pause one ReloadRequired discard worker of `session_id` after it
    /// released the registration transaction and claimed any teardown
    /// handoff, before it takes the registration's mutation gate.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_reload_required_discard_before_mutation_gate_test_hook(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_reload_required_discard_before_mutation_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "ReloadRequired before-mutation-gate test hook already armed"
        );
        *hook = Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    /// Install a fresh runtime-loop teardown slot on the current registration
    /// of `session_id`, as a runtime loop attached to it would, when it has
    /// none. A real loop can reach a durability-degraded registration this
    /// way: an executor registration checks durability when it claims the
    /// session, releases the registration transaction while it still holds
    /// the mutation gate, and attaches its loop without checking again, so a
    /// session that degrades in between gets a loop after a discard was
    /// admitted. Tests use this to place that attach inside a discard's
    /// window deterministically. `false` when the session is not registered
    /// or already has one.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn install_runtime_loop_teardown_slot_for_test(
        &self,
        session_id: &SessionId,
    ) -> bool {
        let mut sessions = self.sessions.write().await;
        match sessions.get_mut(session_id) {
            Some(entry) if entry.runtime_loop_teardown.is_none() => {
                entry.runtime_loop_teardown =
                    Some(crate::runtime_loop::RuntimeLoopTeardownSlot::pending());
                true
            }
            _ => false,
        }
    }

    /// Force one persistent session's shared durability gate into
    /// ReloadRequired without fabricating a store outcome. Test-only callers
    /// use this to exercise cancellation around the real disposal coordinator.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn force_session_durability_reload_required_for_test(
        &self,
        session_id: &SessionId,
    ) -> bool {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|entry| entry.durability_health.as_ref())
            .is_some_and(|health| {
                health.mark_reload_required(
                    "test_reload_required_discard",
                    "deterministic test fault",
                )
            })
    }

    /// Report the next degradation-aware wait for `session_id`'s mutation
    /// gate that finds the gate held: the receiver resolves once that waiter
    /// has subscribed to the session's degradation and is about to park
    /// behind the holder, so a test can degrade the session during the wait
    /// instead of before it. Test support.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_mutation_gate_wait_observer_for_test(
        &self,
        session_id: SessionId,
    ) -> crate::tokio::sync::oneshot::Receiver<()> {
        let (parked_tx, parked_rx) = crate::tokio::sync::oneshot::channel();
        let replaced = self
            .test_mutation_gate_wait_observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace((session_id, parked_tx));
        assert!(
            replaced.is_none(),
            "mutation gate wait observer already armed"
        );
        parked_rx
    }

    /// Measure the commit outcome declarations of the runtime loops this
    /// machine starts from now on in busy windows of `window` (test support).
    /// A store declares its deadline as a sum of busy windows, so this
    /// shortens each window and keeps the declaration; it does not override
    /// the deadline.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn set_runtime_commit_busy_window_for_test(&self, window: std::time::Duration) {
        *self
            .test_runtime_commit_busy_window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(window);
    }

    #[cfg(any(test, feature = "test-support"))]
    fn runtime_commit_deadline_for_test(
        &self,
        commit_deadline: crate::runtime_loop::RuntimeLoopCommitDeadline,
    ) -> crate::runtime_loop::RuntimeLoopCommitDeadline {
        let window = *self
            .test_runtime_commit_busy_window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match window {
            Some(window) => commit_deadline.with_busy_window(window),
            None => commit_deadline,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn note_mutation_gate_wait_for_test(&self, session_id: &SessionId, gate: &SessionMutationGate) {
        if gate.try_lock().is_ok() {
            return;
        }
        let parked_tx = {
            let mut observer = self
                .test_mutation_gate_wait_observer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if observer
                .as_ref()
                .is_some_and(|(armed_session_id, _)| armed_session_id == session_id)
            {
                observer.take().map(|(_, parked_tx)| parked_tx)
            } else {
                None
            }
        };
        if let Some(parked_tx) = parked_tx {
            let _ = parked_tx.send(());
        }
    }

    /// Persist the exact current operation registry using the registration's
    /// own durable epoch and cursor authority. This is a deterministic fault
    /// fixture seam: cancellation tests snapshot the real pre-fault authority
    /// before injecting ReloadRequired, so cold recovery cannot be vacuous.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn persist_current_ops_lifecycle_for_test(
        &self,
        session_id: &SessionId,
    ) -> Result<(), String> {
        let (store, runtime_id, snapshot) = {
            let store = self
                .store
                .clone()
                .ok_or_else(|| "runtime has no durable store".to_string())?;
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or_else(|| format!("session {session_id} is not registered"))?;
            let snapshot = entry
                .ops_lifecycle
                .capture_persistence_snapshot(entry.epoch_id.clone(), &entry.cursor_state)
                .map_err(|error| error.to_string())?;
            (store, entry.runtime_id.clone(), snapshot)
        };
        store
            .persist_ops_lifecycle(&runtime_id, &snapshot)
            .await
            .map_err(|error| error.to_string())
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn run_reload_required_discard_before_mutation_gate_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_reload_required_discard_before_mutation_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn run_reload_required_discard_after_successor_publication_test_hook(
        &self,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_reload_required_discard_after_successor_publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    #[cfg(test)]
    fn arm_control_command_after_logical_lookup_test_hook(
        &self,
        kind: ControlCommandLookupTestKind,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut hook = self
            .test_control_command_after_logical_lookup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            hook.is_none(),
            "control-command lookup test hook already armed"
        );
        *hook = Some((kind, session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    #[cfg(test)]
    async fn run_control_command_after_logical_lookup_test_hook(
        &self,
        kind: ControlCommandLookupTestKind,
        session_id: &SessionId,
    ) {
        let armed = {
            let mut hook = self
                .test_control_command_after_logical_lookup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hook
                .as_ref()
                .is_some_and(|(armed_kind, armed_session_id, _, _)| {
                    *armed_kind == kind && armed_session_id == session_id
                })
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, _, entered_tx, release_rx)) = armed {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    async fn lock_registration_gate(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        let gate_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let blocked = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            entry.registration_blocked_by_unregister(session_id)
        };
        if let Some(error) = blocked {
            return Err(error);
        }
        Ok(gate_guard)
    }

    #[cfg(feature = "live")]
    async fn session_live_lifecycle_gate(&self, session_id: &SessionId) -> Option<Arc<Mutex<()>>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| Arc::clone(&entry.live_lifecycle_gate))
    }

    #[cfg(feature = "live")]
    async fn lock_current_session_live_lifecycle_gate(
        &self,
        session_id: &SessionId,
    ) -> Option<(Arc<Mutex<()>>, crate::tokio::sync::OwnedMutexGuard<()>)> {
        loop {
            let gate = self.session_live_lifecycle_gate(session_id).await?;
            let guard = Arc::clone(&gate).lock_owned().await;
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            if Arc::ptr_eq(&entry.live_lifecycle_gate, &gate) {
                return Some((gate, guard));
            }
        }
    }

    /// The mutation gate tied to `lease`'s exact live lifecycle gate, taken as
    /// `wait` says; `Ok(None)` once the entry is gone or no longer `lease`'s.
    #[cfg(feature = "live")]
    async fn lock_session_mutation_gate_for_live_lifecycle_lease(
        &self,
        session_id: &SessionId,
        lease: &crate::member_live::MemberLiveLifecycleLease,
        wait: SessionMutationGateWait,
    ) -> Result<Option<crate::tokio::sync::OwnedMutexGuard<()>>, DurabilityReloadRequired> {
        let mutation_gate = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(None);
            };
            if !lease.matches_gate(&entry.live_lifecycle_gate) {
                return Ok(None);
            }
            entry.mutation_gate.clone()
        };
        let guard = self
            .lock_session_mutation_gate_as(session_id, &mutation_gate, wait)
            .await?;
        let sessions = self.sessions.read().await;
        let Some(entry) = sessions.get(session_id) else {
            return Ok(None);
        };
        if lease.matches_gate(&entry.live_lifecycle_gate)
            && entry.mutation_gate.same_gate(&mutation_gate)
        {
            Ok(Some(guard))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn lock_current_session_driver_gate(
        &self,
        session_id: &SessionId,
        driver: &SharedDriver,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        let gate_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if !Arc::ptr_eq(&entry.driver, driver) {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            }
            // A settlement's internal registration is the settlement's alone:
            // a path that takes M without staging a DSL input (a cancel point,
            // an archive lease) is refused here, after the gate.
            if entry.is_predecessor_settlement() {
                return Err(RuntimeDriverError::UnregisterInProgress {
                    runtime_id: entry.runtime_id.clone(),
                });
            }
        }
        Ok(gate_guard)
    }

    pub(crate) async fn lock_current_runtime_loop_driver_authority(
        &self,
        session_id: &SessionId,
        driver: &SharedDriver,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        let gate_guard = self
            .lock_current_session_driver_gate(session_id, driver)
            .await?;
        {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if !entry.generated_executor_registration_active_or_draining() {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason:
                        "generated MeerkatMachine has no active runtime-loop executor registration"
                            .to_string(),
                });
            }
        }
        Ok(gate_guard)
    }

    /// Acquire queue-admission authority for a current runtime-loop driver.
    ///
    /// Existing run commit and terminalization must remain available while an
    /// unregister is Draining, but no new queued run may cross that boundary.
    /// Holding the session mutation gate while checking Active gives queue
    /// admission and BeginUnregisterSession one total order.
    pub(crate) async fn lock_current_runtime_loop_queue_driver_authority(
        &self,
        session_id: &SessionId,
        driver: &SharedDriver,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        let gate_guard = self
            .lock_current_session_driver_gate(session_id, driver)
            .await?;
        {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if !entry.generated_executor_registration_active() {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason:
                        "generated MeerkatMachine has no active runtime-loop queue registration"
                            .to_string(),
                });
            }
        }
        Ok(gate_guard)
    }

    pub(crate) async fn current_runtime_stop_cleanup_in_progress(
        &self,
        session_id: &SessionId,
        driver: &SharedDriver,
    ) -> Result<bool, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if !Arc::ptr_eq(&entry.driver, driver) {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        }
        let Some(coordinator) = entry.runtime_stop_cleanup_coordinator.as_ref() else {
            return Ok(false);
        };
        if coordinator.epoch_id != entry.epoch_id {
            return Err(RuntimeDriverError::Internal(format!(
                "stale runtime-stop cleanup coordinator epoch for session {session_id}"
            )));
        }
        Ok(coordinator.result_rx.borrow().is_none())
    }

    async fn session_dsl_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>, String> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| Arc::clone(&entry.dsl_authority))
            .ok_or_else(|| {
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                }
                .to_string()
            })
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn session_handle_teardown_gate(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<crate::handles::HandleTeardownGate>, String> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| Arc::clone(&entry.handle_teardown_gate))
            .ok_or_else(|| {
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                }
                .to_string()
            })
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn session_durability_health(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<DurabilityHealthHandle>, String> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| entry.durability_health.clone())
            .ok_or_else(|| {
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                }
                .to_string()
            })
    }

    /// Report whether `session_id`'s registration is durability-degraded.
    ///
    /// Reads the same per-session gate the ingress admission path consults
    /// (`RuntimeSessionEntry::require_durability_ready`) without taking the
    /// session's mutation gate. `None` means the session is either ready,
    /// ephemeral (no durability gate), or not registered on this machine: an
    /// absent registration is not a durability verdict and is reported by the
    /// registration seams instead.
    pub async fn durability_reload_required(
        &self,
        session_id: &SessionId,
    ) -> Option<SessionDurabilityReloadRequired> {
        let sessions = self.sessions.read().await;
        let required = sessions.get(session_id)?.require_durability_ready().err()?;
        Some(SessionDurabilityReloadRequired {
            operation: required.operation().to_string(),
            reason: required.reason().to_string(),
        })
    }

    /// `true` unless `session_id` is registered here and durability-degraded.
    pub async fn is_durability_ready(&self, session_id: &SessionId) -> bool {
        self.durability_reload_required(session_id).await.is_none()
    }

    /// Test-support: install the session's generated peer-comms handle (and its
    /// owner token) onto a comms runtime, so the runtime accepts generated trust
    /// mutations minted from THIS adapter's session dsl authority. Mirrors what
    /// `prepare_session_runtime_bindings` does in production via
    /// `SessionRuntimeBindings`, for tests/harnesses that construct external
    /// member runtimes directly (e.g. the external-TCP production-drain smoke
    /// lane). Gated behind `test-support` so it never reaches a production build.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn test_install_session_peer_comms_handle_on_runtime(
        &self,
        session_id: &SessionId,
        runtime: &(dyn meerkat_core::handles::PeerCommsInstallTarget + '_),
    ) -> Result<(), String> {
        let dsl = self
            .session_dsl_authority(session_id)
            .await
            .map_err(|error| format!("session dsl authority unavailable: {error}"))?;
        let teardown_gate = self
            .session_handle_teardown_gate(session_id)
            .await
            .map_err(|error| format!("session handle teardown gate unavailable: {error}"))?;
        let durability_health = self
            .session_durability_health(session_id)
            .await
            .map_err(|error| format!("session durability health unavailable: {error}"))?;
        let handle = std::sync::Arc::new(
            crate::handles::HandleDslAuthority::from_shared_with_runtime_gates(
                dsl,
                teardown_gate,
                durability_health,
            ),
        );
        crate::handles::RuntimePeerCommsHandle::install_generated_on(handle, runtime)
    }

    fn preview_dsl_input_on_authority(
        mut preview: dsl::MeerkatMachineAuthority,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<dsl::MeerkatMachineEffect>, String> {
        dsl::MeerkatMachineMutator::apply(&mut preview, input)
            .map(|transition| transition.into_effects())
            .map_err(|err| dsl_authority::map_error(err, context))
    }

    async fn preview_session_dsl_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<dsl::MeerkatMachineEffect>, String> {
        let authority = self.session_dsl_authority(session_id).await?;
        Self::preview_dsl_input_on_shared_authority(&authority, input, context)
    }

    /// Preview `input` on a fork of one session's live authority.
    fn preview_dsl_input_on_shared_authority(
        authority: &std::sync::Mutex<dsl::MeerkatMachineAuthority>,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<dsl::MeerkatMachineEffect>, String> {
        let preview = match authority.lock() {
            Ok(authority) => Ok(authority.fork()),
            Err(poisoned) => {
                // Live forks require a healthy lock. A possibly partial state
                // still crosses the existing cold-recovery validation boundary.
                let state = poisoned.into_inner().state().clone();
                dsl::MeerkatMachineAuthority::recover_from_state(state)
            }
        }
        .map_err(|err| dsl_authority::map_error(err, context))?;
        Self::preview_dsl_input_on_authority(preview, input, context)
    }

    /// The run the machine currently records for `session_id`, if any (`None`
    /// also for a session that is not registered).
    pub async fn current_run(&self, session_id: &SessionId) -> Option<RunId> {
        let authority = self.session_dsl_authority(session_id).await.ok()?;
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        dsl_authority::current_run_id_from_authority(&authority)
    }

    /// The turn terminal outcome the machine retains for exactly `run_id` of
    /// `session_id`: `None` while that run's turn has not terminalized, when
    /// the retained terminal witness belongs to another run, or when the
    /// session is not registered.
    pub async fn run_turn_terminal(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Option<meerkat_core::turn_execution_authority::TurnTerminalOutcome> {
        let authority = self.session_dsl_authority(session_id).await.ok()?;
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        if state.turn_terminal_run_id.as_ref() != Some(&dsl::RunId::from_domain(run_id)) {
            return None;
        }
        state
            .terminal_outcome
            .map(dsl_authority::core_turn_terminal_outcome)
    }

    /// Wait until `run_id` is no longer the run the machine records for
    /// `session_id`.
    ///
    /// The runtime records a run's end in the session's machine state; a
    /// session service reporting its turn inactive precedes that record, so a
    /// caller that must act only once a run is over (a stopped member about
    /// to be unregistered or resumed) waits here instead. Typed and event
    /// driven: the runtime loop that executes the run signals after recording
    /// its end, and this re-reads machine truth on every signal, subscribing
    /// before the first read so no signal between read and wait is lost.
    /// Unbounded: callers bound it with their own deadline.
    pub async fn wait_run_settled(&self, session_id: &SessionId, run_id: &RunId) {
        let mut settlements = self.run_settlements.subscribe();
        while self.current_run(session_id).await.as_ref() == Some(run_id) {
            if settlements.changed().await.is_err() {
                return;
            }
        }
    }

    /// Whether `session_id` has an active live channel, at this instant
    /// (`false` also for a session that is not registered). A point-in-time
    /// observation: no lock is held across an await, and a channel opened
    /// right after it is not seen.
    pub async fn session_live_channel_active(&self, session_id: &SessionId) -> bool {
        let Ok(authority) = self.session_dsl_authority(session_id).await else {
            return false;
        };
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authority
            .state()
            .live_active_channel_by_session
            .contains_key(&session_id.to_string())
    }

    /// Wait until the run current for `session_id` when this is called is no
    /// longer current, and return its id; `None` when no run was current.
    /// See [`Self::wait_run_settled`].
    pub async fn wait_current_run_settled(&self, session_id: &SessionId) -> Option<RunId> {
        let run_id = self.current_run(session_id).await?;
        self.wait_run_settled(session_id, &run_id).await;
        Some(run_id)
    }

    /// Observe run settlements: the generation advances after each runtime
    /// loop iteration, each run iteration and each loop exit of this machine,
    /// whenever a run may have ended. It carries no state; an observer
    /// re-reads the authority it cares about on every advance. In-process
    /// only.
    pub fn subscribe_run_settlements(&self) -> crate::tokio::sync::watch::Receiver<u64> {
        self.run_settlements.subscribe()
    }

    /// Wake every [`Self::wait_run_settled`] waiter to re-check.
    pub(crate) fn publish_run_settlement(&self) {
        self.run_settlements
            .send_modify(|settlements| *settlements = settlements.wrapping_add(1));
    }

    async fn session_dsl_state(
        &self,
        session_id: &SessionId,
    ) -> Result<dsl::MeerkatMachineState, RuntimeControlPlaneError> {
        let authority = self
            .session_dsl_authority(session_id)
            .await
            .map_err(RuntimeControlPlaneError::Internal)?;
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(authority.state().clone())
    }

    async fn commit_session_dsl_transition(
        &self,
        session_id: &SessionId,
        staged: StagedSessionDslInput,
        context: &str,
    ) -> Result<(), String> {
        self.commit_session_dsl_transition_with_dispatch_failure(
            session_id,
            staged,
            context,
            CommittedEffectDispatchFailure::PreserveCommittedDslState,
        )
        .await
    }

    async fn commit_session_dsl_transition_preserving_committed_state(
        &self,
        session_id: &SessionId,
        staged: StagedSessionDslInput,
        context: &str,
    ) -> Result<(), String> {
        self.commit_session_dsl_transition_with_dispatch_failure(
            session_id,
            staged,
            context,
            CommittedEffectDispatchFailure::PreserveCommittedDslState,
        )
        .await
    }

    async fn commit_session_dsl_transition_with_dispatch_failure(
        &self,
        _session_id: &SessionId,
        staged: StagedSessionDslInput,
        context: &str,
        dispatch_failure: CommittedEffectDispatchFailure,
    ) -> Result<(), String> {
        if let Err(error) = self
            .dispatch_routed_signals_from_effects(
                staged.signal_dispatcher.as_ref(),
                &staged.effects,
            )
            .await
        {
            let CommittedEffectDispatchFailure::PreserveCommittedDslState = dispatch_failure;
            return Err(format!(
                "DSL authority ({context}): committed effect dispatch failed: {error}"
            ));
        }
        Ok(())
    }

    async fn dispatch_routed_signals_from_effects(
        &self,
        dispatcher: Option<&composition::MeerkatCompositionSignalDispatcher>,
        effects: &[dsl::MeerkatMachineEffect],
    ) -> Result<(), String> {
        let Some(dispatcher) = dispatcher else {
            return Ok(());
        };

        for effect in effects {
            if let Some(signal) = composition::lift_routed_signal(effect) {
                composition::dispatch_routed_signal(dispatcher, signal).await?;
            }
        }
        Ok(())
    }

    // The wrapper transfers one exact, non-cloneable dispatch transaction to
    // process-owned cleanup; a params bag would duplicate that authority shape.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_cancel_after_boundary_runtime_effect(
        &self,
        session_id: &SessionId,
        witness: RuntimeEffectDispatchAttachmentWitness,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
        member_authority: Option<RuntimeEffectDispatchMemberAuthority>,
        pending_dispatch: PendingBoundaryCancelDispatchGuard,
        expected_run_id: Option<&RunId>,
        projected_effect: crate::effect::ProjectedRuntimeEffect,
        dispatch_generation: u64,
        dispatch_lifecycle_phase: dsl::MeerkatPhase,
        allow_during_unregister_drain: bool,
        context: &str,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;
        let machine = self.clone();
        let session_id = session_id.clone();
        let expected_run_id = expected_run_id.cloned();
        let context = context.to_string();
        let completion = cleanup_spawner.spawn(async move {
            machine
                .dispatch_cancel_after_boundary_runtime_effect_owned(
                    &session_id,
                    witness,
                    held_mutation_gate,
                    boundary_handle,
                    member_authority,
                    pending_dispatch,
                    expected_run_id.as_ref(),
                    projected_effect,
                    dispatch_generation,
                    dispatch_lifecycle_phase,
                    allow_during_unregister_drain,
                    &context,
                )
                .await
        });
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "process-owned boundary-effect dispatch ended without a result: {error}"
            ))
        })?
    }

    // Keep every exact attachment and pending-dispatch owner visible at the
    // cancellation-safe process boundary rather than re-packaging authority.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_cancel_after_boundary_runtime_effect_owned(
        &self,
        session_id: &SessionId,
        witness: RuntimeEffectDispatchAttachmentWitness,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
        member_authority: Option<RuntimeEffectDispatchMemberAuthority>,
        mut pending_dispatch: PendingBoundaryCancelDispatchGuard,
        expected_run_id: Option<&RunId>,
        projected_effect: crate::effect::ProjectedRuntimeEffect,
        dispatch_generation: u64,
        dispatch_lifecycle_phase: dsl::MeerkatPhase,
        allow_during_unregister_drain: bool,
        context: &str,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        // Executor callbacks are allowed to route back into the machine. Drop
        // M before invoking one, then reacquire the exact captured gate and
        // prove that neither the logical session nor attachment incarnation
        // changed before the queued effect is published.
        drop(held_mutation_gate);
        let live_dispatch_result = self
            .dispatch_cancel_after_boundary_live_handle(
                boundary_handle,
                expected_run_id,
                &projected_effect,
                context,
            )
            .await;
        #[cfg(any(test, feature = "test-support"))]
        if live_dispatch_result.is_ok() {
            self.test_boundary_cancel_dispatches
                .send_modify(|dispatches| *dispatches = dispatches.wrapping_add(1));
        }

        // Reserve bounded-channel capacity without M. A wedged executor may
        // delay this process-owned transaction, but it cannot retain the
        // session mutation authority or block teardown/replacement.
        let pre_reserve_state = Self::classify_dsl_boundary_cancel_dispatch(
            &witness.dsl_authority,
            dispatch_generation,
            &dispatch_lifecycle_phase,
            expected_run_id,
        );
        let effect_permit = if live_dispatch_result.is_ok()
            && pre_reserve_state == BoundaryCancelDispatchState::Current
        {
            Some(witness.effect_tx.clone().reserve_owned().await)
        } else {
            None
        };

        let gate_guard = self
            .lock_live_session_mutation_gate(session_id, &witness.mutation_gate)
            .await
            .map_err(degraded_gate_wait_refusal)?;
        let current_effect_tx = {
            let sessions = self.sessions.read().await;
            match sessions.get(session_id) {
                None => Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "{context}: runtime session disappeared during boundary callback"
                    ),
                }),
                Some(entry)
                    if !entry.mutation_gate.same_gate(&witness.mutation_gate)
                        || !Arc::ptr_eq(&entry.driver, &witness.driver)
                        || !Arc::ptr_eq(&entry.dsl_authority, &witness.dsl_authority)
                        || !entry.owns_runtime_loop_attachment(witness.attachment_id) =>
                {
                    Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "{context}: runtime attachment changed during boundary callback"
                        ),
                    })
                }
                Some(entry) => {
                    entry.require_durability_ready().map_err(|required| {
                        RuntimeDriverError::RecoveryRepairBlocked {
                            evidence_digest: None,
                            reason: required.to_string(),
                        }
                    })?;
                    match entry.effect_sender() {
                        None => Err(RuntimeDriverError::StaleAuthority {
                            reason: format!(
                                "{context}: runtime effect channel disappeared during boundary callback"
                            ),
                        }),
                        Some(current_effect_tx)
                            if !witness.effect_tx.same_channel(&current_effect_tx) =>
                        {
                            Err(RuntimeDriverError::StaleAuthority {
                                reason: format!(
                                    "{context}: runtime effect channel changed during boundary callback"
                                ),
                            })
                        }
                        Some(current_effect_tx) => Ok(current_effect_tx),
                    }
                }
            }
        };
        let current_effect_tx = current_effect_tx?;

        if let Some(member_authority) = &member_authority {
            self.validate_member_effect_authority_lease_current(
                session_id,
                &member_authority.lease,
                member_authority.expected_member.as_ref(),
            )?;
        }

        // Callback failure is meaningful only while the captured attachment
        // remains current. If replacement B won while A's callback was
        // outside M, the exact checks above return StaleAuthority first so A's
        // result cannot be mistaken for B's or trigger a caller-side retry on
        // B under the old request.
        if let Err(error) = live_dispatch_result {
            return Err(error);
        }

        // The live turn may have crossed its boundary while M was released.
        // That is successful convergence; do not replay the old effect into a
        // successor phase. A same-generation orphan in another phase is
        // cleared by the generated abort input.
        match Self::classify_dsl_boundary_cancel_dispatch(
            &witness.dsl_authority,
            dispatch_generation,
            &dispatch_lifecycle_phase,
            expected_run_id,
        ) {
            BoundaryCancelDispatchState::ConsumedConverged => return Ok(gate_guard),
            BoundaryCancelDispatchState::SameGenerationOrphan
            | BoundaryCancelDispatchState::Superseded => {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "{context}: boundary-cancel dispatch authority changed during callback"
                    ),
                });
            }
            BoundaryCancelDispatchState::Current => {}
        }

        let state = self
            .existing_session_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);
        if !allow_during_unregister_drain {
            self.reject_unregistration_drain_ingress(session_id, state)
                .await?;
        }

        debug_assert!(witness.effect_tx.same_channel(&current_effect_tx));
        let effect_permit = effect_permit
            .ok_or_else(|| RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "{context}: boundary-cancel dispatch became current after capacity reservation was skipped"
                ),
            })?
            .map_err(|_| RuntimeDriverError::NotReady {
                state: RuntimeState::Idle,
            })?;
        let _effect_tx = effect_permit.send(projected_effect.into_effect());
        pending_dispatch.disarm();
        Ok(gate_guard)
    }

    /// Transfer every actor-bound post-admission optimization away from the
    /// durable admission acknowledgement. The input and its completion waiter
    /// are already committed before this is called; actor slowness can delay
    /// exact injection but can no longer delay acceptance of this or later
    /// inputs. The admission's owed runless terminals move with its mutation
    /// guard and are told once this work has released it.
    #[allow(clippy::too_many_arguments)]
    fn spawn_accepted_ingress_boundary_work(
        &self,
        cleanup_spawner: MachineCleanupTaskSpawner,
        session_id: &SessionId,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        live_boundary_plan: Option<RuntimeAcceptedLiveBoundaryPlan>,
        cancel_plan: Option<RuntimeAcceptedBoundaryCancelPlan>,
        completions: crate::meerkat_machine::driver::SharedCompletionRegistry,
        wake_tx: Option<mpsc::Sender<()>>,
        should_wake: bool,
        fallback_wake: AcceptedIngressFallbackWakeGuard,
        runless_terminal: RunlessTerminalAnnouncement,
    ) {
        let machine = self.clone();
        let session_id = session_id.clone();
        let work_session_id = session_id.clone();
        let work = cleanup_spawner.spawn(async move {
            #[cfg(test)]
            {
                let hold = machine
                    .test_accepted_boundary_work_hold
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some((entered, release)) = hold {
                    let _ = entered.send(());
                    let _ = release.await;
                }
            }
            #[cfg(test)]
            let outcome_input_id = live_boundary_plan
                .as_ref()
                .map(|plan| plan.input_id.clone());
            let dispatched = machine
                .dispatch_accepted_ingress_boundary_work_owned(
                    &work_session_id,
                    held_mutation_gate,
                    live_boundary_plan,
                    cancel_plan,
                    completions,
                    wake_tx,
                    should_wake,
                    fallback_wake,
                )
                .await;
            drop(runless_terminal);
            #[cfg(test)]
            if let Some(outcomes) = machine
                .test_accepted_boundary_work_outcomes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                let _ = outcomes.send((
                    outcome_input_id,
                    dispatched.as_ref().copied().map_err(ToString::to_string),
                ));
            }
            if let Err(error) = dispatched {
                tracing::error!(
                    session_id = %work_session_id,
                    %error,
                    "asynchronous accepted-input boundary optimization failed"
                );
            }
        });
        cleanup_spawner.spawn(async move {
            if let Err(error) = work.await {
                tracing::error!(
                    %session_id,
                    %error,
                    "asynchronous accepted-input boundary task panicked or was cancelled"
                );
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_accepted_ingress_boundary_work_owned(
        &self,
        session_id: &SessionId,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        live_boundary_plan: Option<RuntimeAcceptedLiveBoundaryPlan>,
        cancel_plan: Option<RuntimeAcceptedBoundaryCancelPlan>,
        completions: crate::meerkat_machine::driver::SharedCompletionRegistry,
        wake_tx: Option<mpsc::Sender<()>>,
        should_wake: bool,
        mut fallback_wake: AcceptedIngressFallbackWakeGuard,
    ) -> Result<(), RuntimeDriverError> {
        let mut gate_guard = held_mutation_gate;
        let live_boundary_disposition = if let Some(live_boundary_plan) = live_boundary_plan {
            let (returned_gate, disposition) = self
                .commit_live_boundary_input_if_available(
                    session_id,
                    &live_boundary_plan.witness,
                    gate_guard,
                    &live_boundary_plan.input_id,
                    &completions,
                    live_boundary_plan.publication_handle,
                    &mut fallback_wake,
                )
                .await?;
            gate_guard = returned_gate;
            disposition
        } else {
            dispatch_ingress::LiveBoundaryInputDisposition::QueuedFallback
        };

        let cancel_plan = if live_boundary_disposition.suppress_cancel() {
            None
        } else {
            cancel_plan
        };
        let should_wake = should_wake && !live_boundary_disposition.suppress_wake();
        if let Some(cancel_plan) = cancel_plan {
            gate_guard = self
                .dispatch_cancel_after_boundary_runtime_effect_owned(
                    session_id,
                    cancel_plan.witness,
                    gate_guard,
                    cancel_plan.boundary_handle,
                    None,
                    cancel_plan.pending_dispatch,
                    Some(&cancel_plan.expected_run_id),
                    cancel_plan.projected_effect,
                    cancel_plan.dispatch_generation,
                    cancel_plan.dispatch_lifecycle_phase,
                    false,
                    "AcceptWithCompletion",
                )
                .await?;
        }
        if should_wake && let Some(wake_tx) = wake_tx {
            let _ = wake_tx.try_send(());
        }
        fallback_wake.disarm();
        drop(gate_guard);
        Ok(())
    }

    /// Invoke the executor-owned boundary hook without holding a session
    /// mutation mutex. Embedders may route this hook back through
    /// `MeerkatMachine::cancel_after_boundary`; the already-pending machine
    /// fact bounds that re-entry, but only if the nested call can acquire the
    /// gate and observe it.
    async fn dispatch_cancel_after_boundary_live_handle(
        &self,
        boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
        expected_run_id: Option<&RunId>,
        projected_effect: &crate::effect::ProjectedRuntimeEffect,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        let reason = projected_effect.reason().to_string();
        // The cloneable live handle is exact-run authority. Attached runtimes
        // legitimately have no run yet; their generated CancelAfterBoundary
        // dispatch still goes through the in-loop executor effect below, but
        // must not fabricate an ID for this live callback.
        if let (Some(boundary_handle), Some(expected_run_id)) = (boundary_handle, expected_run_id) {
            boundary_handle
                .cancel_after_boundary(expected_run_id, reason)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "{context}: failed to apply live boundary cancel: {err}"
                    ))
                })?;
        }
        Ok(())
    }

    async fn restore_session_dsl_state(
        &self,
        session_id: &SessionId,
        snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    ) {
        if let Ok(authority) = self.session_dsl_authority(session_id).await {
            Self::restore_dsl_authority_snapshot(&authority, snapshot);
        }
    }

    async fn restore_session_dsl_state_if_current(
        &self,
        session_id: &SessionId,
        expected_current: dsl::MeerkatMachineAuthoritySnapshot,
        restore: dsl::MeerkatMachineAuthoritySnapshot,
    ) -> bool {
        let Ok(authority) = self.session_dsl_authority(session_id).await else {
            return false;
        };
        Self::restore_dsl_authority_snapshot_if_current(&authority, expected_current, restore)
    }

    fn classify_dsl_boundary_cancel_dispatch(
        authority: &Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        dispatch_generation: u64,
        dispatch_lifecycle_phase: &dsl::MeerkatPhase,
        expected_run_id: Option<&RunId>,
    ) -> BoundaryCancelDispatchState {
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        if state.boundary_cancel_dispatch_generation != dispatch_generation {
            return BoundaryCancelDispatchState::Superseded;
        }
        if !state.boundary_cancel_dispatch_pending {
            return BoundaryCancelDispatchState::ConsumedConverged;
        }
        let expected_dsl_run_id = expected_run_id.map(dsl::RunId::from_domain);
        if state.lifecycle_phase == *dispatch_lifecycle_phase
            && state.current_run_id.as_ref() == expected_dsl_run_id.as_ref()
        {
            BoundaryCancelDispatchState::Current
        } else {
            BoundaryCancelDispatchState::SameGenerationOrphan
        }
    }

    /// Clear only the failed boundary-dispatch fact represented by
    /// `dispatch_generation`. This deliberately applies a generated input to
    /// the current authority instead of restoring a whole pre-callback
    /// snapshot: the live executor hook may have re-entered the machine and
    /// committed unrelated state while the session mutation gate was
    /// released.
    fn abort_dsl_boundary_cancel_dispatch_if_current(
        authority: &Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        dispatch_generation: u64,
    ) -> Result<bool, RuntimeDriverError> {
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !authority.state().boundary_cancel_dispatch_pending
            || authority.state().boundary_cancel_dispatch_generation != dispatch_generation
        {
            return Ok(false);
        }
        dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            dsl::MeerkatMachineInput::AbortCancelAfterBoundaryDispatch {
                dispatch_generation,
            },
        )
        .map(|_| true)
        .map_err(|err| {
            RuntimeDriverError::Internal(dsl_authority::map_error(
                err,
                "AbortCancelAfterBoundaryDispatch",
            ))
        })
    }

    fn restore_dsl_authority_snapshot(
        authority: &Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    ) {
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authority.restore_snapshot(snapshot);
    }

    fn restore_dsl_authority_snapshot_if_current(
        authority: &Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        expected_current: dsl::MeerkatMachineAuthoritySnapshot,
        restore: dsl::MeerkatMachineAuthoritySnapshot,
    ) -> bool {
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = authority.snapshot();
        if current.state() == expected_current.state() {
            authority.restore_snapshot(restore);
            true
        } else {
            false
        }
    }
}

/// Capability token proving a session-control mutation is routed through
/// `MeerkatMachine` authority instead of a public store-only service path.
#[derive(Debug, Clone, Copy)]
pub struct MachineSessionControlAuthority {
    _private: (),
}

#[cfg(feature = "live")]
struct LiveOpenAdmissionGeneratedAuthorityBridgeToken;

#[cfg(feature = "live")]
struct LiveCloseResultGeneratedAuthorityBridgeToken;

#[cfg(feature = "live")]
struct LiveChannelStatusResultGeneratedAuthorityBridgeToken;

#[cfg(feature = "live")]
static LIVE_OPEN_ADMISSION_GENERATED_AUTHORITY_BRIDGE_TOKEN:
    LiveOpenAdmissionGeneratedAuthorityBridgeToken = LiveOpenAdmissionGeneratedAuthorityBridgeToken;

#[cfg(feature = "live")]
static LIVE_CLOSE_RESULT_GENERATED_AUTHORITY_BRIDGE_TOKEN:
    LiveCloseResultGeneratedAuthorityBridgeToken = LiveCloseResultGeneratedAuthorityBridgeToken;

#[cfg(feature = "live")]
static LIVE_CHANNEL_STATUS_RESULT_GENERATED_AUTHORITY_BRIDGE_TOKEN:
    LiveChannelStatusResultGeneratedAuthorityBridgeToken =
    LiveChannelStatusResultGeneratedAuthorityBridgeToken;

#[cfg(feature = "live")]
fn live_open_admission_generated_authority_bridge_token()
-> &'static (dyn std::any::Any + Send + Sync) {
    &LIVE_OPEN_ADMISSION_GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[cfg(feature = "live")]
fn live_close_result_generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync)
{
    &LIVE_CLOSE_RESULT_GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[cfg(feature = "live")]
fn live_channel_status_result_generated_authority_bridge_token()
-> &'static (dyn std::any::Any + Send + Sync) {
    &LIVE_CHANNEL_STATUS_RESULT_GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[cfg(feature = "live")]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_open_admission_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn live_open_admission_generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<LiveOpenAdmissionGeneratedAuthorityBridgeToken>()
}

#[cfg(feature = "live")]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_close_result_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn live_close_result_generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<LiveCloseResultGeneratedAuthorityBridgeToken>()
}

#[cfg(feature = "live")]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_channel_status_result_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn live_channel_status_result_generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<LiveChannelStatusResultGeneratedAuthorityBridgeToken>()
}

#[cfg(feature = "live")]
fn build_live_channel_open_authority(
    session_id: SessionId,
    channel_id: meerkat_live::LiveChannelId,
    sequence: u64,
) -> Result<meerkat_live::LiveChannelOpenAuthority, String> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_live_runtime_generated_live_channel_open_authority_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn live_generated_channel_open_authority_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            session_id: SessionId,
            channel_id: meerkat_live::LiveChannelId,
            sequence: u64,
        ) -> Result<meerkat_live::LiveChannelOpenAuthority, String>;
    }
    #[allow(unsafe_code)]
    unsafe {
        live_generated_channel_open_authority_build(
            live_open_admission_generated_authority_bridge_token(),
            session_id,
            channel_id,
            sequence,
        )
    }
}

#[cfg(feature = "live")]
fn build_live_channel_close_commit_authority(
    channel_id: String,
    close_sequence: u64,
) -> Result<meerkat_live::LiveChannelCloseCommitAuthority, String> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_live_runtime_generated_live_channel_close_commit_authority_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn live_generated_channel_close_commit_authority_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            channel_id: String,
            close_sequence: u64,
        ) -> Result<meerkat_live::LiveChannelCloseCommitAuthority, String>;
    }
    #[allow(unsafe_code)]
    unsafe {
        live_generated_channel_close_commit_authority_build(
            live_close_result_generated_authority_bridge_token(),
            channel_id,
            close_sequence,
        )
    }
}

#[cfg(feature = "live")]
fn build_live_channel_status_commit_authority(
    channel_id: String,
    status_observation_sequence: u64,
) -> Result<meerkat_live::LiveChannelStatusCommitAuthority, String> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_live_runtime_generated_live_channel_status_commit_authority_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn live_generated_channel_status_commit_authority_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            channel_id: String,
            status_observation_sequence: u64,
        ) -> Result<meerkat_live::LiveChannelStatusCommitAuthority, String>;
    }
    #[allow(unsafe_code)]
    unsafe {
        live_generated_channel_status_commit_authority_build(
            live_channel_status_result_generated_authority_bridge_token(),
            channel_id,
            status_observation_sequence,
        )
    }
}

/// Generated authority output for `live/open` admission.
///
/// Constructed only from `MeerkatMachineEffect::LiveOpenAdmissionResolved`.
/// The live host accepts this as a typed handoff before materializing
/// transport resources; it does not decide duplicate-session admission from
/// its local maps.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveOpenAdmissionAuthority {
    session_id: SessionId,
    channel_id: meerkat_live::LiveChannelId,
    admitted: bool,
    rejection: Option<dsl::LiveOpenAdmissionRejection>,
    bound_llm_identity: Option<meerkat_core::SessionLlmIdentity>,
    sequence: u64,
    channel_open_authority: Option<meerkat_live::LiveChannelOpenAuthority>,
}

#[cfg(feature = "live")]
impl LiveOpenAdmissionAuthority {
    pub(crate) fn from_generated_effect(
        session_id: SessionId,
        channel_id: meerkat_live::LiveChannelId,
        admitted: bool,
        rejection: Option<dsl::LiveOpenAdmissionRejection>,
        bound_llm_identity: Option<dsl::SessionLlmIdentity>,
        sequence: u64,
    ) -> Result<Self, String> {
        let bound_llm_identity = match (admitted, bound_llm_identity) {
            (true, Some(identity)) => Some(identity.try_into()?),
            (true, None) => {
                return Err(
                    "generated live-open admission was admitted without bound LLM identity"
                        .to_string(),
                );
            }
            (false, _) => None,
        };
        let channel_open_authority = if admitted {
            Some(build_live_channel_open_authority(
                session_id.clone(),
                channel_id.clone(),
                sequence,
            )?)
        } else {
            None
        };
        Ok(Self {
            session_id,
            channel_id,
            admitted,
            rejection,
            bound_llm_identity,
            sequence,
            channel_open_authority,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &meerkat_live::LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn admitted(&self) -> bool {
        self.admitted
    }

    #[must_use]
    pub fn rejection(&self) -> Option<dsl::LiveOpenAdmissionRejection> {
        self.rejection
    }

    #[must_use]
    pub fn bound_llm_identity(&self) -> Option<&meerkat_core::SessionLlmIdentity> {
        self.bound_llm_identity.as_ref()
    }

    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub fn channel_open_authority(&self) -> Option<&meerkat_live::LiveChannelOpenAuthority> {
        self.channel_open_authority.as_ref()
    }
}

/// Generated authority output for the public `live/refresh` success result.
///
/// Constructed only from a `MeerkatMachineEffect::LiveRefreshResultResolved`
/// emitted after the live adapter command queue has accepted the refresh
/// handoff. RPC/SDK surfaces project this value to their wire result instead
/// of classifying the public status from host queue mechanics.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveRefreshResultAuthority {
    pub status: dsl::LiveRefreshPublicStatus,
    pub sequence: u64,
    pub queue_acceptance_sequence: u64,
}

/// Generated authority output for the public `live/close` success result.
///
/// Constructed only from a `MeerkatMachineEffect::LiveCloseResultResolved`
/// emitted after the live host supplies typed close-observation evidence.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveCloseResultAuthority {
    pub status: dsl::LiveClosePublicStatus,
    pub sequence: u64,
    pub close_observation_sequence: u64,
    channel_close_commit_authority: Option<meerkat_live::LiveChannelCloseCommitAuthority>,
}

#[cfg(feature = "live")]
impl LiveCloseResultAuthority {
    pub(crate) fn from_generated_effect(
        channel_id: String,
        status: dsl::LiveClosePublicStatus,
        sequence: u64,
        close_observation_sequence: u64,
    ) -> Result<Self, String> {
        let channel_close_commit_authority = match status {
            dsl::LiveClosePublicStatus::Closed => Some(build_live_channel_close_commit_authority(
                channel_id,
                close_observation_sequence,
            )?),
        };
        Ok(Self {
            status,
            sequence,
            close_observation_sequence,
            channel_close_commit_authority,
        })
    }

    #[must_use]
    pub fn channel_close_commit_authority(
        &self,
    ) -> Option<&meerkat_live::LiveChannelCloseCommitAuthority> {
        self.channel_close_commit_authority.as_ref()
    }

    #[must_use]
    pub fn into_channel_close_commit_authority(
        self,
    ) -> Option<meerkat_live::LiveChannelCloseCommitAuthority> {
        self.channel_close_commit_authority
    }
}

/// Generated authority output for public live command success results.
///
/// Constructed only from a `MeerkatMachineEffect::LiveCommandResultResolved`
/// emitted after the live host supplies typed command queue-acceptance
/// evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveCommandResultAuthority {
    pub command: dsl::LiveCommandPublicKind,
    pub sequence: u64,
    pub command_acceptance_sequence: u64,
}

/// Generated authority output for public live command rejection results.
///
/// Constructed only from a `MeerkatMachineEffect::LiveCommandRejectionResolved`
/// emitted after the live host supplies typed rejection evidence. RPC/SDK
/// surfaces project error classes from this value instead of matching host
/// errors directly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveCommandRejectionAuthority {
    pub command: dsl::LiveCommandPublicKind,
    pub rejection: dsl::LiveCommandRejectionReason,
    pub public_error_class: dsl::LiveCommandRejectionPublicErrorClass,
    pub sequence: u64,
}

/// Generated authority output for public live channel control request
/// rejections.
///
/// Constructed only from a
/// `MeerkatMachineEffect::LiveChannelRequestRejectionResolved` emitted after
/// the live host supplies typed rejection evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveChannelRequestRejectionAuthority {
    pub request: dsl::LiveChannelRequestPublicKind,
    pub rejection: dsl::LiveChannelRequestRejectionReason,
    pub public_error_class: dsl::LiveChannelRequestRejectionPublicErrorClass,
    pub sequence: u64,
}

/// Generated authority output for a WebRTC answer token issued by
/// MeerkatMachine.
///
/// Constructed only from `MeerkatMachineEffect::LiveWebrtcTokenIssued`.
/// The transport supplies random bearer material, but it is not returned to a
/// caller until the generated machine records the channel binding and expiry.
///
/// `Debug` redacts the bearer token.
#[derive(Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveWebrtcTokenAuthority {
    pub token: String,
    pub expires_at_ms: u64,
    pub sequence: u64,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveWebrtcTokenAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveWebrtcTokenAuthority")
            .field("token", &meerkat_core::redact::REDACTED)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("sequence", &self.sequence)
            .finish()
    }
}

/// Generated authority output for WebRTC answer token admission.
///
/// Constructed only from
/// `MeerkatMachineEffect::LiveWebrtcAnswerAdmissionResolved`. RPC signaling
/// proceeds to peer setup only when this effect admits the token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveWebrtcAnswerAdmissionAuthority {
    pub admitted: bool,
    pub rejection: Option<dsl::LiveWebrtcAnswerAdmissionRejection>,
    pub public_error_class: Option<dsl::LiveChannelRequestRejectionPublicErrorClass>,
    pub sequence: u64,
    /// Opaque one-use transport seal present only for an admitted generated
    /// effect. RPC consumes it while constructing the provider answer offer.
    pub transport_seal: Option<meerkat_live::LiveWebrtcAnswerAdmissionSeal>,
}

/// Generated authority output for the public `live/webrtc/answer` success
/// class.
///
/// Constructed only from
/// `MeerkatMachineEffect::LiveWebrtcAnswerResultResolved` emitted after the
/// WebRTC transport supplies answer-observation evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveWebrtcAnswerResultAuthority {
    pub status: dsl::LiveWebrtcAnswerPublicStatus,
    pub answered: bool,
    pub sequence: u64,
    pub answer_observation_sequence: u64,
}

/// Generated admission of one exact provider foreground turn to one Meerkat
/// interaction. Constructed only after typed sideband TurnStarted evidence is
/// fenced to the active execution binding.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveProviderTurnStartedAuthority {
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
    interaction_id: meerkat_core::InteractionId,
    provider_turn_ref: String,
}

#[cfg(feature = "live")]
impl LiveProviderTurnStartedAuthority {
    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub const fn interaction_id(&self) -> meerkat_core::InteractionId {
        self.interaction_id
    }

    #[must_use]
    pub fn provider_turn_ref(&self) -> &str {
        &self.provider_turn_ref
    }
}

/// Opaque one-use address for one exact assistant playback segment.
///
/// The initial handle follows generated `LiveAssistantTurnStarted` authority;
/// successors require a session-sealed snapshot cut and generated segment
/// advancement within that same frozen interaction. Clones share reservation and terminal
/// state, so pre-acceptance failures can release exact custody while stale,
/// cross-channel, concurrent, or replayed commands still fail closed.
#[derive(Clone)]
#[cfg(feature = "live")]
pub struct LiveAssistantOutputHandle {
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
    interaction_id: meerkat_core::InteractionId,
    origin: dsl::LiveAssistantTurnOrigin,
    assistant_turn_ref: String,
    playback_segment: u64,
    output_id: String,
    target: Arc<StdMutex<Option<(String, String, u32)>>>,
    terminal_reserved: Arc<std::sync::atomic::AtomicBool>,
    terminal_consumed: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveAssistantOutputHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAssistantOutputHandle")
            .field("binding", &self.binding)
            .field("interaction_id", &self.interaction_id)
            .field("origin", &self.origin)
            .field("assistant_turn_ref", &"<opaque>")
            .field("output_id", &self.output_id)
            .field(
                "target_bound",
                &self
                    .target
                    .lock()
                    .map(|target| target.is_some())
                    .unwrap_or(false),
            )
            .field(
                "terminal_reserved",
                &self
                    .terminal_reserved
                    .load(std::sync::atomic::Ordering::Acquire),
            )
            .field(
                "terminal_consumed",
                &self
                    .terminal_consumed
                    .load(std::sync::atomic::Ordering::Acquire),
            )
            .finish()
    }
}

#[cfg(feature = "live")]
impl LiveAssistantOutputHandle {
    #[doc(hidden)]
    pub fn __playback_segment(&self) -> u64 {
        self.playback_segment
    }

    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub const fn interaction_id(&self) -> meerkat_core::InteractionId {
        self.interaction_id
    }

    #[must_use]
    pub fn output_id(&self) -> &str {
        &self.output_id
    }

    /// Machine-owned attribution; provider-initiated output does not prove
    /// consumption of any particular context append.
    #[must_use]
    pub const fn origin(&self) -> dsl::LiveAssistantTurnOrigin {
        self.origin
    }

    #[doc(hidden)]
    pub fn __bind_target(
        &self,
        response_id: &str,
        item_id: &str,
        content_index: u32,
    ) -> Result<(), crate::live_execution::LiveExecutionAuthorityError> {
        if response_id.trim().is_empty() || item_id.trim().is_empty() {
            return Err(
                crate::live_execution::LiveExecutionAuthorityError::AssistantOutputMismatch,
            );
        }
        let mut target = self
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let candidate = (response_id.to_string(), item_id.to_string(), content_index);
        match target.as_ref() {
            None => *target = Some(candidate),
            Some(existing) if existing == &candidate => {}
            Some(_) => {
                return Err(
                    crate::live_execution::LiveExecutionAuthorityError::AssistantOutputMismatch,
                );
            }
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn __target(&self) -> Option<(String, String, u32)> {
        self.target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __assistant_turn_ref(&self) -> &str {
        &self.assistant_turn_ref
    }

    /// Reserve terminal authority only for the exact channel-scoped output.
    ///
    /// Reservation is deliberately distinct from terminal consumption: a
    /// transport rejection before queue acceptance releases the reservation,
    /// leaving the same exact handle eligible for a later terminal attempt.
    #[doc(hidden)]
    pub(crate) fn __reserve_for_playback_terminal(
        &self,
        binding: &crate::live_execution::LiveDelegationRuntimeBinding,
        interaction_id: meerkat_core::InteractionId,
        assistant_turn_ref: &str,
    ) -> Result<(), crate::live_execution::LiveExecutionAuthorityError> {
        if self.binding != *binding
            || self.interaction_id != interaction_id
            || self.assistant_turn_ref != assistant_turn_ref
        {
            return Err(
                crate::live_execution::LiveExecutionAuthorityError::AssistantOutputMismatch,
            );
        }
        if self
            .terminal_consumed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(
                crate::live_execution::LiveExecutionAuthorityError::AssistantOutputAlreadyConsumed,
            );
        }
        self.terminal_reserved
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map_err(|_| {
                crate::live_execution::LiveExecutionAuthorityError::AssistantOutputAlreadyReserved
            })?;
        if self
            .terminal_consumed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.terminal_reserved
                .store(false, std::sync::atomic::Ordering::Release);
            return Err(
                crate::live_execution::LiveExecutionAuthorityError::AssistantOutputAlreadyConsumed,
            );
        }
        Ok(())
    }

    fn commit_playback_terminal(
        &self,
    ) -> Result<(), crate::live_execution::LiveExecutionAuthorityError> {
        if !self
            .terminal_reserved
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(
                crate::live_execution::LiveExecutionAuthorityError::AssistantOutputMismatch,
            );
        }
        self.terminal_consumed
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map_err(|_| {
                crate::live_execution::LiveExecutionAuthorityError::AssistantOutputAlreadyConsumed
            })?;
        self.terminal_reserved
            .store(false, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    fn release_playback_terminal(&self) {
        if !self
            .terminal_consumed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.terminal_reserved
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }
}

/// Two-phase custody of one assistant-output terminal dispatch.
///
/// Dropping an uncommitted reservation releases it. The output address is
/// removed and becomes permanently terminal only after the caller proves the
/// corresponding command or SessionDocument terminal was accepted.
#[cfg(feature = "live")]
pub struct LiveAssistantOutputTerminalReservation {
    handle: LiveAssistantOutputHandle,
    finalized: bool,
    _lifecycle_lease: crate::member_live::MemberLiveLifecycleLease,
}

/// Keeps one closed channel's generated assistant-output handles addressable
/// while the close's deferred settlement replays rows that need them: an
/// output start that the member turn's held session boundary parked until
/// after the close still finds its generated handle.
///
/// A retirement requested while a hold exists
/// ([`MeerkatMachine::retire_live_assistant_output_handles`]) runs when the
/// last hold ends. A held handle never becomes reservable: reservation still
/// requires the channel's current generated binding, which the close removed.
#[cfg(feature = "live")]
#[must_use = "dropping the hold ends it at once"]
pub struct LiveAssistantOutputRetirementHold {
    machine: MeerkatMachine,
    session_id: SessionId,
    channel_id: meerkat_core::LiveChannelId,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveAssistantOutputRetirementHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAssistantOutputRetirementHold")
            .field("session_id", &self.session_id)
            .field("channel_id", &self.channel_id)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "live")]
impl Drop for LiveAssistantOutputRetirementHold {
    fn drop(&mut self) {
        self.machine
            .end_live_assistant_output_retirement_hold(&self.session_id, &self.channel_id);
    }
}

/// The holds on one channel's output handles, and whether a retirement waits
/// for the last of them to end.
#[cfg(feature = "live")]
#[derive(Debug, Default)]
struct LiveAssistantOutputRetirementHolds {
    holds: usize,
    retirement_requested: bool,
}

/// Opaque custody proving one exact provider binding is still current while
/// its public observation is written and flushed.
///
/// The contained session lifecycle lease prevents a close/replacement from
/// crossing the writer settlement boundary. Surfaces cannot reconstruct this
/// proof from copied channel, generation, or fence values.
#[cfg(feature = "live")]
pub struct LiveBindingPublicationCustody {
    binding: meerkat_live::ProviderWebrtcBinding,
    _lifecycle_lease: crate::member_live::MemberLiveLifecycleLease,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveBindingPublicationCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveBindingPublicationCustody")
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

/// Total result of acquiring exact live observation publication custody.
#[derive(Debug)]
#[cfg(feature = "live")]
pub enum LiveBindingPublicationAdmission {
    Current(LiveBindingPublicationCustody),
    Stale,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveAssistantOutputTerminalReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAssistantOutputTerminalReservation")
            .field("handle", &self.handle)
            .field("finalized", &self.finalized)
            .finish()
    }
}

#[cfg(feature = "live")]
impl LiveAssistantOutputTerminalReservation {
    #[must_use]
    pub fn handle(&self) -> &LiveAssistantOutputHandle {
        &self.handle
    }

    /// Explicitly release terminal custody after a pre-acceptance failure.
    pub fn release(mut self) {
        self.handle.release_playback_terminal();
        self.finalized = true;
    }

    fn commit(
        mut self,
    ) -> Result<LiveAssistantOutputHandle, crate::live_execution::LiveExecutionAuthorityError> {
        self.handle.commit_playback_terminal()?;
        self.finalized = true;
        Ok(self.handle.clone())
    }
}

#[cfg(feature = "live")]
impl Drop for LiveAssistantOutputTerminalReservation {
    fn drop(&mut self) {
        if !self.finalized {
            self.handle.release_playback_terminal();
        }
    }
}

/// Generated proof that the exact typed provider turn which opened an
/// interaction has finished. This is the safe-boundary trigger for draining
/// deferred canonical context.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveProviderTurnFinishedAuthority {
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
    interaction_id: meerkat_core::InteractionId,
    provider_turn_ref: String,
}

/// Generated pending custody for one strict experimental live execution.
/// The marker binds pre-answer context rows to exact session/channel/runtime
/// identity and the canonical seed cursor that the provider must acknowledge.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct ExperimentalLiveExecutionStageAuthority {
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
    canonical_seed_cursor: u64,
    pending_receipt: String,
}

#[cfg(feature = "live")]
impl ExperimentalLiveExecutionStageAuthority {
    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub const fn canonical_seed_cursor(&self) -> u64 {
        self.canonical_seed_cursor
    }

    #[must_use]
    pub fn pending_receipt(&self) -> &str {
        &self.pending_receipt
    }

    #[must_use]
    pub const fn phase(&self) -> meerkat_core::LiveExecutionChannelPhase {
        meerkat_core::LiveExecutionChannelPhase::Pending
    }
}

/// Sealed proof that the sole playback owner is ready while the channel is
/// still pending. Answer acceptance cannot activate without this machine fact.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LivePlaybackOwnerReadinessAuthority {
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
    pending_receipt: String,
    owner_id: String,
    readiness_id: String,
}

#[cfg(feature = "live")]
impl LivePlaybackOwnerReadinessAuthority {
    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub fn pending_receipt(&self) -> &str {
        &self.pending_receipt
    }

    #[must_use]
    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    #[must_use]
    pub fn readiness_id(&self) -> &str {
        &self.readiness_id
    }
}

#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveChannelActivationReceipt {
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
    activation_receipt: String,
}

#[cfg(feature = "live")]
impl LiveChannelActivationReceipt {
    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub fn activation_receipt(&self) -> &str {
        &self.activation_receipt
    }

    #[must_use]
    pub const fn phase(&self) -> meerkat_core::LiveExecutionChannelPhase {
        meerkat_core::LiveExecutionChannelPhase::Active
    }
}

/// Sealed read-only projection for one opaque pending receipt. Active custody
/// carries the exact machine-minted activation receipt; callers never infer or
/// manufacture phase transitions.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub enum LiveChannelCustodyState {
    Pending(ExperimentalLiveExecutionStageAuthority),
    Active(LiveChannelActivationReceipt),
    Revoked,
    Closed,
}

#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveChannelCustodyProjection {
    session_id: SessionId,
    channel_id: meerkat_core::LiveChannelId,
    mode: meerkat_core::LiveExecutionMode,
    state: LiveChannelCustodyState,
    preparation: crate::live_execution::LiveContextPreparationStatus,
}

#[cfg(feature = "live")]
impl LiveChannelCustodyProjection {
    #[must_use]
    pub const fn phase(&self) -> Option<meerkat_core::LiveExecutionChannelPhase> {
        match &self.state {
            LiveChannelCustodyState::Pending(_) => {
                Some(meerkat_core::LiveExecutionChannelPhase::Pending)
            }
            LiveChannelCustodyState::Active(_) => {
                Some(meerkat_core::LiveExecutionChannelPhase::Active)
            }
            LiveChannelCustodyState::Revoked => {
                Some(meerkat_core::LiveExecutionChannelPhase::Revoked)
            }
            LiveChannelCustodyState::Closed => None,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &meerkat_core::LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub const fn mode(&self) -> meerkat_core::LiveExecutionMode {
        self.mode
    }

    #[must_use]
    pub const fn state(&self) -> &LiveChannelCustodyState {
        &self.state
    }

    #[must_use]
    pub const fn preparation(&self) -> crate::live_execution::LiveContextPreparationStatus {
        self.preparation
    }

    #[must_use]
    pub const fn context_preparation(
        &self,
    ) -> &crate::live_execution::LiveContextPreparationStatus {
        &self.preparation
    }
}

/// Caller-held machine receipt accepted for strict close. The enum provides
/// no authority by itself; the generated state validates the exact value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub enum LiveChannelCloseReceipt {
    Pending(String),
    Activation(String),
}

#[derive(Debug)]
#[cfg(feature = "live")]
pub struct LiveChannelCloseCustodyAuthority {
    session_id: SessionId,
    channel_id: meerkat_core::LiveChannelId,
    already_closed: bool,
    context_recovery_channel_id: Option<meerkat_core::LiveChannelId>,
    result_recovery_channel_id: Option<meerkat_core::LiveChannelId>,
}

#[cfg(feature = "live")]
impl LiveChannelCloseCustodyAuthority {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &meerkat_core::LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub const fn already_closed(&self) -> bool {
        self.already_closed
    }

    /// Exact cancelled recovery candidates, never a later unrelated open.
    pub fn recovery_channel_ids(&self) -> impl Iterator<Item = &meerkat_core::LiveChannelId> {
        self.context_recovery_channel_id
            .iter()
            .chain(self.result_recovery_channel_id.iter())
    }
}

#[derive(Debug)]
#[cfg(feature = "live")]
pub struct LiveActiveChannelControlAuthority {
    activation: LiveChannelActivationReceipt,
    control_authority_id: String,
    operation: String,
}

#[cfg(feature = "live")]
impl LiveActiveChannelControlAuthority {
    #[must_use]
    pub fn activation(&self) -> &LiveChannelActivationReceipt {
        &self.activation
    }

    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }
}

#[derive(Debug)]
#[cfg(feature = "live")]
pub struct LiveActiveChannelControlDispatchAuthority(LiveActiveChannelControlAuthority);

#[cfg(feature = "live")]
impl LiveActiveChannelControlDispatchAuthority {
    #[must_use]
    pub fn control(&self) -> &LiveActiveChannelControlAuthority {
        &self.0
    }
}

#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveChannelRevocationReceipt {
    session_id: SessionId,
    channel_id: meerkat_core::LiveChannelId,
    owner_id: String,
}

#[cfg(feature = "live")]
impl LiveChannelRevocationReceipt {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &meerkat_core::LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    #[must_use]
    pub const fn phase(&self) -> meerkat_core::LiveExecutionChannelPhase {
        meerkat_core::LiveExecutionChannelPhase::Revoked
    }
}

#[cfg(feature = "live")]
impl LiveProviderTurnFinishedAuthority {
    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub const fn interaction_id(&self) -> meerkat_core::InteractionId {
        self.interaction_id
    }

    #[must_use]
    pub fn provider_turn_ref(&self) -> &str {
        &self.provider_turn_ref
    }
}

/// Atomic generated authority for an answered WebRTC transport whose exact
/// runtime binding and acknowledged canonical seed cursor committed in the
/// same MeerkatMachine transition.
#[cfg(feature = "live")]
pub struct LiveWebrtcAnswerExecutionBindingAuthority {
    answer: LiveWebrtcAnswerResultAuthority,
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
    rollback: LiveWebrtcAnswerExecutionRollbackAuthority,
    activation: Option<LiveChannelActivationReceipt>,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveWebrtcAnswerExecutionBindingAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveWebrtcAnswerExecutionBindingAuthority")
            .field("answer", &self.answer)
            .field("binding", &"[REDACTED]")
            .field("rollback", &"[SEALED]")
            .finish()
    }
}

#[cfg(feature = "live")]
impl LiveWebrtcAnswerExecutionBindingAuthority {
    pub(crate) fn new_active(
        answer: LiveWebrtcAnswerResultAuthority,
        binding: crate::live_execution::LiveDelegationRuntimeBinding,
        activation_receipt: String,
    ) -> Self {
        let activation = LiveChannelActivationReceipt {
            binding: binding.clone(),
            activation_receipt,
        };
        Self {
            rollback: LiveWebrtcAnswerExecutionRollbackAuthority {
                binding: binding.clone(),
            },
            answer,
            binding,
            activation: Some(activation),
        }
    }

    #[must_use]
    pub fn answer(&self) -> &LiveWebrtcAnswerResultAuthority {
        &self.answer
    }

    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub fn activation_receipt(&self) -> Option<&LiveChannelActivationReceipt> {
        self.activation.as_ref()
    }

    /// Commit publication custody and relinquish the rollback capability.
    #[must_use]
    pub fn commit(
        self,
    ) -> (
        LiveWebrtcAnswerResultAuthority,
        crate::live_execution::LiveDelegationRuntimeBinding,
    ) {
        (self.answer, self.binding)
    }

    /// Retain the exact one-use capability required to close generated state
    /// if answer publication is rejected or the publication custody is dropped.
    #[must_use]
    pub fn into_rollback(self) -> LiveWebrtcAnswerExecutionRollbackAuthority {
        self.rollback
    }
}

/// One-use exact authority to close the channel bound by an atomic accepted
/// WebRTC answer. It is intentionally non-Clone.
#[cfg(feature = "live")]
pub struct LiveWebrtcAnswerExecutionRollbackAuthority {
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveWebrtcAnswerExecutionRollbackAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveWebrtcAnswerExecutionRollbackAuthority")
            .field("binding", &"[REDACTED]")
            .finish()
    }
}

#[cfg(feature = "live")]
impl LiveWebrtcAnswerExecutionRollbackAuthority {
    #[must_use]
    pub fn binding(&self) -> &crate::live_execution::LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub fn authorizes(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> bool {
        self.binding.session_id() == session_id && self.binding.channel_id() == channel_id
    }
}

/// Generated authority output for a WebSocket transport token issued by
/// MeerkatMachine.
///
/// Constructed only from `MeerkatMachineEffect::LiveWebsocketTokenIssued`.
/// The WebSocket transport supplies random bearer material, but it is not
/// returned until generated authority records channel binding and expiry.
///
/// `Debug` redacts the bearer token.
#[derive(Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveWebsocketTokenAuthority {
    pub token: String,
    pub expires_at_ms: u64,
    pub sequence: u64,
}

#[cfg(feature = "live")]
impl std::fmt::Debug for LiveWebsocketTokenAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveWebsocketTokenAuthority")
            .field("token", &meerkat_core::redact::REDACTED)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("sequence", &self.sequence)
            .finish()
    }
}

#[cfg(all(test, feature = "live"))]
mod live_token_authority_debug_tests {
    use super::{LiveWebrtcTokenAuthority, LiveWebsocketTokenAuthority};

    #[test]
    fn live_token_authorities_debug_redact_token() {
        const SECRET: &str = "live-bootstrap-secret";
        let webrtc = LiveWebrtcTokenAuthority {
            token: SECRET.into(),
            expires_at_ms: 10,
            sequence: 1,
        };
        let websocket = LiveWebsocketTokenAuthority {
            token: SECRET.into(),
            expires_at_ms: 20,
            sequence: 2,
        };
        let rendered = format!("{webrtc:?} {webrtc:#?} {websocket:?} {websocket:#?}");
        assert!(!rendered.contains(SECRET), "secret leaked: {rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn dsl_live_token_state_and_effects_debug_redact_token() {
        const SECRET: &str = "live-bootstrap-secret";
        let mut state = super::dsl::MeerkatMachineState::default();
        state
            .live_websocket_token_expires_at_ms_by_token
            .insert(SECRET.to_owned(), 5);
        let effect = super::dsl::MeerkatMachineEffect::LiveWebrtcTokenIssued {
            session_id: "session-visible".to_owned(),
            channel_id: "channel-visible".to_owned(),
            token: SECRET.to_owned(),
            expires_at_ms: 3,
            sequence: 4,
        };
        let rendered = format!("{state:?} {effect:?} {effect:#?}");
        assert!(!rendered.contains(SECRET), "token leaked into Debug output");
        assert!(
            rendered.contains("live_websocket_token_expires_at_ms_by_token: <redacted; 1 entries>"),
            "state map not redacted"
        );
        assert!(
            rendered.contains("channel-visible"),
            "visible field missing"
        );
    }
}

/// Generated authority output for WebSocket token admission.
///
/// Constructed only from
/// `MeerkatMachineEffect::LiveWebsocketTokenAdmissionResolved`. The WebSocket
/// transport upgrade proceeds only when this effect admits the token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveWebsocketTokenAdmissionAuthority {
    pub admitted: bool,
    pub rejection: Option<dsl::LiveWebsocketTokenAdmissionRejection>,
    pub public_error_class: Option<dsl::LiveWebsocketTokenAdmissionPublicErrorClass>,
    pub sequence: u64,
}

/// Generated authority output for the public `live/status` result.
///
/// Constructed only from a `MeerkatMachineEffect::LiveChannelStatusResolved`
/// emitted after the live host supplies typed adapter-status observation
/// evidence.
#[derive(Debug, Clone)]
#[cfg(feature = "live")]
pub struct LiveChannelStatusAuthority {
    pub status: dsl::LiveChannelPublicStatus,
    pub sequence: u64,
    pub status_observation_sequence: u64,
    pub degradation_reason: Option<dsl::LiveChannelDegradationReason>,
    pub degradation_detail: Option<String>,
    /// Set when the channel closed on a media fault: whether the session may
    /// reopen it (its one media-fault reopen is not spent).
    pub media_fault_reopen_recommended: Option<bool>,
    pub channel_status_commit_authority: Option<meerkat_live::LiveChannelStatusCommitAuthority>,
}

#[cfg(feature = "live")]
impl LiveChannelStatusAuthority {
    pub(crate) fn from_generated_effect(
        channel_id: String,
        status: dsl::LiveChannelPublicStatus,
        sequence: u64,
        status_observation_sequence: u64,
        degradation_reason: Option<dsl::LiveChannelDegradationReason>,
        degradation_detail: Option<String>,
        media_fault_reopen_recommended: Option<bool>,
    ) -> Result<Self, String> {
        Ok(Self {
            status,
            sequence,
            status_observation_sequence,
            degradation_reason,
            degradation_detail,
            media_fault_reopen_recommended,
            channel_status_commit_authority: Some(build_live_channel_status_commit_authority(
                channel_id,
                status_observation_sequence,
            )?),
        })
    }

    #[must_use]
    pub fn channel_status_commit_authority(
        &self,
    ) -> Option<&meerkat_live::LiveChannelStatusCommitAuthority> {
        self.channel_status_commit_authority.as_ref()
    }

    #[must_use]
    pub fn into_channel_status_commit_authority(
        self,
    ) -> Option<meerkat_live::LiveChannelStatusCommitAuthority> {
        self.channel_status_commit_authority
    }
}

/// Told, with no argument, each time a machine makes an accepted input's
/// terminal readable without running it: an exact cancellation abandoned the
/// input while it was queued, a later admission superseded or coalesced it,
/// or a refused admission at its retry cap, an interrupted run's settlement,
/// a stop, a retirement without a runtime loop, a replaced or recovered
/// predecessor, or a failed accepted admission abandoned it. It carries no
/// state and grants nothing; whoever is told re-reads the native terminal
/// facts it cares about. It is only a signal: it must neither block nor call
/// back into the machine. See
/// [`MeerkatMachine::set_runless_input_terminal_hint`].
pub type RunlessInputTerminalHint = Arc<dyn Fn() + Send + Sync>;

/// Why a runtime refused to install an issuing consent owner as its
/// owner-loss settlement
/// ([`MeerkatMachine::install_consent_owner_loss_settlement`]). Nothing
/// changed: the runtime keeps its registrations and any settlement it
/// already had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConsentOwnerLossInstallRefused {
    /// A session is registered, or being registered, on this runtime: its
    /// registration published without this owner.
    #[error("a session is already registered on this runtime")]
    SessionsRegistered,
    /// Another owner-loss settlement is already installed on this runtime.
    #[error("another consent owner-loss settlement is already installed on this runtime")]
    AlreadyInstalled,
}

/// Why a runtime refused to install a governed peer delivery binder
/// ([`MeerkatMachine::install_peer_ingress_binder`]). Nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PeerIngressBinderInstallRefused {
    /// No native work authorization host is installed: the runtime is
    /// ungoverned, admits peer deliveries as it always did, and has nothing to
    /// bind them for.
    #[error("no native work authorization host is installed on this runtime")]
    Ungoverned,
    /// A session is registered, or being registered, on this runtime.
    #[error("a session is already registered on this runtime")]
    SessionsRegistered,
    /// Another binder is already installed on this runtime.
    #[error("another peer ingress binder is already installed on this runtime")]
    AlreadyInstalled,
}

/// One tell of a machine's [`RunlessInputTerminalHint`], owed by the step
/// that commits runless input terminals.
///
/// The step takes it before the native locks it commits under, so that it
/// drops after them, and [owes](Self::owe) it after each committed terminal.
/// It tells the hint once, when it drops or is [released](Self::release)
/// owing: after the locks are released, for a committed prefix even when a
/// later step fails, and, moved into a retained handoff, even when nobody
/// awaits that handoff any more. Moved with M into detached work, it is told
/// once that work releases M, not once the work ends.
#[derive(Default)]
#[must_use = "an owed runless terminal is told only when its announcement drops or is released"]
pub(crate) struct RunlessTerminalAnnouncement {
    hint: Option<RunlessInputTerminalHint>,
    owed: bool,
}

impl RunlessTerminalAnnouncement {
    /// A runless input terminal was committed.
    pub(crate) fn owe(&mut self) {
        self.owed = true;
    }

    /// Tell the hint now if a terminal is owed; nothing stays owed.
    pub(crate) fn release(&mut self) {
        if std::mem::take(&mut self.owed)
            && let Some(hint) = self.hint.as_ref()
        {
            hint();
        }
    }
}

impl Drop for RunlessTerminalAnnouncement {
    fn drop(&mut self) {
        self.release();
    }
}

/// [`MeerkatMachine::hold_run_starts`]'s seam in test builds: the delay every
/// hold waits before it takes the session's mutation gate, the session whose
/// holds fail, and the holds in flight (now and at most at once).
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default)]
struct RunStartHoldsForTest {
    delay: Option<std::time::Duration>,
    failing_session: Option<SessionId>,
    in_flight: usize,
    max_in_flight: usize,
}

/// A [`MeerkatMachine::hold_run_starts`] in flight, counted by the test seam
/// until it drops.
#[cfg(any(test, feature = "test-support"))]
pub(crate) struct RunStartHoldInFlightForTest<'a> {
    machine: &'a MeerkatMachine,
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for RunStartHoldInFlightForTest<'_> {
    fn drop(&mut self) {
        let mut seam = self.machine.run_start_holds_for_test();
        seam.in_flight = seam.in_flight.saturating_sub(1);
    }
}

/// Session-scoped execution kernel for the Meerkat runtime.
///
/// Owns per-session runtime state (driver, ops registry, completion waiters,
/// comms drain, epoch bindings) and routes all internal mutations through one
/// canonical command reducer, with smaller group handlers retained only as
/// implementation detail helpers.
#[doc(hidden)]
pub struct MeerkatMachineShared {
    native_work_authorization_host: crate::input_authority::NativeWorkAuthorizationSlot,
    /// Source of [`RuntimeSessionRegistrationUse`] stamps: one machine-wide
    /// sequence, so no two stamps this machine hands out are equal.
    registration_use_sequence: std::sync::atomic::AtomicU64,
    /// The issuing consent owner's owner-loss settlement, installed by the
    /// composition before any session is registered. Empty: nothing is
    /// settled and nothing is recorded as settled.
    consent_owner_loss_settlement:
        std::sync::OnceLock<Arc<dyn meerkat_core::approval::ConsentOwnerLossSettlement>>,
    /// The trusted host's binder of governed peer deliveries, installed by
    /// the composition before any session is registered. Empty: no peer
    /// delivery is bound, and a governed runtime refuses them as before.
    peer_ingress_binder: std::sync::OnceLock<Arc<dyn crate::input_authority::PeerIngressBinder>>,
    /// This runtime owner's identity for session hosting claims (#1813).
    /// Every claim this machine's lineage takes (its registrations, and its
    /// session service's actors, cold attaches and store-only writes) carries
    /// it; another machine over the same store is another owner and is
    /// refused a session this one hosts.
    hosting_owner: meerkat_core::session_hosting::HostingOwner,
    /// Per-session entries.
    sessions: RwLock<HashMap<SessionId, RuntimeSessionEntry>>,
    #[cfg(not(target_arch = "wasm32"))]
    credential_release_observer:
        std::sync::OnceLock<Arc<credential_custody::NativeCredentialReleaseObserver>>,
    /// Once-per-distinct-payload panic log gate for the attachment
    /// `catch_unwind` boundaries (executor factory, surface activation,
    /// surface publication). See `crate::panic_boundary` for the 2026-07-29
    /// incident WHY: provisioning retry loops may re-run a panicking
    /// boundary indefinitely, so the recovered payload is logged on
    /// transition, never per iteration.
    boundary_panic_log_gate: meerkat_core::panic_payload::PanicPayloadLogGate,
    /// Generation that advances each time a runtime attachment becomes
    /// serving. See [`MeerkatMachine::subscribe_attachment_commits`].
    attachment_commits: crate::tokio::sync::watch::Sender<u64>,
    /// Stable process-local serialization slots for session registration and
    /// final unregister publication. The index retains only weak references:
    /// every new lookup prunes dead slots, so historical session ids cannot
    /// accumulate while overlapping transactions still rendezvous on one gate.
    registration_transaction_slots: StdRwLock<HashMap<SessionId, std::sync::Weak<Mutex<()>>>>,
    /// Process-local dedup for exact runless-terminal publication callbacks.
    /// Keys are stable Arc identities for driver incarnations; values retain
    /// that Arc, preventing address reuse while a dispatch remains pending.
    pending_runless_terminal_publications:
        StdMutex<HashMap<usize, PendingRunlessTerminalPublicationDispatch>>,
    /// Per-session single-flight for process-owned archive lease preparation.
    /// Followers wait on the leader's completion instead of accumulating tasks
    /// behind a wedged registration transaction or injected RuntimeStore read.
    pending_session_archive_lease_preparations:
        StdMutex<HashMap<SessionId, PendingSessionArchiveLeasePreparation>>,
    /// Per-session single flight for the process-owned task of a predecessor
    /// settlement: its terminal drain, closing census and exact removal. A
    /// later settlement of the session waits for it, holding no gate, before
    /// it begins.
    pending_predecessor_settlements:
        StdMutex<HashMap<SessionId, predecessor_settlement::PendingPredecessorSettlement>>,
    /// Per-session process-owned serialization for durable V5 direct-member
    /// semantic admission. The task never holds residency or registration
    /// authority while awaiting a custom store.
    pending_direct_member_bind_admissions:
        StdMutex<HashMap<SessionId, PendingDirectMemberBindAdmission>>,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
    /// Same actual backend claim retained by exported capabilities, drivers,
    /// and owned tasks. Failed acquisition never constructs a machine.
    execution_custody: Option<Arc<crate::store::RuntimeStoreExecutionClaim>>,
    /// Blob store used by persistent drivers for durable input externalization.
    blob_store: Option<Arc<dyn BlobStore>>,
    /// Runtime-owned shell seam for live session LLM reconfiguration I/O.
    llm_reconfigure_host: StdRwLock<Option<Arc<dyn SessionLlmReconfigureHost>>>,
    /// Shared readiness flag mirroring `llm_reconfigure_host.is_some()`.
    ///
    /// Handed to every model-routing handle so a factory build can ask whether
    /// this runtime can actually realize a committed handoff, rather than
    /// inferring it from build mode alone.
    reconfigure_host_ready: Arc<std::sync::atomic::AtomicBool>,
    /// Machine-wide injected member-observation host (multi-host mobs
    /// DEC-P6E-2; the `llm_reconfigure_host` precedent). Serves the
    /// `ReadMemberHistory` / `PollMemberEvents` drain arms and directed-turn
    /// admission; absent ⇒ those arms reply typed `Unavailable`.
    member_observation_host:
        StdRwLock<Option<Arc<dyn crate::member_observation::MemberObservationHost>>>,
    /// Machine-wide injected member live host (multi-host mobs ADJ-P6B-1;
    /// the `member_observation_host` precedent). Serves the four
    /// member-addressed live drain arms; absent ⇒ those arms reply typed
    /// `LiveTransportUnavailable`. Shell composition slot, NOT machine
    /// state — no catalog delta.
    member_live_host: StdRwLock<Option<Arc<dyn crate::member_live::MemberLiveHost>>>,
    /// Host-injected provider of interrupted-run evidence (durable tool
    /// process custody). Every pending attachment of a session settles the
    /// session's earlier-incarnation tool processes through it before it can
    /// serve. Shell composition slot, NOT machine state.
    interrupted_tool_evidence_source:
        StdRwLock<Option<Arc<dyn meerkat_core::tool_process::InterruptedToolEvidenceSource>>>,
    /// Provider-neutral host for generated canonical context appends.
    #[cfg(feature = "live")]
    live_context_mirror_host:
        StdRwLock<Option<Arc<dyn crate::live_context_mirror::LiveContextMirrorHost>>>,
    /// Session event publication for committed live channel closes.
    #[cfg(feature = "live")]
    live_channel_close_publisher:
        StdRwLock<Option<Arc<dyn crate::live_execution::LiveChannelCloseEventPublisher>>>,
    /// Live channel close operations executing right now, by channel, with
    /// the waiters woken as each one ends. See
    /// [`MeerkatMachine::begin_live_channel_close`].
    #[cfg(feature = "live")]
    live_channel_closes_in_flight: Arc<LiveChannelClosesInFlight>,
    /// Sealed committed-row custody retained across generated unsafe turn
    /// boundaries. Keys are session-scoped canonical row sequences.
    #[cfg(feature = "live")]
    live_context_queued_rows:
        StdMutex<HashMap<(SessionId, u64), crate::live_execution::LiveContextQueuedRow>>,
    /// Titles of voice delegations whose result was merged into the session
    /// after their channel closed, keyed by the delegation's interaction (the
    /// merge reply's transcript interaction): a reopened channel replays the
    /// reply framed as that request's result
    /// ([`crate::live_execution::post_close_result_context`]).
    #[cfg(feature = "live")]
    post_close_result_titles:
        StdMutex<HashMap<(SessionId, meerkat_core::interaction::InteractionId), String>>,
    #[cfg(feature = "live")]
    live_context_preparation_leases: StdMutex<
        HashMap<
            (SessionId, meerkat_core::LiveChannelId),
            crate::live_execution::LiveContextPreparationLease,
        >,
    >,
    /// Serialize local row projection and generated claim selection only;
    /// never hold this mechanical gate while awaiting provider delivery.
    #[cfg(feature = "live")]
    live_context_projection_gates:
        StdMutex<HashMap<SessionId, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    #[cfg(feature = "live")]
    live_context_drain_tasks: StdMutex<
        HashMap<
            (SessionId, meerkat_core::LiveChannelId),
            Arc<crate::live_context_mirror::LiveContextDrainTask>,
        >,
    >,
    #[cfg(feature = "live")]
    live_context_projection_tasks:
        StdMutex<HashMap<SessionId, Arc<crate::live_context_mirror::LiveContextDrainTask>>>,
    /// Process-local custody of generated assistant-output handles. Semantic
    /// identity was frozen by the generated Assistant TurnStarted transition;
    /// these maps provide only exact host addressability.
    #[cfg(feature = "live")]
    live_assistant_output_by_turn: StdMutex<
        HashMap<(SessionId, meerkat_core::LiveChannelId, String), LiveAssistantOutputHandle>,
    >,
    #[cfg(feature = "live")]
    live_assistant_output_by_id: StdMutex<HashMap<String, LiveAssistantOutputHandle>>,
    /// Channels whose output handles a deferred close settlement still
    /// replays rows against; see [`LiveAssistantOutputRetirementHold`].
    #[cfg(feature = "live")]
    live_assistant_output_retirement_holds: StdMutex<
        HashMap<(SessionId, meerkat_core::LiveChannelId), LiveAssistantOutputRetirementHolds>,
    >,
    /// Count of live bridge commands that reached this member's drain arms
    /// (ADJ-P6B seam S3b). Deterministic lanes pin zero-bridge-traffic
    /// gates against it; mechanical observability, not machine state.
    live_commands_served: std::sync::atomic::AtomicU64,
    /// Exact host-materialized incarnation registered for each resident
    /// member session, paired with the runtime-session mutation gate that was
    /// current when that residency was installed.  Remembering the gate is
    /// what makes a reused `SessionId` distinguishable from the runtime entry
    /// that the member incarnation actually materialized.
    /// Stable, never-replaced serialization slot for each member session id.
    /// Registration replacement/removal and every incarnation-fenced live
    /// effect acquire this same slot.  Keeping the `Arc` after unregister is
    /// intentional: a delayed G1 effect and a concurrent G2 install must
    /// rendezvous even when there was a transient unregistered interval.
    member_incarnation_slots: StdRwLock<HashMap<SessionId, Arc<MemberResidencySlot>>>,
    /// AuthMachine lifecycle authority shared by runtime-backed auth
    /// resolution/refresh paths and public auth-status surfaces.
    auth_lease: StdRwLock<meerkat_core::handles::GeneratedAuthLeaseHandle>,
    /// OAuth login-flow lifecycle authority shared by public auth surfaces
    /// that operate through this runtime adapter.
    #[cfg(not(target_arch = "wasm32"))]
    oauth_flows: StdRwLock<Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority>>,
    /// Runtime-scoped generated authority for live control/command rejections
    /// that cannot be attributed to a session because generated active-channel
    /// ownership has no binding for the requested channel.
    #[cfg(feature = "live")]
    live_unbound_rejection_authority: crate::driver::ephemeral::SharedIngressDslAuthority,
    /// Canonical owner of "this session id is currently active" — replaces
    /// the deleted process-global `SESSION_IDENTITY_CLAIMS` static in the
    /// comms shell (dogma #2). Comms runtimes acquire a typed
    /// [`meerkat_core::handles::SessionClaim`] through this handle and hold
    /// it for their lifetime; the registry is scoped to this `MeerkatMachine`
    /// instance, so tests / multi-runtime processes get clean isolation.
    session_claims: Arc<crate::handles::RuntimeSessionClaimRegistry>,
    /// Wake signal for [`MeerkatMachine::wait_run_settled`]. The
    /// runtime loops of this machine, which execute every run and record its
    /// end, bump it after each run iteration, each loop iteration and at loop
    /// exit. It carries no state: waiters re-check the session's DSL
    /// `current_run_id` after every wake.
    run_settlements: crate::tokio::sync::watch::Sender<u64>,
    /// Host-installed [`RunlessInputTerminalHint`]. Shell composition slot,
    /// NOT machine state.
    runless_input_terminal_hint: StdRwLock<Option<RunlessInputTerminalHint>>,
    /// One-shot deterministic fault for the materializer's executor-attach
    /// publication window. Test-support only; production builds compile the
    /// post-ensure hook to a no-op and carry no field.
    #[cfg(feature = "test-support")]
    test_stop_executor_after_ensure: std::sync::atomic::AtomicBool,
    /// Optional deterministic pause paired with the post-ensure stop fault.
    /// Cross-crate tests acquire competing surface boundaries immediately
    /// before the exact pending attachment is stopped and cleanup begins.
    #[cfg(feature = "test-support")]
    test_pause_executor_after_ensure: std::sync::atomic::AtomicBool,
    #[cfg(feature = "test-support")]
    test_executor_after_ensure_pause_reached: crate::tokio::sync::Notify,
    #[cfg(feature = "test-support")]
    test_executor_after_ensure_pause_release: crate::tokio::sync::Notify,
    /// One-shot hold armed by a test: the next unregister teardown saga this
    /// machine starts waits here (entered, release) before it tears down.
    /// Per-machine unregister caller grace override set by a test.
    #[cfg(feature = "test-support")]
    test_unregister_caller_wait_grace: StdMutex<Option<std::time::Duration>>,
    #[cfg(feature = "test-support")]
    test_unregister_saga_hold: StdMutex<
        Option<(
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot hold armed by a test: the next live channel custody or
    /// playback-readiness read waits here (entered, release) right after it
    /// takes its machine snapshot.
    #[cfg(all(feature = "live", any(test, feature = "test-support")))]
    test_live_custody_read_hold: StdMutex<
        Option<(
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot witness armed by a test: the next caller that waits on an
    /// unregister teardown saga reports the wait it dispatched on.
    #[cfg(feature = "test-support")]
    test_unregister_wait_witness:
        StdMutex<Option<crate::tokio::sync::oneshot::Sender<UnregisterTeardownWaitWitness>>>,
    /// One-shot signal armed by a test: the next unregister teardown saga
    /// reports once it holds its session's live lifecycle lease.
    #[cfg(feature = "test-support")]
    test_unregister_saga_live_lease_signal:
        StdMutex<Option<crate::tokio::sync::oneshot::Sender<()>>>,
    /// This machine's user-interrupt acknowledgement bound. Tests scope it
    /// per machine: a short bound would make every success-path interrupt
    /// race the process-global cleanup dispatcher that all in-process tests
    /// share, so only a test that exercises a wedged callback shortens it.
    #[cfg(any(test, feature = "test-support"))]
    test_user_interrupt_ack_timeout: StdMutex<std::time::Duration>,
    /// Deterministic test gate after fenced input captures its residency slot
    /// and exact session gate but before it locks that session gate.
    #[cfg(test)]
    test_fenced_accept_after_lease: StdMutex<
        Option<(
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// Deterministic test gate inside accepted-ingress boundary work, while
    /// it still owns the admission's session mutation gate.
    #[cfg(test)]
    test_accepted_boundary_work_hold: StdMutex<
        Option<(
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// Deterministic test gate in a durable steer's live-boundary work after
    /// its wait for an earlier durable steer ended, before it retakes the
    /// session mutation gate. One-shot: the first waiter to get there takes it.
    #[cfg(test)]
    test_durable_steer_relock_hold: StdMutex<
        Option<(
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// Receives each accepted-ingress boundary work's input and outcome, when
    /// a test installs a sender.
    #[cfg(test)]
    test_accepted_boundary_work_outcomes: StdMutex<
        Option<crate::tokio::sync::mpsc::UnboundedSender<(Option<InputId>, Result<(), String>)>>,
    >,
    /// Deterministic test gate after an executor attachment's runtime loop
    /// has released the registration gate it held through startup recovery,
    /// before the attachment reacquires that gate as its pending guard.
    #[cfg(test)]
    test_pending_attachment_before_regate: StdMutex<
        Option<(
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot positive witness for registration-slot contention. The armed
    /// acquisition itself uses `try_lock_owned`, reports whether it found the
    /// exact stable slot held, then either returns that guard or waits on that
    /// same slot. Test-only mechanical observation, not lifecycle authority.
    #[cfg(test)]
    test_registration_transaction_contention_probe:
        StdMutex<Option<(SessionId, crate::tokio::sync::oneshot::Sender<bool>)>>,
    /// One-shot fault after a machine-managed executor hands its exact
    /// post-stop mutation fence into cleanup. Proves retry reacquires instead
    /// of retaining or fabricating that consumed guard.
    #[cfg(test)]
    test_fail_post_stop_unregister_after_fence: StdMutex<Option<SessionId>>,
    /// One-shot fault at the typed DSL apply boundary after generated state
    /// commits but before routed effects settle. This proves callers can
    /// reconcile a committed transition after a recoverable dispatch error.
    #[cfg(test)]
    test_fail_next_typed_dsl_post_commit_dispatch: std::sync::atomic::AtomicBool,
    /// One-shot fault after an exact-run boundary cancel is staged: its
    /// runtime-effect projection fails. Proves a failure after staging is
    /// reported as owned by the recorded cancel.
    #[cfg(test)]
    test_fail_next_boundary_cancel_effect_projection: std::sync::atomic::AtomicBool,
    /// One-shot fault in the withdrawal of a staged boundary-cancel dispatch
    /// after a later failure. Proves the withdrawal's own error is reported
    /// as owned by the recorded cancel too.
    #[cfg(test)]
    test_fail_next_boundary_cancel_dispatch_abort: std::sync::atomic::AtomicBool,
    /// Fail the next record of a deferred close-time playback settlement
    /// after its machine transition, as a failed recovery-state persist does.
    #[cfg(all(feature = "live", any(test, feature = "test-support")))]
    test_fail_next_live_close_settlement_deferral: std::sync::atomic::AtomicBool,
    /// One-shot deterministic gate before the runtime loop's terminal run
    /// commit takes the session driver. The turn is terminal in its live
    /// agent while its run input is still applied and unconsumed, and the
    /// driver is free: cross-crate tests read that state without racing the
    /// commit.
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_loop_before_terminal_commit: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// Run-start holds a session's next registration applies (#1500),
    /// before its runtime loop can start a run. A reason stays staged until
    /// it is released, so every re-registration re-applies exactly the
    /// reasons still outstanding.
    registration_run_start_holds:
        std::sync::Mutex<HashMap<SessionId, std::collections::BTreeSet<dsl::RunStartHoldReason>>>,
    /// One-shot deterministic gate after the runtime loop's terminal run
    /// commit landed in the store but before the committed boundary is
    /// acknowledged to the session executor (the turn-finalization boundary
    /// is still held).
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_loop_before_boundary_acknowledgement: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic gate in an executor attachment commit that
    /// owes interrupted-run notices: after its publication (the loop serves
    /// and was woken), before the notices are handed to the session.
    #[cfg(any(test, feature = "test-support"))]
    test_attachment_interrupted_notice_handoff: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic gate at the runtime loop's next pre-dequeue
    /// check that finds no owed interrupted-run notice (the
    /// turn-finalization boundary is held).
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_loop_after_empty_interrupted_notice_check: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// Runtime-loop parks on held run starts (#1500), counted so tests can
    /// wait for the park as a positive event.
    #[cfg(any(test, feature = "test-support"))]
    test_run_start_held_parks: crate::tokio::sync::watch::Sender<u64>,
    /// Sessions whose runtime loop is parked with no buffered wake or effect:
    /// it awaits its next wake. Test support, so a test can admit input "now
    /// that the loop is idle" as a positive event instead of a state poll.
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_loop_parked:
        crate::tokio::sync::watch::Sender<std::collections::HashSet<SessionId>>,
    /// Boundary cancels dispatched to an executor's boundary handle (#1471),
    /// counted so tests can wait for the dispatch as a positive event.
    #[cfg(any(test, feature = "test-support"))]
    test_boundary_cancel_dispatches: crate::tokio::sync::watch::Sender<u64>,
    /// The run-start hold seam ([`RunStartHoldsForTest`]).
    #[cfg(any(test, feature = "test-support"))]
    test_run_start_holds: StdMutex<RunStartHoldsForTest>,
    /// One-shot deterministic gate after the runtime loop's first ready-effect
    /// drain but before it acquires queue authority. Tests publish an executor
    /// effect in this exact gap and prove the consumed wake is retained.
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_loop_before_queue_authority: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic gate after the runtime loop staged a run and
    /// signalled its turn start but before it calls the executor's `apply`
    /// (#1471): the run is current in the machine and the session has not
    /// claimed the turn yet.
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_loop_before_executor_apply: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic gate between a runtime loop lap's read of its
    /// fresh batch's durable rows and the batch's staging commit: the lap
    /// holds the turn-finalization boundary and its queue authority, not the
    /// driver.
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_loop_before_batch_staging: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic gate in a scoped context binding, immediately
    /// before its driver takes the session document's machine authority:
    /// its registration, mutation gate and driver are held, and its binder's
    /// final decision has not run. Tests complete an authorization
    /// publication while a binding is held here.
    #[cfg(any(test, feature = "test-support"))]
    test_scoped_binding_before_machine_authority: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic gate after a ReloadRequired discard publishes
    /// its exact cold successor but before the detached coordinator reports
    /// completion to its original waiter. Cross-crate recovery tests cancel
    /// that waiter here and prove a retry reconstructs the registry handoff
    /// from the already-current successor.
    #[cfg(any(test, feature = "test-support"))]
    test_reload_required_discard_after_successor_publication: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic gate in a ReloadRequired discard worker, after
    /// the registration transaction was released and any teardown handoff
    /// claimed, before the worker takes the registration's mutation gate.
    #[cfg(any(test, feature = "test-support"))]
    test_reload_required_discard_before_mutation_gate: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot report of a degradation-aware wait for one session's mutation
    /// gate that found the gate held: the waiter has subscribed to the
    /// session's degradation and is about to park behind the holder.
    #[cfg(any(test, feature = "test-support"))]
    test_mutation_gate_wait_observer:
        StdMutex<Option<(SessionId, crate::tokio::sync::oneshot::Sender<()>)>>,
    /// Test seam: the busy window this machine's runtime loops measure their
    /// stores' commit outcome declarations in.
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_commit_busy_window: StdMutex<Option<std::time::Duration>>,
    /// Test probe: how many times a caller that left a degraded session
    /// without settling it handed the degraded settlement to a process-owned
    /// task (a ready-gated waiter that left on the degradation signal, an
    /// input admission whose store write degraded the session, or the
    /// reload-required discard of a registration whose runtime loop is still
    /// attached). Counts calls: a call that finds a task already waiting for
    /// the gate spawns nothing (see `test_degraded_gate_settlement_spawns`).
    #[cfg(test)]
    test_degraded_gate_settlement_hand_offs: std::sync::atomic::AtomicUsize,
    /// The mutation gates, by address, with a degraded settlement task
    /// waiting to run. A caller that hands the settlement off spawns that
    /// task only for a gate that has none, so callers that keep arriving
    /// behind one holder add no tasks. The task keeps its gate alive while
    /// the gate is listed, so a listed address is never another gate's.
    pending_degraded_gate_settlements: StdMutex<HashSet<usize>>,
    /// Test probe: how many degraded settlement tasks were spawned.
    #[cfg(test)]
    test_degraded_gate_settlement_spawns: std::sync::atomic::AtomicUsize,
    /// One-shot deterministic gate after a logical control command resolves
    /// its SessionId but before it acquires the current entry's mutation gate.
    /// Tests replace A with B in this window to prove the command captures no
    /// incarnation-local handles before M.
    #[cfg(test)]
    test_control_command_after_logical_lookup: StdMutex<
        Option<(
            ControlCommandLookupTestKind,
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// One-shot deterministic pause after durable unregister finalization and
    /// before the exact entry-removal mechanics begin.
    #[cfg(test)]
    test_before_finalized_unregister_removal: StdMutex<
        Option<(
            SessionId,
            crate::tokio::sync::oneshot::Sender<()>,
            crate::tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlCommandLookupTestKind {
    PublishEvent,
    Recycle,
    Recover,
    Destroy,
    Retire,
}

/// Exclusive lifecycle transaction for one stable member-residency slot.
/// Host materialization/revival acquires this before an old runtime can be
/// quiesced or a replacement can become reachable, then carries it back to
/// the host actor's durable commit point. Dropping an uncommitted update
/// publishes a vacant placed slot, so stale authority can never reappear
/// after a failed cutover.
pub struct MemberResidencyUpdate {
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    slot: Arc<MemberResidencySlot>,
    slot_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    committed: bool,
}

/// Post-commit publication guard for the normal host-delivery path. The placed
/// registration and journal are already visible, but no member effect may
/// acquire the residency slot until the host actor has published its matching
/// observation projection/cache. If the host responder/runtime disappears
/// after its durable row and machine residency commit, those durable facts are
/// restart authority; the process-owned publication result may be dropped as
/// part of that runtime-catastrophe boundary.
pub struct MemberResidencyPublication {
    _slot_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

/// Reversible publication used only while an exact attachment still owns M.
///
/// The placed residency is visible synchronously to the machine commit, but
/// dropping this value before `finalize` restores the host-owned vacancy while
/// the stable residency slot is still exclusively held. This is what lets a
/// failed runtime-loop serving release roll back every local publication at
/// the same linearization point.
struct StagedMemberResidencyPublication {
    slot: Arc<MemberResidencySlot>,
    slot_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
}

impl StagedMemberResidencyPublication {
    fn finalize(mut self) -> Result<MemberResidencyPublication, RuntimeDriverError> {
        let slot_guard = self.slot_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "staged member residency lost its exclusive slot guard".to_string(),
            )
        })?;
        Ok(MemberResidencyPublication {
            _slot_guard: slot_guard,
        })
    }
}

impl Drop for StagedMemberResidencyPublication {
    fn drop(&mut self) {
        if self.slot_guard.is_none() {
            return;
        }
        *self
            .slot
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            MemberResidencyState::VacantPlaced;
    }
}

impl MemberResidencyUpdate {
    fn validate_for_retained_attachment(
        &self,
        adapter: &Arc<MeerkatMachine>,
        witness: &RuntimeExecutorAttachmentWitness,
        incarnation: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Result<(), RuntimeDriverError> {
        if !Arc::ptr_eq(&self.adapter, adapter) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "member residency update belongs to another machine".to_string(),
            });
        }
        if &self.session_id != witness.session_id() {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "member residency update for session {} cannot publish attachment for {}",
                    self.session_id,
                    witness.session_id()
                ),
            });
        }
        if self.slot_guard.is_none() {
            return Err(RuntimeDriverError::Internal(
                "member residency update lost its publication guard".to_string(),
            ));
        }
        self.adapter
            .validate_member_incarnation_registration(&self.session_id, incarnation)
    }

    fn stage_under_retained_mutation_gate(
        mut self,
        incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        tracked_turn_journal: Option<Arc<dyn crate::member_observation::TrackedTurnJournal>>,
        session_mutation_gate: SessionMutationGate,
    ) -> Result<StagedMemberResidencyPublication, RuntimeDriverError> {
        let slot_guard = self.slot_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "member residency update lost its publication guard".to_string(),
            )
        })?;
        *self
            .slot
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            MemberResidencyState::Placed(MemberIncarnationRegistration {
                incarnation,
                session_mutation_gate,
                tracked_turn_journal,
            });
        self.committed = true;
        Ok(StagedMemberResidencyPublication {
            slot: Arc::clone(&self.slot),
            slot_guard: Some(slot_guard),
        })
    }

    /// Publish the exact placed residency and optional tracked-turn journal
    /// while the lifecycle slot is still exclusively held.
    pub async fn commit(
        self,
        incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        tracked_turn_journal: Option<Arc<dyn crate::member_observation::TrackedTurnJournal>>,
    ) -> Result<MemberResidencyPublication, RuntimeDriverError> {
        self.adapter
            .validate_member_incarnation_registration(&self.session_id, &incarnation)?;
        let session_mutation_gate = self
            .adapter
            .lock_current_durability_ready_session_mutation_gate(&self.session_id)
            .await?;
        let gate = {
            let sessions = self.adapter.sessions.read().await;
            sessions
                .get(&self.session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?
                .mutation_gate
                .clone()
        };
        let publication = self
            .stage_under_retained_mutation_gate(incarnation, tracked_turn_journal, gate)
            .and_then(StagedMemberResidencyPublication::finalize);
        drop(session_mutation_gate);
        publication
    }

    /// Publish the host-owned slot as VacantPlaced while retaining the gate
    /// until the host actor has removed the matching observation projection.
    pub fn vacate(mut self) -> Result<MemberResidencyPublication, RuntimeDriverError> {
        let slot_guard = self.slot_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "member residency update lost its vacancy publication guard".to_string(),
            )
        })?;
        *self
            .slot
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            MemberResidencyState::VacantPlaced;
        self.committed = true;
        Ok(MemberResidencyPublication {
            _slot_guard: slot_guard,
        })
    }
}

impl Drop for MemberResidencyUpdate {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        *self
            .slot
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            MemberResidencyState::VacantPlaced;
    }
}

/// A non-owning, opaque identity of one machine's runtime owner
/// ([`MeerkatMachine::runtime_owner`]). It is not an execution handle: it
/// reaches nothing of the machine and grants nothing. Holding it keeps none
/// of that owner alive (no execution custody, session fact or store). It is
/// not serializable: it names an owner only in this process, while held.
/// While it is held the owner's allocation is not reused, so it never names
/// another owner's handle; once every handle of its owner is gone it names
/// no handle at all.
#[derive(Clone)]
pub struct RuntimeOwnerIdentity(std::sync::Weak<MeerkatMachineShared>);

impl std::fmt::Debug for RuntimeOwnerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RuntimeOwnerIdentity")
    }
}

impl RuntimeOwnerIdentity {
    /// Whether `machine` is a handle of this exact runtime owner, as
    /// [`MeerkatMachine::is_same_runtime_owner`] decides between two handles.
    #[must_use]
    pub fn is_owner_of(&self, machine: &MeerkatMachine) -> bool {
        std::ptr::eq(self.0.as_ptr(), Arc::as_ptr(&machine.shared))
    }
}

/// Cloneable handle to the process-local runtime authority.
///
/// Clones share every machine-owned session fact and shell-mechanics lock.
/// The owned unregister coordinator uses a clone so dropping any individual
/// stop/unregister caller can never cancel teardown halfway through the
/// generated `Draining` window.
#[derive(Clone)]
pub struct MeerkatMachine {
    shared: Arc<MeerkatMachineShared>,
}

/// A governed persistent machine before its native host is installed.
///
/// It owns the realm's governed execution custody and the machine's sole
/// auth lifecycle owner, with no session registration and no native work
/// authorization host. A host composition prepares work that needs that
/// actual owner (for example a controller resolved under its auth lease)
/// and performs its idle administrative effects under the same custody,
/// then installs the native host exactly once through one of the
/// `install_*` methods, which return the ordinary machine. Installation
/// refuses while any clone of this machine is still shared.
///
/// Not `Clone`: the stage has one owner. Dropping it releases its custody
/// only when no handle sharing that custody survives (see
/// [`Self::release`]).
pub struct StagedMeerkatMachine {
    machine: MeerkatMachine,
}

/// The execution custody a persistent machine acquires over its store.
enum MachineCustody {
    /// An ordinary shared claim.
    Shared,
    /// The store's exclusive governed claim, retaining an associated store's
    /// governed claim with it when one is supplied.
    Governed {
        associated: Option<crate::store::RuntimeStoreExecutionClaim>,
    },
}

/// What [`StagedMeerkatMachine::release`] did to the stage's execution
/// custody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedCustodyRelease {
    /// The release itself completed the custody claim's final destruction:
    /// the realm's execution custody is released.
    Released,
    /// Another handle sharing the custody (an exported auth lease clone held
    /// by owned auth work, or a machine clone) still holds the claim; its
    /// last holder releases the realm, and nothing here waits for it.
    Retained,
}

impl StagedMeerkatMachine {
    /// The machine's generated auth lifecycle owner: the same owner its
    /// sessions' bindings clone once the stage is installed. A clone shares
    /// the stage's custody for as long as it lives.
    pub fn generated_auth_lease_handle(&self) -> meerkat_core::handles::GeneratedAuthLeaseHandle {
        self.machine.generated_auth_lease_handle()
    }

    /// The machine's current provider-auth pair, exactly as
    /// [`MeerkatMachine::provider_auth_runtime_authority`] snapshots it: the
    /// generated auth lifecycle owner above with the OAuth flow authority
    /// over that same owner, never a new one. Like an auth lease clone, the
    /// pair shares the stage's custody claim and not the machine: it does
    /// not refuse installation, and while it lives [`Self::release`] reports
    /// `Retained`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn provider_auth_runtime_authority(&self) -> ProviderAuthRuntimeAuthority {
        self.machine.provider_auth_runtime_authority()
    }

    /// A clone for one administrative effect performed under this stage's
    /// custody. The native host cannot be installed until every such clone
    /// has been dropped.
    pub fn administrative_machine(&self) -> MeerkatMachine {
        self.machine.clone()
    }

    /// Drop the stage without installing a host and report whether that
    /// released its execution custody.
    ///
    /// The exact claim is kept while the stage is dropped. `Released` is
    /// reported only when this call then holds the only reference to it (no
    /// other strong or weak reference, so none can appear) and has run the
    /// claim's final destruction, in place, to completion: the realm's
    /// custody is free when it returns. Otherwise another holder still shares
    /// the claim and `Retained` is reported; that holder's own final drop
    /// releases the realm, and nothing here waits for it.
    pub fn release(self) -> StagedCustodyRelease {
        let claim = self.machine.shared.execution_custody.clone();
        drop(self);
        let Some(mut claim) = claim else {
            return StagedCustodyRelease::Released;
        };
        if Arc::get_mut(&mut claim).is_none() {
            return StagedCustodyRelease::Retained;
        }
        drop(claim);
        StagedCustodyRelease::Released
    }

    /// Hold the final destruction of the stage's execution custody claim,
    /// whichever thread runs it (test support): the receiver reports that
    /// thread once it is parked there, still holding the custody, and the
    /// destruction completes once the sender is used or dropped. `None`
    /// without execution custody.
    #[cfg(any(test, feature = "test-support"))]
    pub fn hold_custody_release_for_test(
        &self,
    ) -> Option<(
        std::sync::mpsc::Receiver<std::thread::ThreadId>,
        std::sync::mpsc::Sender<()>,
    )> {
        self.machine
            .shared
            .execution_custody
            .as_ref()
            .map(crate::store::execution_custody_final_drop_hold)
    }
}

impl std::fmt::Debug for StagedMeerkatMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedMeerkatMachine")
            .finish_non_exhaustive()
    }
}

impl std::ops::Deref for MeerkatMachine {
    type Target = MeerkatMachineShared;

    fn deref(&self) -> &Self::Target {
        self.shared.as_ref()
    }
}

impl MeerkatMachine {
    /// Return the stable transaction slot for one session id.
    ///
    /// Cold registration holds this slot across durable recovery/ops-epoch
    /// initialization and session-map publication. Unregister uses one section
    /// to install its exact coordinator and another around durable finalization
    /// plus compare-and-remove; the present coordinator/Draining entry fences
    /// the quiescence interval between them. A weak index preserves rendezvous
    /// for overlapping operations without retaining every session id for the
    /// lifetime of the machine.
    fn session_registration_transaction_slot(&self, session_id: &SessionId) -> Arc<Mutex<()>> {
        let mut slots = self
            .registration_transaction_slots
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = slots.get(session_id).and_then(std::sync::Weak::upgrade) {
            return slot;
        }
        slots.retain(|_, slot| slot.upgrade().is_some());
        let slot = Arc::new(Mutex::new(()));
        slots.insert(session_id.clone(), Arc::downgrade(&slot));
        slot
    }

    async fn lock_session_registration_transaction(
        &self,
        session_id: &SessionId,
    ) -> crate::tokio::sync::OwnedMutexGuard<()> {
        let slot = self.session_registration_transaction_slot(session_id);
        #[cfg(test)]
        let contention_probe = {
            let mut probe = self
                .test_registration_transaction_contention_probe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if probe
                .as_ref()
                .is_some_and(|(probe_session_id, _)| probe_session_id == session_id)
            {
                probe.take().map(|(_, sender)| sender)
            } else {
                None
            }
        };
        #[cfg(test)]
        if let Some(contention_probe) = contention_probe {
            return match Arc::clone(&slot).try_lock_owned() {
                Ok(guard) => {
                    let _ = contention_probe.send(false);
                    guard
                }
                Err(_) => {
                    let _ = contention_probe.send(true);
                    slot.lock_owned().await
                }
            };
        }
        slot.lock_owned().await
    }

    /// Hold `session_id`'s registration transaction until the guard drops, so
    /// a test can stand a session's unregister admission behind it.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn hold_session_registration_transaction_for_test(
        &self,
        session_id: &SessionId,
    ) -> crate::tokio::sync::OwnedMutexGuard<()> {
        self.lock_session_registration_transaction(session_id).await
    }

    /// Hold `session_id`'s session mutation gate until the guard drops, as a
    /// run's finalization holds it, so a test can stand work that takes the
    /// gate behind it. `None` when the session is not registered.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn hold_session_mutation_gate_for_test(
        &self,
        session_id: &SessionId,
    ) -> Option<crate::tokio::sync::OwnedMutexGuard<()>> {
        let gate = self.session_mutation_gate(session_id).await?;
        Some(gate.lock_settled(SettledGateWait::TestFixture).await)
    }

    #[cfg(test)]
    pub(crate) fn probe_next_session_registration_transaction_contention_for_test(
        &self,
        session_id: &SessionId,
    ) -> crate::tokio::sync::oneshot::Receiver<bool> {
        let (sender, receiver) = crate::tokio::sync::oneshot::channel();
        *self
            .test_registration_transaction_contention_probe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((session_id.clone(), sender));
        receiver
    }

    /// Begin an authority-owned placed-residency lifecycle update. The stable
    /// slot Arc is never removed, even when this transaction aborts.
    pub async fn begin_member_residency_update(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> MemberResidencyUpdate {
        let slot = self.member_incarnation_slot(&session_id);
        let slot_guard = Arc::clone(&slot.gate).lock_owned().await;
        *slot
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            MemberResidencyState::VacantPlaced;
        MemberResidencyUpdate {
            adapter: Arc::clone(self),
            session_id,
            slot,
            slot_guard: Some(slot_guard),
            committed: false,
        }
    }

    /// Capability token for store-only session-control mutations routed
    /// through this machine authority.
    #[must_use]
    pub fn session_control_authority(&self) -> MachineSessionControlAuthority {
        MachineSessionControlAuthority { _private: () }
    }

    /// Install the machine-wide member-observation host (multi-host mobs
    /// DEC-P6E-2). Called by the composing surface (the mob host daemon);
    /// sessions resolve per call, so residency establishment needs no
    /// per-session install.
    pub fn set_member_observation_host(
        &self,
        host: Arc<dyn crate::member_observation::MemberObservationHost>,
    ) {
        *self
            .member_observation_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    }

    /// Resolve the injected member-observation host, if composed.
    pub(crate) fn member_observation_host(
        &self,
    ) -> Option<Arc<dyn crate::member_observation::MemberObservationHost>> {
        self.member_observation_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Install the machine-wide member live host (multi-host mobs
    /// ADJ-P6B-1). Called by the composing surface (the mob host daemon,
    /// live-capable `rkat-rpc`); sessions resolve per call, so member
    /// materialization needs no per-session install.
    pub fn set_member_live_host(&self, host: Arc<dyn crate::member_live::MemberLiveHost>) {
        *self
            .member_live_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    }

    /// Install the host's provider of interrupted-run evidence. Every
    /// subsequent pending executor attachment settles its session's
    /// earlier-incarnation tool processes through it before serving, on
    /// every surface, even when no agent has been built for the session yet.
    pub fn set_interrupted_tool_evidence_source(
        &self,
        source: Arc<dyn meerkat_core::tool_process::InterruptedToolEvidenceSource>,
    ) {
        *self
            .interrupted_tool_evidence_source
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
    }

    pub(crate) fn interrupted_tool_evidence_source(
        &self,
    ) -> Option<Arc<dyn meerkat_core::tool_process::InterruptedToolEvidenceSource>> {
        self.interrupted_tool_evidence_source
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Install the [`RunlessInputTerminalHint`] this machine tells after it
    /// makes an accepted input's terminal readable without a run, once the
    /// native locks it committed the terminal under are released. A later
    /// install replaces the earlier one; announcements already taken keep
    /// the hint they were taken with.
    pub fn set_runless_input_terminal_hint(&self, hint: RunlessInputTerminalHint) {
        *self
            .runless_input_terminal_hint
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hint);
    }

    /// Tell the installed [`RunlessInputTerminalHint`], if any. Call it only
    /// once the terminal it announces is durable and readable and no native
    /// lock is held; a step that can fail after its commit, or that holds
    /// native locks, owes a [`Self::runless_terminal_announcement`] instead.
    #[allow(
        dead_code,
        reason = "the direct tell for a step that holds no native lock and cannot fail after its commit"
    )]
    pub(crate) fn hint_runless_input_terminal(&self) {
        let mut announcement = self.runless_terminal_announcement();
        announcement.owe();
    }

    /// An announcement that tells the installed [`RunlessInputTerminalHint`]
    /// once it is owed and drops.
    pub(crate) fn runless_terminal_announcement(&self) -> RunlessTerminalAnnouncement {
        RunlessTerminalAnnouncement {
            hint: self
                .runless_input_terminal_hint
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            owed: false,
        }
    }

    /// The interrupted-run evidence store installed in the session's
    /// canonical bindings, if any. Never settles anything.
    pub(crate) async fn bound_interrupted_tool_evidence(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::tool_process::InterruptedToolEvidence>> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).and_then(|entry| {
            entry
                .canonical_runtime_bindings
                .as_ref()
                .and_then(|bindings| bindings.interrupted_tool_evidence().get())
        })
    }

    /// The run whose provisional (uncommitted) tail the runtime store holds
    /// for `session_id`, if any.
    pub(crate) async fn provisional_tail_run(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<RunId>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(session_id);
        let store_error = |error: crate::store::RuntimeStoreError| {
            RuntimeDriverError::Internal(format!(
                "session {session_id} provisional tail lookup failed: {error}"
            ))
        };
        match store.session_persistence_profile() {
            crate::store::RuntimeSessionPersistenceProfile::WholeBlobV1 => Ok(store
                .load_whole_blob_provisional_tail(&runtime_id)
                .await
                .map_err(store_error)?
                .map(|tail| tail.authority().run_id().clone())),
            crate::store::RuntimeSessionPersistenceProfile::HeadCanonicalV1 => Ok(store
                .load_head_canonical_provisional_tail(&runtime_id)
                .await
                .map_err(store_error)?
                .map(|authority| authority.run_id().clone())),
        }
    }

    /// The session's interrupted-run evidence store: the one installed in the
    /// canonical session bindings by an agent build, else the one the host's
    /// evidence source settles and returns (installed into the canonical
    /// bindings so later builds share it).
    pub(crate) async fn session_interrupted_tool_evidence(
        &self,
        session_id: &SessionId,
    ) -> Result<
        Option<Arc<dyn meerkat_core::tool_process::InterruptedToolEvidence>>,
        RuntimeDriverError,
    > {
        let slot = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).and_then(|entry| {
                entry
                    .canonical_runtime_bindings
                    .as_ref()
                    .map(|bindings| Arc::clone(bindings.interrupted_tool_evidence()))
            })
        };
        if let Some(evidence) = slot.as_ref().and_then(|slot| slot.get()) {
            return Ok(Some(evidence));
        }
        let Some(source) = self.interrupted_tool_evidence_source() else {
            return Ok(None);
        };
        let evidence = source.settle_session(session_id).await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "session {session_id} could not settle its interrupted tool processes: {error}"
            ))
        })?;
        if let (Some(slot), Some(evidence)) = (slot.as_ref(), evidence.as_ref()) {
            slot.install(Arc::clone(evidence));
        }
        Ok(evidence)
    }

    /// Resolve the injected member live host, if composed.
    pub(crate) fn member_live_host(&self) -> Option<Arc<dyn crate::member_live::MemberLiveHost>> {
        self.member_live_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Install provider mechanics for generated canonical context delivery.
    #[cfg(feature = "live")]
    pub fn set_live_context_mirror_host(
        &self,
        host: Arc<dyn crate::live_context_mirror::LiveContextMirrorHost>,
    ) {
        *self
            .shared
            .live_context_mirror_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    }

    /// Install the session event publication every committed live channel
    /// close reports to (`AgentEvent::LiveChannelClosed`).
    #[cfg(feature = "live")]
    pub fn set_live_channel_close_publisher(
        &self,
        publisher: Arc<dyn crate::live_execution::LiveChannelCloseEventPublisher>,
    ) {
        *self
            .shared
            .live_channel_close_publisher
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(publisher);
    }

    /// Register a live channel close operation for as long as the returned
    /// guard lives. Every close path holds one from its first step to its
    /// last, whether it commits or fails, so another owner can tell a close
    /// that is executing from one that already ended.
    #[cfg(feature = "live")]
    pub fn begin_live_channel_close(
        &self,
        channel_id: &meerkat_core::LiveChannelId,
    ) -> LiveChannelCloseInFlightGuard {
        LiveChannelCloseInFlightGuard::begin(
            Arc::clone(&self.shared.live_channel_closes_in_flight),
            channel_id.as_str(),
        )
    }

    /// Whether a close of this channel is executing right now.
    #[cfg(feature = "live")]
    #[must_use]
    pub fn live_channel_close_in_flight(&self, channel_id: &meerkat_core::LiveChannelId) -> bool {
        self.shared
            .live_channel_closes_in_flight
            .contains(channel_id.as_str())
    }

    /// Completes when the next live channel close operation ends, on any
    /// channel. `notify_waiters` keeps no permit: create and `enable` the
    /// future before reading [`Self::live_channel_close_in_flight`], then
    /// await it while that reads `true`.
    #[cfg(feature = "live")]
    pub fn live_channel_close_ended(&self) -> tokio::sync::futures::Notified<'_> {
        self.shared.live_channel_closes_in_flight.ended.notified()
    }

    #[cfg(feature = "live")]
    pub(crate) fn live_channel_close_publisher(
        &self,
    ) -> Option<Arc<dyn crate::live_execution::LiveChannelCloseEventPublisher>> {
        self.shared
            .live_channel_close_publisher
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(feature = "live")]
    fn live_context_mirror_host(
        &self,
    ) -> Option<Arc<dyn crate::live_context_mirror::LiveContextMirrorHost>> {
        self.shared
            .live_context_mirror_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Acquire the session's live/lifecycle lease for a complete `live/open`
    /// materialization.  Every surface (RPC and mob member gateways) enters
    /// the same orchestration path and therefore the same machine-owned gate.
    #[cfg(feature = "live")]
    pub async fn acquire_live_open_lifecycle_lease(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<crate::member_live::MemberLiveLifecycleLease, RuntimeDriverError> {
        let (gate, guard) = self
            .lock_current_session_live_lifecycle_gate(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        Ok(crate::member_live::MemberLiveLifecycleLease::new(
            session_id.clone(),
            gate,
            guard,
        ))
    }

    /// Acquire the same lease as `live/open`, prove physical live-channel
    /// absence, and return it still held.  The lifecycle owner must retain the
    /// returned value through its durable retire/archive marker; dropping it
    /// earlier would reopen the original RPC-open race.
    #[cfg(feature = "live")]
    pub async fn acquire_member_live_disposal_lease(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<crate::member_live::MemberLiveLifecycleLease, crate::member_live::MemberLiveError>
    {
        let (gate, guard) = self
            .lock_current_session_live_lifecycle_gate(session_id)
            .await
            .ok_or_else(|| crate::member_live::MemberLiveError::Unavailable {
                reason: format!(
                    "member-live lifecycle gate is unavailable for session {session_id}"
                ),
            })?;
        let lease =
            crate::member_live::MemberLiveLifecycleLease::new(session_id.clone(), gate, guard);

        self.prove_member_live_absence_while_lease_held(session_id, &lease)
            .await?;
        Ok(lease)
    }

    /// Close any active channel while the caller retains this exact session's
    /// live/lifecycle lease. Exact construction rollback uses this split form:
    /// raw lease first, mutation gate second, identity revalidation third,
    /// physical absence proof last. That order cannot close a same-ID
    /// replacement before discovering that its epoch/claim changed.
    #[cfg(feature = "live")]
    async fn prove_member_live_absence_while_lease_held(
        &self,
        session_id: &meerkat_core::types::SessionId,
        lease: &crate::member_live::MemberLiveLifecycleLease,
    ) -> Result<(), crate::member_live::MemberLiveError> {
        if lease.session_id() != session_id {
            return Err(crate::member_live::MemberLiveError::Internal {
                reason: format!(
                    "member-live lifecycle lease for session {} cannot dispose session {session_id}",
                    lease.session_id()
                ),
            });
        }
        let lease_is_current = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .is_some_and(|entry| lease.matches_gate(&entry.live_lifecycle_gate))
        };
        if !lease_is_current {
            return Err(crate::member_live::MemberLiveError::Unavailable {
                reason: format!("member-live lifecycle lease is stale for session {session_id}"),
            });
        }

        let Some(channel_id) = self.live_active_channel_for_session(session_id).await else {
            return Ok(());
        };
        let Some(host) = self.member_live_host() else {
            return Err(crate::member_live::MemberLiveError::Unavailable {
                reason: format!(
                    "session {session_id} still owns live channel '{channel_id}', but no transport-neutral live cleanup host is composed"
                ),
            });
        };
        let timeout = crate::member_live::MEMBER_LIVE_DISPOSAL_CEILING;
        match crate::tokio::time::timeout(timeout, host.close(session_id, channel_id.as_str()))
            .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(crate::member_live::MemberLiveError::Unavailable {
                    reason: format!(
                        "member-live disposal close for channel '{channel_id}' timed out after {}ms for session {session_id}",
                        timeout.as_millis()
                    ),
                });
            }
        }
        if let Some(still_active) = self.live_active_channel_for_session(session_id).await {
            return Err(crate::member_live::MemberLiveError::Internal {
                reason: format!(
                    "member-live close returned success but machine channel '{still_active}' remains active for session {session_id}"
                ),
            });
        }
        Ok(())
    }

    /// Prove this session has no active member-live channel before an owning
    /// lifecycle tears down its runtime/session binding. The machine owns the
    /// injected host slot, so disposal does not receive or cache a second live
    /// gateway. A missing host is accepted only when generated authority has
    /// no active channel; active generated ownership without a cleanup host is
    /// a composition error, never negative transport evidence.
    ///
    /// Each host call is bounded independently. Any timeout or ambiguous host
    /// failure is returned to the caller, which must keep its lifecycle marker
    /// uncommitted and retry. Machine-owned absence or a successful exact close
    /// proves absence; an unexpected host `ChannelNotFound` remains a custody
    /// mismatch rather than being laundered into success.
    #[cfg(feature = "live")]
    pub async fn close_member_live_channel_for_disposal(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), crate::member_live::MemberLiveError> {
        let _lease = self.acquire_member_live_disposal_lease(session_id).await?;
        Ok(())
    }

    /// Record one live bridge command arriving at this member's drain
    /// (counted at host resolution, before any reach — absent-slot rejects
    /// count too: the command DID reach the member).
    pub(crate) fn note_live_command_served(&self) {
        self.live_commands_served
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Live bridge commands served by this member's drain arms (S3b).
    pub fn live_commands_served(&self) -> u64 {
        self.live_commands_served
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Resolve a session's tracked-turn journal, if registered. Public for
    /// member-host restart recovery: durable Pending rows must reattach the
    /// exact residency's journal without re-executing their input.
    pub fn tracked_turn_journal(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn crate::member_observation::TrackedTurnJournal>> {
        let slot = self
            .member_incarnation_slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()?;
        let state = slot
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            MemberResidencyState::Placed(registration) => registration.tracked_turn_journal.clone(),
            MemberResidencyState::PeerOnly { .. } | MemberResidencyState::VacantPlaced => None,
        }
    }

    fn validate_member_incarnation_registration(
        &self,
        session_id: &SessionId,
        incarnation: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Result<(), RuntimeDriverError> {
        if incarnation.mob_id.is_empty()
            || incarnation.agent_identity.is_empty()
            || incarnation.host_id.is_empty()
            || incarnation.member_session_id != session_id.to_string()
            || incarnation.binding_generation == 0
            || incarnation.fence_token == 0
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "member incarnation registration requires complete identities plus nonzero host-binding generation and fence for session '{session_id}': {incarnation:?}"
                ),
            });
        }
        Ok(())
    }

    /// Resolve the exact host-materialized incarnation for a resident
    /// session. `None` means either true PeerOnly or VacantPlaced; callers
    /// that need to authorize peer-only effects must use the residency lease,
    /// which distinguishes those states.
    pub fn member_incarnation(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation> {
        let slot = self
            .member_incarnation_slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()?;
        let state = slot
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            MemberResidencyState::Placed(registration) => Some(registration.incarnation.clone()),
            MemberResidencyState::PeerOnly { .. } | MemberResidencyState::VacantPlaced => None,
        }
    }

    /// Install or replay the exact V5 controller-member fence for a peer-only
    /// runtime. The Mob owns semantic generation/fence ordering; the runtime
    /// owns the opaque runtime/session token and binds it to the current epoch
    /// plus mutation-gate identity under the stable residency slot.
    pub(crate) async fn install_direct_member_incarnation(
        self: &Arc<Self>,
        session_id: &SessionId,
        requested: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
    ) -> Result<
        meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence,
        RuntimeDriverError,
    > {
        if requested.mob_id.is_empty()
            || requested.agent_identity.is_empty()
            || requested.fence_token == 0
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "direct member incarnation requires non-empty mob/agent identity and a nonzero fence token"
                    .to_string(),
            });
        }
        if self.store.is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "V5 direct member bind requires a durable semantic high-water store"
                    .to_string(),
            });
        }

        loop {
            let existing = self
                .pending_direct_member_bind_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(session_id)
                .map(|pending| {
                    (
                        pending.requested == *requested,
                        pending.completion_rx.clone(),
                    )
                });
            if let Some((same_request, mut completion_rx)) = existing {
                let outcome = crate::tokio::time::timeout(DIRECT_MEMBER_BIND_ACK_TIMEOUT, async {
                    loop {
                        if let Some(result) = completion_rx.borrow().clone() {
                            break result;
                        }
                        completion_rx.changed().await.map_err(|error| {
                            RuntimeDriverError::Internal(format!(
                                "direct member bind owner dropped completion: {error}"
                            ))
                        })?;
                    }
                })
                .await
                .map_err(|_| RuntimeDriverError::RecoveryBackoff {
                    reason: format!(
                        "direct member bind admission for {session_id} remains process-owned"
                    ),
                })?;
                if same_request {
                    return outcome;
                }
                continue;
            }

            let spawner = MachineCleanupTaskSpawner::acquire()?;
            let admission_id = uuid::Uuid::new_v4();
            let (completion_tx, completion_rx) = crate::tokio::sync::watch::channel(None);
            {
                let mut pending = self
                    .pending_direct_member_bind_admissions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if pending.contains_key(session_id) {
                    continue;
                }
                pending.insert(
                    session_id.clone(),
                    PendingDirectMemberBindAdmission {
                        admission_id,
                        requested: requested.clone(),
                        completion_rx: completion_rx.clone(),
                    },
                );
            }

            let machine = Arc::clone(self);
            let owned_session_id = session_id.clone();
            let owned_requested = requested.clone();
            spawner.spawn(async move {
                let result =
                    std::panic::AssertUnwindSafe(machine.install_direct_member_incarnation_owned(
                        &owned_session_id,
                        &owned_requested,
                    ))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|payload| {
                        Err(RuntimeDriverError::Internal(format!(
                            "direct member bind admission panicked: {}",
                            meerkat_core::panic_payload::panic_payload_detail(payload.as_ref())
                        )))
                    });
                completion_tx.send_replace(Some(result));
                let mut pending = machine
                    .pending_direct_member_bind_admissions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if pending
                    .get(&owned_session_id)
                    .is_some_and(|pending| pending.admission_id == admission_id)
                {
                    pending.remove(&owned_session_id);
                }
            });
            let mut completion_rx = completion_rx;
            return crate::tokio::time::timeout(DIRECT_MEMBER_BIND_ACK_TIMEOUT, async {
                loop {
                    if let Some(result) = completion_rx.borrow().clone() {
                        break result;
                    }
                    completion_rx.changed().await.map_err(|error| {
                        RuntimeDriverError::Internal(format!(
                            "direct member bind owner dropped completion: {error}"
                        ))
                    })?;
                }
            })
            .await
            .map_err(|_| RuntimeDriverError::RecoveryBackoff {
                reason: format!(
                    "direct member bind admission for {session_id} remains process-owned"
                ),
            })?;
        }
    }

    async fn install_direct_member_incarnation_owned(
        self: &Arc<Self>,
        session_id: &SessionId,
        requested: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
    ) -> Result<
        meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence,
        RuntimeDriverError,
    > {
        let slot = self.member_incarnation_slot(session_id);
        let slot_guard = Arc::clone(&slot.gate).lock_owned().await;
        let registration_guard = self.lock_session_registration_transaction(session_id).await;
        let (captured_runtime_epoch_id, captured_session_mutation_gate) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.epoch_id.clone(), entry.mutation_gate.clone())
        };

        let local_semantic_high_water = {
            let state = slot
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &*state {
                MemberResidencyState::PeerOnly { direct_member } => direct_member
                    .as_ref()
                    .map(|registration| registration.fence.incarnation()),
                MemberResidencyState::Placed(registration) => {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "direct member bind cannot replace placed residency {:?}",
                            registration.incarnation
                        ),
                    });
                }
                MemberResidencyState::VacantPlaced => {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: "direct member bind cannot claim a vacant placed residency"
                            .to_string(),
                    });
                }
            }
        };

        if let Some(current) = local_semantic_high_water.as_ref()
            && !crate::store::direct_member_high_water_accepts(current, requested)
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "direct member bind presented stale semantic incarnation {requested:?}; current is {current:?}"
                ),
            });
        }

        drop(registration_guard);
        drop(slot_guard);

        let store = Arc::clone(self.store.as_ref().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "V5 direct member bind lost its durable semantic high-water store".to_string(),
            )
        })?);
        let durable_current = store
            .admit_direct_member_incarnation_high_water(&session_id.to_string(), requested)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "direct member semantic high-water admission failed closed: {error}"
                ))
            })?;
        if &durable_current != requested {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "direct member bind presented stale semantic incarnation {requested:?}; durable current is {durable_current:?}"
                ),
            });
        }

        let _slot_guard = Arc::clone(&slot.gate).lock_owned().await;
        let _registration_guard = self.lock_session_registration_transaction(session_id).await;
        let (runtime_epoch_id, session_mutation_gate) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::StaleAuthority {
                    reason: "direct member runtime disappeared during durable admission"
                        .to_string(),
                })?;
            (entry.epoch_id.clone(), entry.mutation_gate.clone())
        };
        if runtime_epoch_id != captured_runtime_epoch_id
            || !session_mutation_gate.same_gate(&captured_session_mutation_gate)
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "direct member runtime was replaced during durable admission".to_string(),
            });
        }

        let mut state = slot
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let direct_member = match &mut *state {
            MemberResidencyState::PeerOnly { direct_member } => direct_member,
            MemberResidencyState::Placed(registration) => {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "direct member runtime became placed during durable admission: {:?}",
                        registration.incarnation
                    ),
                });
            }
            MemberResidencyState::VacantPlaced => {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "direct member runtime became vacant-placed during durable admission"
                        .to_string(),
                });
            }
        };

        if let Some(current) = direct_member.as_ref() {
            let current_runtime_is_exact = current.runtime_epoch_id == runtime_epoch_id
                && current
                    .session_mutation_gate
                    .same_gate(&session_mutation_gate);
            let current_semantic = current.fence.incarnation();
            if &current_semantic == requested {
                if current_runtime_is_exact {
                    return Ok(current.fence.clone());
                }
            } else if !crate::store::direct_member_high_water_accepts(&current_semantic, requested)
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "direct member bind presented stale semantic incarnation {requested:?}; current is {current_semantic:?}"
                    ),
                });
            }
        }

        let fence = meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence {
            mob_id: requested.mob_id.clone(),
            agent_identity: requested.agent_identity.clone(),
            generation: requested.generation,
            fence_token: requested.fence_token,
            member_session_id: session_id.to_string(),
            runtime_session_token:
                meerkat_contracts::wire::supervisor_bridge::BridgeDirectRuntimeSessionToken::new(),
        };
        *direct_member = Some(DirectMemberIncarnationRegistration {
            fence: fence.clone(),
            runtime_epoch_id,
            session_mutation_gate,
        });
        Ok(fence)
    }

    #[cfg(test)]
    pub(crate) fn test_member_residency_is_vacant(&self, session_id: &SessionId) -> bool {
        self.member_incarnation_slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .is_some_and(|slot| {
                matches!(
                    &*slot
                        .state
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    MemberResidencyState::VacantPlaced
                )
            })
    }

    fn member_incarnation_slot(&self, session_id: &SessionId) -> Arc<MemberResidencySlot> {
        if let Some(slot) = self
            .member_incarnation_slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
        {
            return slot;
        }
        Arc::clone(
            self.member_incarnation_slots
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(session_id.clone())
                .or_insert_with(|| Arc::new(MemberResidencySlot::peer_only())),
        )
    }

    /// Acquire the stable member-residency slot and the exact runtime-session
    /// gate installed with `expected`.  Holding both guards through the live
    /// effect makes validation and mutation one atomic authority interval:
    /// neither a G2 incarnation install nor a reused-session registration can
    /// overtake a G1 effect after its comparison.
    pub(crate) async fn lock_member_effect_authority(
        &self,
        session_id: &SessionId,
        expected: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Result<MemberEffectAuthorityGuard, RuntimeDriverError> {
        self.lock_optional_member_effect_authority(session_id, Some(expected))
            .await
    }

    /// Acquire the non-reentrant authority interval required by member-live
    /// operations. This deliberately does not take `mutation_gate`: the
    /// shared live pipeline owns lifecycle -> mutation ordering internally.
    pub(crate) async fn lock_member_live_authority(
        &self,
        session_id: &SessionId,
        expected: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Result<MemberLiveAuthorityGuard, RuntimeDriverError> {
        let lease = self
            .acquire_member_effect_authority_lease(session_id, Some(expected))
            .await?;
        let MemberEffectAuthorityLease {
            slot_guard,
            session_mutation_gate,
            ..
        } = lease;
        let registration_guard = self.lock_session_registration_transaction(session_id).await;
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member live/open expected incarnation {expected:?}; runtime session is absent"
                    ),
                });
            };
            if !entry.mutation_gate.same_gate(&session_mutation_gate) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member live/open expected incarnation {expected:?}; runtime session was replaced"
                    ),
                });
            }
            entry.require_durability_ready().map_err(|required| {
                RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: required.to_string(),
                }
            })?;
        }
        let state = self
            .existing_session_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);
        self.reject_unregistration_drain_ingress(session_id, state)
            .await?;
        Ok(MemberLiveAuthorityGuard {
            _slot_guard: slot_guard,
            _registration_guard: registration_guard,
        })
    }

    /// The peer-only sibling of [`Self::lock_member_effect_authority`]. A
    /// `None` expectation is still an authority claim: it holds the stable
    /// slot while proving that no host residency is registered, so a placed
    /// incarnation cannot appear between classification and effect.
    pub(crate) async fn lock_optional_member_effect_authority(
        &self,
        session_id: &SessionId,
        expected: Option<&meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation>,
    ) -> Result<MemberEffectAuthorityGuard, RuntimeDriverError> {
        let lease = self
            .acquire_member_effect_authority_lease(session_id, expected)
            .await?;
        let MemberEffectAuthorityLease {
            slot_guard,
            session_mutation_gate,
            ..
        } = lease;
        let session_guard = self
            .lock_live_session_mutation_gate(session_id, &session_mutation_gate)
            .await
            .map_err(degraded_gate_wait_refusal)?;
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member effect expected incarnation {expected:?}; runtime session is absent"
                    ),
                });
            };
            if !entry.mutation_gate.same_gate(&session_mutation_gate) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member effect expected incarnation {expected:?}; runtime session was replaced"
                    ),
                });
            }
            entry.require_durability_ready().map_err(|required| {
                RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: required.to_string(),
                }
            })?;
        }
        let state = self
            .existing_session_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);
        self.reject_unregistration_drain_ingress(session_id, state)
            .await?;
        Ok(MemberEffectAuthorityGuard {
            _slot_guard: slot_guard,
            _session_guard: session_guard,
        })
    }

    async fn acquire_member_effect_authority_lease(
        &self,
        session_id: &SessionId,
        expected: Option<&meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation>,
    ) -> Result<MemberEffectAuthorityLease, RuntimeDriverError> {
        let slot = self.member_incarnation_slot(session_id);
        let slot_guard = Arc::clone(&slot.gate).lock_owned().await;
        let registered_gate = {
            let state = slot
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match (expected, &*state) {
                (Some(expected), MemberResidencyState::Placed(registration))
                    if &registration.incarnation == expected =>
                {
                    Some(registration.session_mutation_gate.clone())
                }
                (Some(expected), MemberResidencyState::Placed(registration)) => {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "member effect expected incarnation {expected:?}; current is {:?}",
                            registration.incarnation
                        ),
                    });
                }
                (
                    Some(expected),
                    MemberResidencyState::PeerOnly { .. } | MemberResidencyState::VacantPlaced,
                ) => {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "member effect expected incarnation {expected:?}; current residency is absent"
                        ),
                    });
                }
                (None, MemberResidencyState::Placed(registration)) => {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "peer-only member effect expected no host residency; current is {:?}",
                            registration.incarnation
                        ),
                    });
                }
                (None, MemberResidencyState::VacantPlaced) => {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: "peer-only member effect cannot address a vacant placed residency"
                            .to_string(),
                    });
                }
                (None, MemberResidencyState::PeerOnly { .. }) => None,
            }
        };
        let registered_gate = match registered_gate {
            Some(gate) => gate,
            None => self.session_mutation_gate(session_id).await.ok_or(
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                },
            )?,
        };
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member effect expected incarnation {expected:?}; runtime session is absent"
                    ),
                });
            };
            if !entry.mutation_gate.same_gate(&registered_gate) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member effect expected incarnation {expected:?}; runtime session was replaced"
                    ),
                });
            }
        }
        Ok(MemberEffectAuthorityLease {
            slot,
            slot_guard,
            session_mutation_gate: registered_gate,
        })
    }

    /// Revalidate the residency half of a retained member-effect lease without
    /// reacquiring its slot gate. The caller still owns `lease.slot_guard`, so
    /// this read is both race-free and preserves the slot -> session lock order.
    fn validate_member_effect_authority_lease_current(
        &self,
        session_id: &SessionId,
        lease: &MemberEffectAuthorityLease,
        expected: Option<&meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation>,
    ) -> Result<(), RuntimeDriverError> {
        let slot_is_current = self
            .member_incarnation_slots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .is_some_and(|slot| Arc::ptr_eq(slot, &lease.slot));
        if !slot_is_current {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "member effect residency slot was replaced".to_string(),
            });
        }

        let state = lease
            .slot
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match (expected, &*state) {
            (Some(expected), MemberResidencyState::Placed(registration))
                if &registration.incarnation == expected
                    && registration
                        .session_mutation_gate
                        .same_gate(&lease.session_mutation_gate) =>
            {
                Ok(())
            }
            (Some(expected), MemberResidencyState::Placed(registration)) => {
                Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member effect expected incarnation {expected:?}; current is {:?}",
                        registration.incarnation
                    ),
                })
            }
            (
                Some(expected),
                MemberResidencyState::PeerOnly { .. } | MemberResidencyState::VacantPlaced,
            ) => Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "member effect expected incarnation {expected:?}; current residency is absent"
                ),
            }),
            (None, MemberResidencyState::PeerOnly { .. }) => Ok(()),
            (None, MemberResidencyState::Placed(registration)) => {
                Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "peer-only member effect expected no host residency; current is {:?}",
                        registration.incarnation
                    ),
                })
            }
            (None, MemberResidencyState::VacantPlaced) => Err(RuntimeDriverError::StaleAuthority {
                reason: "peer-only member effect cannot address a vacant placed residency"
                    .to_string(),
            }),
        }
    }

    /// Whether two wrappers retain the same actual machine and execution owner.
    /// Sharing only a persistence store does not satisfy this identity check.
    #[must_use]
    pub fn shares_runtime_execution_owner_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Whether both handles retain the same live runtime owner. A separately
    /// constructed machine is a different owner even over the same store.
    #[must_use]
    pub fn is_same_runtime_owner(&self, other: &Self) -> bool {
        self.shares_runtime_execution_owner_with(other)
    }

    /// This machine's runtime owner as a non-owning, opaque identity: it
    /// names the owner [`Self::is_same_runtime_owner`] compares, for any
    /// handle of it, and keeps none of it alive (no execution custody,
    /// session fact or store).
    #[must_use]
    pub fn runtime_owner(&self) -> RuntimeOwnerIdentity {
        RuntimeOwnerIdentity(Arc::downgrade(&self.shared))
    }

    /// Whether this adapter shares the same runtime persistence authority as
    /// another adapter. Runtime-backed composition surfaces use this to reject
    /// mismatched adapters before visible terminal events can outrun the store
    /// that owns their durable commit. Two distinct owners can share a store;
    /// use [`Self::is_same_runtime_owner`] to decide ownership.
    #[must_use]
    pub fn shares_runtime_persistence_with(&self, other: &Self) -> bool {
        match (&self.store, &other.store) {
            (None, None) => true,
            (Some(a), Some(b)) => runtime_stores_share_authority(a, b),
            _ => false,
        }
    }

    /// Whether this adapter owns the same runtime persistence authority as a
    /// concrete runtime store handle.
    #[must_use]
    pub fn shares_runtime_store_authority(&self, store: &Arc<dyn RuntimeStore>) -> bool {
        self.store
            .as_ref()
            .is_some_and(|machine_store| runtime_stores_share_authority(machine_store, store))
    }

    /// Whether this adapter has a runtime persistence store.
    #[must_use]
    pub fn has_runtime_persistence(&self) -> bool {
        self.store.is_some()
    }

    /// What a graceful process shutdown that names no disposition does with
    /// each session it unregisters, as `rkat-rpc`'s shutdown and the REST
    /// server's shutdown do. A runtime with a persistence store serves
    /// sessions that commit each round, so it requeues
    /// ([`UnregisterQueuedInputs::Requeue`] with
    /// [`UnregisterQueuedInputs::DEFAULT_MAX_AGE`]): eligible queued inputs
    /// stay queued for the next start, and the runtime's stop decides per run
    /// whether a run in flight continues there or ends as before. A runtime
    /// without one stops ([`UnregisterQueuedInputs::Stop`]), as every
    /// unregister did before. An unregister that names no disposition is
    /// unchanged: it still stops.
    #[must_use]
    pub fn graceful_shutdown_queued_inputs(&self) -> UnregisterQueuedInputs {
        if self.has_runtime_persistence() {
            UnregisterQueuedInputs::Requeue {
                max_age: UnregisterQueuedInputs::DEFAULT_MAX_AGE,
            }
        } else {
            UnregisterQueuedInputs::Stop
        }
    }

    fn normalize_destroyed_error(err: RuntimeDriverError) -> RuntimeDriverError {
        match err {
            RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            } => RuntimeDriverError::Destroyed,
            other => other,
        }
    }

    /// Create an ephemeral adapter (all sessions use EphemeralRuntimeDriver).
    pub fn ephemeral() -> Self {
        let auth_lease = Arc::new(crate::handles::RuntimeAuthLeaseHandle::new());
        #[cfg(not(target_arch = "wasm32"))]
        let oauth_flows = Arc::new(crate::handles::RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(10 * 60),
            Arc::clone(&auth_lease),
        ));
        let auth_lease = generated_runtime_auth_lease_handle(auth_lease);
        Self {
            shared: Arc::new(MeerkatMachineShared {
                native_work_authorization_host: Arc::new(std::sync::OnceLock::new()),
                consent_owner_loss_settlement: std::sync::OnceLock::new(),
                peer_ingress_binder: std::sync::OnceLock::new(),
                registration_use_sequence: std::sync::atomic::AtomicU64::new(0),
                hosting_owner: meerkat_core::session_hosting::HostingOwner::mint(),
                sessions: RwLock::new(HashMap::new()),
                #[cfg(not(target_arch = "wasm32"))]
                credential_release_observer: std::sync::OnceLock::new(),
                boundary_panic_log_gate: meerkat_core::panic_payload::PanicPayloadLogGate::default(
                ),
                attachment_commits: crate::tokio::sync::watch::Sender::new(0),
                registration_transaction_slots: StdRwLock::new(HashMap::new()),
                pending_runless_terminal_publications: StdMutex::new(HashMap::new()),
                pending_session_archive_lease_preparations: StdMutex::new(HashMap::new()),
                pending_predecessor_settlements: StdMutex::new(HashMap::new()),
                pending_direct_member_bind_admissions: StdMutex::new(HashMap::new()),
                store: None,
                execution_custody: None,
                blob_store: None,
                llm_reconfigure_host: StdRwLock::new(None),
                reconfigure_host_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                member_observation_host: StdRwLock::new(None),
                member_live_host: StdRwLock::new(None),
                interrupted_tool_evidence_source: StdRwLock::new(None),
                #[cfg(feature = "live")]
                live_context_mirror_host: StdRwLock::new(None),
                #[cfg(feature = "live")]
                live_channel_close_publisher: StdRwLock::new(None),
                #[cfg(feature = "live")]
                live_channel_closes_in_flight: Arc::default(),
                #[cfg(feature = "live")]
                live_context_queued_rows: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                post_close_result_titles: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_projection_gates: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_preparation_leases: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_drain_tasks: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_projection_tasks: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_assistant_output_by_turn: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_assistant_output_by_id: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_assistant_output_retirement_holds: StdMutex::new(HashMap::new()),
                live_commands_served: std::sync::atomic::AtomicU64::new(0),
                member_incarnation_slots: StdRwLock::new(HashMap::new()),
                auth_lease: StdRwLock::new(auth_lease),
                #[cfg(not(target_arch = "wasm32"))]
                oauth_flows: StdRwLock::new(oauth_flows),
                #[cfg(feature = "live")]
                live_unbound_rejection_authority: live_unbound_rejection_authority(),
                session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
                run_settlements: crate::tokio::sync::watch::Sender::new(0),
                runless_input_terminal_hint: StdRwLock::new(None),
                #[cfg(feature = "test-support")]
                test_stop_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_pause_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_reached: crate::tokio::sync::Notify::new(),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_release: crate::tokio::sync::Notify::new(),
                #[cfg(feature = "test-support")]
                test_unregister_saga_hold: StdMutex::new(None),
                #[cfg(all(feature = "live", any(test, feature = "test-support")))]
                test_live_custody_read_hold: StdMutex::new(None),
                #[cfg(feature = "test-support")]
                test_unregister_wait_witness: StdMutex::new(None),
                #[cfg(feature = "test-support")]
                test_unregister_saga_live_lease_signal: StdMutex::new(None),
                #[cfg(feature = "test-support")]
                test_unregister_caller_wait_grace: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_user_interrupt_ack_timeout: StdMutex::new(USER_INTERRUPT_ACK_TIMEOUT),
                #[cfg(test)]
                test_fenced_accept_after_lease: StdMutex::new(None),
                #[cfg(test)]
                test_accepted_boundary_work_hold: StdMutex::new(None),
                #[cfg(test)]
                test_durable_steer_relock_hold: StdMutex::new(None),
                #[cfg(test)]
                test_accepted_boundary_work_outcomes: StdMutex::new(None),
                #[cfg(test)]
                test_pending_attachment_before_regate: StdMutex::new(None),
                #[cfg(test)]
                test_registration_transaction_contention_probe: StdMutex::new(None),
                #[cfg(test)]
                test_fail_post_stop_unregister_after_fence: StdMutex::new(None),
                #[cfg(test)]
                test_fail_next_typed_dsl_post_commit_dispatch: std::sync::atomic::AtomicBool::new(
                    false,
                ),
                #[cfg(test)]
                test_fail_next_boundary_cancel_effect_projection:
                    std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                test_fail_next_boundary_cancel_dispatch_abort: std::sync::atomic::AtomicBool::new(
                    false,
                ),
                #[cfg(all(feature = "live", any(test, feature = "test-support")))]
                test_fail_next_live_close_settlement_deferral: std::sync::atomic::AtomicBool::new(
                    false,
                ),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_terminal_commit: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_boundary_acknowledgement: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_attachment_interrupted_notice_handoff: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_after_empty_interrupted_notice_check: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_run_start_holds: StdMutex::new(RunStartHoldsForTest::default()),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_queue_authority: StdMutex::new(None),
                registration_run_start_holds: std::sync::Mutex::new(HashMap::new()),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_executor_apply: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_batch_staging: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_scoped_binding_before_machine_authority: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_run_start_held_parks: crate::tokio::sync::watch::Sender::new(0),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_parked: crate::tokio::sync::watch::Sender::new(
                    std::collections::HashSet::new(),
                ),
                #[cfg(any(test, feature = "test-support"))]
                test_boundary_cancel_dispatches: crate::tokio::sync::watch::Sender::new(0),
                #[cfg(any(test, feature = "test-support"))]
                test_reload_required_discard_after_successor_publication: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_reload_required_discard_before_mutation_gate: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_mutation_gate_wait_observer: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_commit_busy_window: StdMutex::new(None),
                #[cfg(test)]
                test_degraded_gate_settlement_hand_offs: std::sync::atomic::AtomicUsize::new(0),
                pending_degraded_gate_settlements: StdMutex::new(HashSet::new()),
                #[cfg(test)]
                test_degraded_gate_settlement_spawns: std::sync::atomic::AtomicUsize::new(0),
                #[cfg(test)]
                test_control_command_after_logical_lookup: StdMutex::new(None),
                #[cfg(test)]
                test_before_finalized_unregister_removal: StdMutex::new(None),
            }),
        }
    }

    /// Create an ordinary persistent adapter with shared execution custody.
    ///
    /// Separate machines may partition one store across distinct runtime ids,
    /// but must not concurrently register the same session. Share this machine
    /// when composing multiple surfaces. Custody refusal returns before any
    /// provider-auth cache binding or persisted OAuth payload recovery.
    pub fn persistent(
        store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Result<Self, RuntimeDriverError> {
        Self::persistent_with_mode(store, Some(blob_store), false)
    }

    /// Create an ordinary persistent adapter without a blob store.
    ///
    /// Session state remains persistent. Blob-backed inputs fail explicitly at
    /// the blob-store boundary until a real [`BlobStore`] is supplied.
    pub fn persistent_without_blobs(
        store: Arc<dyn RuntimeStore>,
    ) -> Result<Self, RuntimeDriverError> {
        Self::persistent_with_mode(store, None, false)
    }

    fn persistent_with_mode(
        store: Arc<dyn RuntimeStore>,
        blob_store: Option<Arc<dyn BlobStore>>,
        governed: bool,
    ) -> Result<Self, RuntimeDriverError> {
        let custody = if governed {
            MachineCustody::Governed { associated: None }
        } else {
            MachineCustody::Shared
        };
        Self::persistent_with_custody(store, blob_store, custody)
    }

    /// [`Self::persistent_with_mode`] with the execution custody `custody`:
    /// a governed machine may retain an associated store's governed claim
    /// with its own ([`Self::persistent_governed_staged_with_associated_custody`]).
    fn persistent_with_custody(
        store: Arc<dyn RuntimeStore>,
        blob_store: Option<Arc<dyn BlobStore>>,
        custody: MachineCustody,
    ) -> Result<Self, RuntimeDriverError> {
        let execution_custody = match (store.execution_custody(), custody) {
            (Some(owner), MachineCustody::Governed { associated }) => {
                // An associated claim on this store itself (by any path)
                // would let the machine admit itself; it is refused.
                if associated
                    .as_ref()
                    .is_some_and(|associated| owner.binds_same_store(associated))
                {
                    return Err(execution_custody_error(
                        crate::store::RuntimeStoreExecutionCustodyError::Unsupported,
                    ));
                }
                let mut claim = owner
                    .try_acquire_governed()
                    .map_err(execution_custody_error)?;
                if let Some(associated) = associated {
                    claim
                        .retain_associated(associated)
                        .map_err(execution_custody_error)?;
                }
                Some(Arc::new(claim))
            }
            (Some(owner), MachineCustody::Shared) => Some(Arc::new(
                owner
                    .try_acquire_shared()
                    .map_err(execution_custody_error)?,
            )),
            (None, MachineCustody::Governed { .. }) => {
                return Err(execution_custody_error(
                    crate::store::RuntimeStoreExecutionCustodyError::Unsupported,
                ));
            }
            (None, MachineCustody::Shared) => None,
        };
        #[cfg(not(target_arch = "wasm32"))]
        let (auth_lease, oauth_flows) = {
            let authorities = persistent_auth_authorities(&store);
            let auth_lease = Arc::new(
                authorities
                    .auth_lease
                    .with_execution_custody(execution_custody.clone()),
            );
            let oauth_flows = Arc::new(
                authorities
                    .oauth_flows
                    .with_scoped_lifecycle(Arc::clone(&auth_lease)),
            );
            (auth_lease, oauth_flows)
        };
        #[cfg(target_arch = "wasm32")]
        let auth_lease = Arc::new(
            crate::handles::RuntimeAuthLeaseHandle::new()
                .with_execution_custody(execution_custody.clone()),
        );
        let auth_lease = generated_runtime_auth_lease_handle(auth_lease);
        Ok(Self {
            shared: Arc::new(MeerkatMachineShared {
                native_work_authorization_host: Arc::new(std::sync::OnceLock::new()),
                consent_owner_loss_settlement: std::sync::OnceLock::new(),
                peer_ingress_binder: std::sync::OnceLock::new(),
                registration_use_sequence: std::sync::atomic::AtomicU64::new(0),
                hosting_owner: meerkat_core::session_hosting::HostingOwner::mint(),
                sessions: RwLock::new(HashMap::new()),
                #[cfg(not(target_arch = "wasm32"))]
                credential_release_observer: std::sync::OnceLock::new(),
                boundary_panic_log_gate: meerkat_core::panic_payload::PanicPayloadLogGate::default(
                ),
                attachment_commits: crate::tokio::sync::watch::Sender::new(0),
                registration_transaction_slots: StdRwLock::new(HashMap::new()),
                pending_runless_terminal_publications: StdMutex::new(HashMap::new()),
                pending_session_archive_lease_preparations: StdMutex::new(HashMap::new()),
                pending_predecessor_settlements: StdMutex::new(HashMap::new()),
                pending_direct_member_bind_admissions: StdMutex::new(HashMap::new()),
                store: Some(store),
                execution_custody,
                blob_store: Some(blob_store.unwrap_or_else(|| Arc::new(UnavailableBlobStore))),
                llm_reconfigure_host: StdRwLock::new(None),
                reconfigure_host_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                member_observation_host: StdRwLock::new(None),
                member_live_host: StdRwLock::new(None),
                interrupted_tool_evidence_source: StdRwLock::new(None),
                #[cfg(feature = "live")]
                live_context_mirror_host: StdRwLock::new(None),
                #[cfg(feature = "live")]
                live_channel_close_publisher: StdRwLock::new(None),
                #[cfg(feature = "live")]
                live_channel_closes_in_flight: Arc::default(),
                #[cfg(feature = "live")]
                live_context_queued_rows: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                post_close_result_titles: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_projection_gates: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_preparation_leases: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_drain_tasks: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_context_projection_tasks: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_assistant_output_by_turn: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_assistant_output_by_id: StdMutex::new(HashMap::new()),
                #[cfg(feature = "live")]
                live_assistant_output_retirement_holds: StdMutex::new(HashMap::new()),
                live_commands_served: std::sync::atomic::AtomicU64::new(0),
                member_incarnation_slots: StdRwLock::new(HashMap::new()),
                auth_lease: StdRwLock::new(auth_lease),
                #[cfg(not(target_arch = "wasm32"))]
                oauth_flows: StdRwLock::new(oauth_flows),
                #[cfg(feature = "live")]
                live_unbound_rejection_authority: live_unbound_rejection_authority(),
                session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
                run_settlements: crate::tokio::sync::watch::Sender::new(0),
                runless_input_terminal_hint: StdRwLock::new(None),
                #[cfg(feature = "test-support")]
                test_stop_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_pause_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_reached: crate::tokio::sync::Notify::new(),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_release: crate::tokio::sync::Notify::new(),
                #[cfg(feature = "test-support")]
                test_unregister_saga_hold: StdMutex::new(None),
                #[cfg(all(feature = "live", any(test, feature = "test-support")))]
                test_live_custody_read_hold: StdMutex::new(None),
                #[cfg(feature = "test-support")]
                test_unregister_wait_witness: StdMutex::new(None),
                #[cfg(feature = "test-support")]
                test_unregister_saga_live_lease_signal: StdMutex::new(None),
                #[cfg(feature = "test-support")]
                test_unregister_caller_wait_grace: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_user_interrupt_ack_timeout: StdMutex::new(USER_INTERRUPT_ACK_TIMEOUT),
                #[cfg(test)]
                test_fenced_accept_after_lease: StdMutex::new(None),
                #[cfg(test)]
                test_accepted_boundary_work_hold: StdMutex::new(None),
                #[cfg(test)]
                test_durable_steer_relock_hold: StdMutex::new(None),
                #[cfg(test)]
                test_accepted_boundary_work_outcomes: StdMutex::new(None),
                #[cfg(test)]
                test_pending_attachment_before_regate: StdMutex::new(None),
                #[cfg(test)]
                test_registration_transaction_contention_probe: StdMutex::new(None),
                #[cfg(test)]
                test_fail_post_stop_unregister_after_fence: StdMutex::new(None),
                #[cfg(test)]
                test_fail_next_typed_dsl_post_commit_dispatch: std::sync::atomic::AtomicBool::new(
                    false,
                ),
                #[cfg(test)]
                test_fail_next_boundary_cancel_effect_projection:
                    std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                test_fail_next_boundary_cancel_dispatch_abort: std::sync::atomic::AtomicBool::new(
                    false,
                ),
                #[cfg(all(feature = "live", any(test, feature = "test-support")))]
                test_fail_next_live_close_settlement_deferral: std::sync::atomic::AtomicBool::new(
                    false,
                ),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_terminal_commit: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_boundary_acknowledgement: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_attachment_interrupted_notice_handoff: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_after_empty_interrupted_notice_check: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_run_start_holds: StdMutex::new(RunStartHoldsForTest::default()),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_queue_authority: StdMutex::new(None),
                registration_run_start_holds: std::sync::Mutex::new(HashMap::new()),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_executor_apply: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_batch_staging: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_scoped_binding_before_machine_authority: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_run_start_held_parks: crate::tokio::sync::watch::Sender::new(0),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_parked: crate::tokio::sync::watch::Sender::new(
                    std::collections::HashSet::new(),
                ),
                #[cfg(any(test, feature = "test-support"))]
                test_boundary_cancel_dispatches: crate::tokio::sync::watch::Sender::new(0),
                #[cfg(any(test, feature = "test-support"))]
                test_reload_required_discard_after_successor_publication: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_reload_required_discard_before_mutation_gate: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_mutation_gate_wait_observer: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_commit_busy_window: StdMutex::new(None),
                #[cfg(test)]
                test_degraded_gate_settlement_hand_offs: std::sync::atomic::AtomicUsize::new(0),
                pending_degraded_gate_settlements: StdMutex::new(HashSet::new()),
                #[cfg(test)]
                test_degraded_gate_settlement_spawns: std::sync::atomic::AtomicUsize::new(0),
                #[cfg(test)]
                test_control_command_after_logical_lookup: StdMutex::new(None),
                #[cfg(test)]
                test_before_finalized_unregister_removal: StdMutex::new(None),
            }),
        })
    }

    /// The realm's governed persistent machine, staged before its native
    /// host is installed (see [`StagedMeerkatMachine`]).
    ///
    /// It acquires the store's governed execution custody now, exactly as a
    /// governed open does, so a competing opener of the same realm is refused
    /// promptly by the existing custody rules for as long as the stage (or any
    /// handle sharing its custody) lives.
    pub fn persistent_governed_staged(
        store: Arc<dyn crate::store::RuntimeStore>,
        blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
    ) -> Result<StagedMeerkatMachine, RuntimeDriverError> {
        Self::persistent_with_mode(store, blob_store, true)
            .map(|machine| StagedMeerkatMachine { machine })
    }

    /// [`Self::persistent_governed_staged`] for a trusted host whose
    /// governed composition is bound to another actual store's files: the
    /// stage also retains `associated`, that store's governed claim (for
    /// example `SqliteRuntimeStore::try_acquire_governed_execution_custody`),
    /// together with its own store's claim. Every handle sharing the stage's
    /// custody shares both, and both are released only with that custody, so
    /// the associated store's other execution owners stay out for as long as
    /// this machine (or any handle sharing its custody) lives. Refused when
    /// this store's governed custody is held, when `associated` binds this
    /// same store (by any path) or is not a governed claim; `associated` is
    /// then released.
    pub fn persistent_governed_staged_with_associated_custody(
        store: Arc<dyn crate::store::RuntimeStore>,
        blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
        associated: crate::store::RuntimeStoreExecutionClaim,
    ) -> Result<StagedMeerkatMachine, RuntimeDriverError> {
        Self::persistent_with_custody(
            store,
            blob_store,
            MachineCustody::Governed {
                associated: Some(associated),
            },
        )
        .map(|machine| StagedMeerkatMachine { machine })
    }

    /// Shared auth lifecycle handle used by all runtime-backed session
    /// bindings created by this adapter.
    pub fn auth_lease_handle(&self) -> Arc<dyn meerkat_core::handles::AuthLeaseHandle> {
        self.generated_auth_lease_handle().clone_handle()
    }

    /// Generated-authority-certified auth lifecycle handle used at factory and
    /// resolver seams that must reject arbitrary handwritten handles.
    pub fn generated_auth_lease_handle(&self) -> meerkat_core::handles::GeneratedAuthLeaseHandle {
        self.auth_lease
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Return the atomically snapshotted provider-auth authority pair.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn provider_auth_runtime_authority(&self) -> ProviderAuthRuntimeAuthority {
        let auth_lease = self
            .auth_lease
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let oauth_flows = self
            .oauth_flows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ProviderAuthRuntimeAuthority {
            auth_lease: auth_lease.clone(),
            oauth_flows: Arc::clone(&oauth_flows),
        }
    }

    fn is_installed_credential_owner(
        &self,
        handle: &crate::handles::RuntimeAuthLeaseHandle,
    ) -> bool {
        (self.generated_auth_lease_handle().as_handle() as &dyn std::any::Any)
            .downcast_ref::<crate::handles::RuntimeAuthLeaseHandle>()
            .is_some_and(|installed| installed.shares_authority_with(handle))
    }

    /// Install the auth lifecycle authority that public surfaces also read.
    ///
    /// Storeless surfaces can align runtime traffic with their status handle.
    /// Persistent machines retain their construction-time scoped authority;
    /// requesting that same actual owner is a no-op and replacement is refused.
    pub fn set_auth_lease_handle(
        &self,
        handle: Arc<crate::handles::RuntimeAuthLeaseHandle>,
    ) -> Result<(), RuntimeDriverError> {
        self.set_runtime_auth_lease_handle(handle)
    }

    /// Install the runtime credential lifecycle handle together with an
    /// explicit OAuth login-flow authority.
    ///
    /// The credential side still has to be a generated AuthMachine authority;
    /// the explicit OAuth authority only controls login-flow test seams.
    /// Once native credential custody is installed, a same-credential request
    /// with a different OAuth owner is unsupported and leaves the pair unchanged.
    #[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-support")))]
    pub fn set_auth_lease_handle_with_oauth_flow_authority(
        &self,
        handle: Arc<crate::handles::RuntimeAuthLeaseHandle>,
        oauth_flows: Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority>,
    ) -> Result<(), RuntimeDriverError> {
        let mut installed = false;
        self.with_credential_authority_replacement(&handle, || {
            let mut auth_slot = self
                .auth_lease
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut oauth_slot = self
                .oauth_flows
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *oauth_slot = Arc::clone(&oauth_flows);
            *auth_slot = generated_runtime_auth_lease_handle(handle.clone());
            installed = true;
        })?;
        if !installed {
            // The credential helper's same-handle no-op does not install a
            // different OAuth owner. Only the exact existing pair is a no-op.
            let current = self.provider_auth_runtime_authority();
            if !self.is_installed_credential_owner(&handle)
                || !Arc::ptr_eq(&current.oauth_flow_authority(), &oauth_flows)
            {
                return Err(credential_custody::unavailable(
                    crate::traits::ControllerReadinessFailure::UnsupportedScope,
                ));
            }
        }
        Ok(())
    }

    /// Install a paired generated credential and OAuth authority on a storeless
    /// machine. Persistent replacement is unsupported; requesting the installed
    /// concrete owner is a no-op. An unfinished storeless governed controller
    /// prevents replacement, and temporary contention preserves the pair.
    pub fn set_runtime_auth_lease_handle(
        &self,
        handle: Arc<crate::handles::RuntimeAuthLeaseHandle>,
    ) -> Result<(), RuntimeDriverError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.with_credential_authority_replacement(&handle, || {
                let mut auth_slot = self
                    .auth_lease
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut oauth_slot = self
                    .oauth_flows
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *oauth_slot =
                    Arc::new(crate::handles::RuntimeOAuthFlowHandle::new_with_auth_lease(
                        std::time::Duration::from_secs(10 * 60),
                        handle.clone(),
                    ));
                *auth_slot = generated_runtime_auth_lease_handle(handle.clone());
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            if self.store.is_some() {
                return if self.is_installed_credential_owner(&handle) {
                    Ok(())
                } else {
                    Err(credential_custody::unavailable(
                        crate::traits::ControllerReadinessFailure::UnsupportedScope,
                    ))
                };
            }
            if self.native_work_authorization_host.get().is_some() {
                return Err(credential_custody::unavailable(
                    crate::traits::ControllerReadinessFailure::UnsupportedScope,
                ));
            }
            *self
                .auth_lease
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                generated_runtime_auth_lease_handle(handle);
            Ok(())
        }
    }

    /// Shared OAuth login-flow authority used by all auth surfaces that are
    /// backed by this runtime adapter.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn oauth_flow_authority(
        &self,
    ) -> Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority> {
        Arc::clone(
            &self
                .oauth_flows
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// The canonical session-identity claim handle owned by this
    /// `MeerkatMachine`. Comms runtimes wired through this machine acquire
    /// their session-id claim through it; the registry is scoped to this
    /// machine instance so tests / parallel runtimes do not collide.
    pub fn session_claim_handle(&self) -> Arc<dyn meerkat_core::handles::SessionClaimHandle> {
        Arc::clone(&self.session_claims) as Arc<dyn meerkat_core::handles::SessionClaimHandle>
    }

    /// Install an exact-registration signal endpoint for fault-injection tests.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::expect_used)]
    pub async fn set_session_composition_signal_dispatcher_for_test(
        &self,
        session_id: &SessionId,
        dispatcher: composition::MeerkatCompositionSignalDispatcher,
    ) {
        let _gate = self
            .lock_current_settled_session_mutation_gate(session_id, SettledGateWait::TestFixture)
            .await
            .expect("test signal endpoint requires an existing registration");
        self.sessions
            .write()
            .await
            .get_mut(session_id)
            .expect("test registration remains current under its mutation gate")
            .composition_signal_dispatcher = Some(dispatcher);
    }

    /// The exact registration's installed signal endpoint, for tests that
    /// route a signal to its consumer the way the runtime does.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn session_composition_signal_dispatcher_for_test(
        &self,
        session_id: &SessionId,
    ) -> Option<composition::MeerkatCompositionSignalDispatcher> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .and_then(|entry| entry.composition_signal_dispatcher.clone())
    }

    /// Apply a routed-input variant delivered by the `meerkat_mob_seam`
    /// composition dispatcher against the session's shared DSL authority.
    ///
    /// The caller is
    /// [`crate::meerkat_machine::composition::MeerkatConsumerSurface::apply_routed_input`];
    /// it has already projected producer fields into the typed
    /// [`dsl::MeerkatMachineInput`] shape. This method performs the
    /// session lookup + DSL-lock-scoped apply. A typed transition error
    /// from the kernel is surfaced as a `String` so the dispatcher can
    /// map it onto `DispatchRefusal::ConsumerRefused`.
    #[cfg(test)]
    pub(crate) async fn apply_routed_meerkat_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
    ) -> Result<(), dsl_authority::DslTransitionRefusal> {
        self.apply_routed_meerkat_input_with_signal_dispatcher(session_id, input, None)
            .await
    }

    /// Transfer delivery custody for an exact recovered member binding after
    /// its old composition actor has permanently closed. A cold unbound entry
    /// returns false and still requires an exclusive Prepared claim.
    pub async fn recover_composition_signal_dispatcher(
        &self,
        registration: &RuntimeSessionRegistrationWitness,
        runtime_id: dsl::AgentRuntimeId,
        fence: dsl::FenceToken,
        generation: Option<dsl::Generation>,
        dispatcher: composition::MeerkatCompositionSignalDispatcher,
    ) -> Result<bool, RuntimeDriverError> {
        let session_id = registration.session_id();
        let _guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let mut sessions = self.sessions.write().await;
        let entry =
            sessions
                .get_mut(session_id)
                .ok_or_else(|| RuntimeDriverError::StaleAuthority {
                    reason: "composition recovery registration disappeared".into(),
                })?;
        if !registration.belongs_to(self)
            || !registration.matches_entry(entry)
            || registration.epoch_id() != &entry.epoch_id
            || dispatcher.transport_is_closed() != Some(false)
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "composition recovery requires its current registration and open receiver"
                    .into(),
            });
        }
        if let Some(error) = entry.dsl_mutation_blocked_by_unregister(session_id) {
            return Err(error);
        }
        // Actor materialization takes the generated authority before its
        // process claim even outside M. Keep the same order here.
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let claim = entry
            .materialization_claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            claim.phase,
            crate::RuntimeActorMaterializationClaimPhase::Vacant
                | crate::RuntimeActorMaterializationClaimPhase::RetainedActor
        ) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "composition recovery cannot take an unfinished materialization claim"
                    .into(),
            });
        }
        let state = authority.state();
        if state.active_runtime_id.is_none()
            && state.active_fence_token.is_none()
            && state.active_runtime_generation.is_none()
        {
            if entry.physical_attachment_is_live()
                || entry.provisional_materialization_claim_id.is_some()
                || entry
                    .composition_signal_dispatcher
                    .as_ref()
                    .is_some_and(|endpoint| {
                        !Arc::ptr_eq(endpoint, &dispatcher)
                            && endpoint.transport_is_closed() != Some(true)
                    })
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "unbound composition recovery has existing physical custody".into(),
                });
            }
            return Ok(false);
        }
        if state.active_runtime_id.as_ref() != Some(&runtime_id)
            || state.active_fence_token != Some(fence)
            || state.active_runtime_generation != generation
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "composition recovery does not match the generated member binding".into(),
            });
        }
        if entry
            .composition_signal_dispatcher
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &dispatcher))
        {
            return Ok(true);
        }
        if entry
            .composition_signal_dispatcher
            .as_ref()
            .is_none_or(|current| current.transport_is_closed() != Some(true))
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "composition recovery requires proof the previous receiver is closed"
                    .into(),
            });
        }
        // The composition takes its custody back through the machine: a
        // recovered registration starts session-owned (residency is not
        // persisted), and the composition's claim of the exact recovered
        // binding marks it Composition-owned again, so an empty resume keeps
        // the recovered member serving.
        dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            dsl::MeerkatMachineInput::ClaimCompositionResidency {
                session_id: dsl::SessionId::from_domain(session_id),
                agent_runtime_id: runtime_id,
                fence_token: fence,
                generation,
            },
        )
        .map_err(|error| RuntimeDriverError::StaleAuthority {
            reason: format!(
                "composition recovery could not claim the recovered registration's residency: {}",
                dsl_authority::map_error(error, "ClaimCompositionResidency")
            ),
        })?;
        drop(authority);
        drop(claim);
        entry.composition_signal_dispatcher = Some(dispatcher);
        entry.composition_materialization_claim_id = None;
        Ok(true)
    }

    /// The operation registry of `session_id`'s registration when that
    /// registration already serves exactly one member binding, read without
    /// waiting on anything a run holds.
    ///
    /// `Some` only while the full binding restore (local session bindings,
    /// then [`Self::recover_composition_signal_dispatcher`]) would leave the
    /// registration as it is: an executor attached and live under an Active
    /// registration that is durability ready; no unregister, stop cleanup or
    /// predecessor settlement; no actor materialization claim in flight;
    /// runtime bindings already issued; MeerkatMachine's active runtime
    /// binding exactly `runtime_id`, `fence` and `generation`; and, when the
    /// caller delivers composition signals, `dispatcher` already its open
    /// composition endpoint. The attachment that adopted the registration
    /// stamped its use, so a pending unregister of an earlier use cannot take
    /// it over either.
    ///
    /// Read under the session map's read lock and the entry's synchronous
    /// locks alone: it never takes the session mutation gate, the driver or
    /// the registration transaction, so a run in flight (whose finalization
    /// holds the gate) never holds it up. `None` otherwise; the caller then
    /// takes the full restore.
    pub async fn serving_member_binding_ops_registry(
        &self,
        session_id: &SessionId,
        runtime_id: &dsl::AgentRuntimeId,
        fence: dsl::FenceToken,
        generation: Option<dsl::Generation>,
        dispatcher: Option<&composition::MeerkatCompositionSignalDispatcher>,
    ) -> Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        if entry.require_durability_ready().is_err()
            || entry
                .registration_blocked_by_unregister(session_id)
                .is_some()
            || !entry.generated_executor_registration_active()
            || !entry.attachment_is_live()
            || entry.canonical_runtime_bindings.is_none()
        {
            return None;
        }
        {
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            if state.active_runtime_id.as_ref() != Some(runtime_id)
                || state.active_fence_token != Some(fence)
                || state.active_runtime_generation != generation
            {
                return None;
            }
        }
        {
            let claim = entry
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !matches!(
                claim.phase,
                crate::RuntimeActorMaterializationClaimPhase::Vacant
                    | crate::RuntimeActorMaterializationClaimPhase::RetainedActor
            ) {
                return None;
            }
        }
        if let Some(dispatcher) = dispatcher {
            let current = entry.composition_signal_dispatcher.as_ref()?;
            if !Arc::ptr_eq(current, dispatcher) || dispatcher.transport_is_closed() != Some(false)
            {
                return None;
            }
        }
        Some(Arc::clone(&entry.ops_lifecycle))
    }

    pub(crate) async fn apply_routed_meerkat_input_with_signal_dispatcher(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        signal_dispatcher: Option<&composition::MeerkatCompositionSignalDispatcher>,
    ) -> Result<(), dsl_authority::DslTransitionRefusal> {
        let registration = self
            .current_session_registration_witness(session_id)
            .await
            .ok_or_else(|| {
                dsl_authority::DslTransitionRefusal::other(
                    "routed_session_not_durability_ready",
                    format!(
                        "session `{session_id}` cannot accept routed input until its persistent runtime is cold reloaded: {}",
                        RuntimeDriverError::NotReady { state: RuntimeState::Destroyed }
                    ),
                )
            })?;
        let _gate_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await
            .map_err(|error| {
                dsl_authority::DslTransitionRefusal::other(
                    "routed_session_not_durability_ready",
                    format!(
                        "session `{session_id}` cannot accept routed input until its persistent \
                         runtime is cold reloaded: {error}"
                    ),
                )
            })?;
        self.apply_routed_session_dsl_input(
            session_id,
            input,
            &registration,
            signal_dispatcher,
            "RoutedMeerkatInput",
        )
        .await
        .map(|_| ())
    }

    #[cfg(test)]
    pub(crate) async fn debug_shared_ingress_authorities(
        &self,
        session_id: &SessionId,
    ) -> Option<(
        Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        crate::driver::ephemeral::SharedIngressDslAuthority,
    )> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        let session_authority = Arc::clone(&entry.dsl_authority);
        let driver = entry.driver.lock().await;
        Some((session_authority, driver.shared_dsl_authority()))
    }

    #[cfg(test)]
    pub(crate) fn pause_before_finalized_unregister_removal(
        &self,
        session_id: SessionId,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        *self
            .test_before_finalized_unregister_removal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((session_id, entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    #[cfg(test)]
    async fn run_before_finalized_unregister_removal_test_hook(&self, session_id: &SessionId) {
        let gate = {
            let mut slot = self
                .test_before_finalized_unregister_removal
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot
                .as_ref()
                .is_some_and(|(armed_session_id, _, _)| armed_session_id == session_id)
            {
                slot.take()
            } else {
                None
            }
        };
        if let Some((_, entered_tx, release_rx)) = gate {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }

    /// Create a driver entry for a session.
    fn make_driver(
        &self,
        runtime_id: LogicalRuntimeId,
        dsl_authority: crate::driver::ephemeral::SharedIngressDslAuthority,
        initial_runtime_state: RuntimeState,
        durability_health: Option<DurabilityHealthHandle>,
    ) -> Result<DriverEntry, RuntimeDriverError> {
        let control_projection = Arc::new(StdRwLock::new(
            crate::driver::ephemeral::RuntimeControlProjection {
                phase: initial_runtime_state,
                current_run_id: None,
                pre_run_phase: None,
            },
        ));
        match (&self.store, &self.blob_store) {
            (Some(store), Some(blob_store)) => {
                let durability_health =
                    durability_health.ok_or_else(|| RuntimeDriverError::Internal(
                        "persistent runtime driver construction requires the registration cold-install durability handle"
                            .to_string(),
                    ))?;
                let mut driver =
                    PersistentRuntimeDriver::new_with_control_health_and_execution_custody(
                        runtime_id,
                        store.clone(),
                        blob_store.clone(),
                        control_projection,
                        dsl_authority,
                        durability_health,
                        Ok(self.execution_custody.clone()),
                    );
                driver
                    .inner_mut()
                    .set_work_authorization_host(Arc::clone(&self.native_work_authorization_host));
                Ok(DriverEntry::Persistent(driver))
            }
            _ if durability_health.is_some() => Err(RuntimeDriverError::Internal(
                "storeless runtime driver construction received a persistent durability handle"
                    .to_string(),
            )),
            _ => {
                let mut driver = EphemeralRuntimeDriver::new_with_control_and_dsl(
                    runtime_id,
                    control_projection,
                    dsl_authority,
                );
                driver
                    .set_work_authorization_host(Arc::clone(&self.native_work_authorization_host));
                Ok(DriverEntry::Ephemeral(driver))
            }
        }
    }

    /// Recover or create fresh ops lifecycle state for a session.
    ///
    /// This is the single canonical recovery seam. Both `register_session()`
    /// and `ensure_session_with_executor()`'s cold path call this to create
    /// epoch-local state. If a durable store is available, attempts to load
    /// the persisted snapshot; otherwise creates fresh state with a new epoch.
    async fn recover_or_create_ops_state(
        &self,
        session_id: &SessionId,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        (
            Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
            meerkat_core::RuntimeEpochId,
            Arc<meerkat_core::EpochCursorState>,
        ),
        RuntimeDriverError,
    > {
        if let Some(ref store) = self.store {
            let (registry, epoch_id, cursor_state) = Self::fresh_ops_state();
            // The epoch is an owner witness embedded in durable input/outbox
            // records. Atomically initialize its empty authority image before
            // bindings can escape. A competing cold registrar may win this
            // boundary; in that case we recover the winner's canonical image
            // rather than publishing two epochs for one logical runtime.
            let initial_snapshot = registry
                .capture_persistence_snapshot(epoch_id.clone(), cursor_state.as_ref())
                .map_err(|error| {
                    RuntimeDriverError::Internal(format!(
                        "failed to capture initial ops lifecycle authority for session {session_id}: {error}"
                    ))
                })?;
            let canonical_snapshot = store
                .initialize_ops_lifecycle_if_absent(runtime_id, &initial_snapshot)
                .await
                .map_err(|error| {
                    RuntimeDriverError::Internal(format!(
                        "failed to initialize ops lifecycle authority for session {session_id}: {error}"
                    ))
                })?;
            let recovered_epoch = canonical_snapshot.epoch_id.clone();
            let initialized = recovered_epoch == epoch_id;
            let recovered_ops_count = canonical_snapshot.completion_entries.len();
            let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::from_recovered(
                canonical_snapshot,
            )
            .map_err(|error| {
                tracing::error!(
                    %session_id,
                    %runtime_id,
                    error = %error,
                    "failed to recover ops lifecycle through generated authority"
                );
                RuntimeDriverError::Internal(format!(
                    "failed to recover ops lifecycle through generated authority: {error}"
                ))
            })?;
            let recovered_cursor_snapshot = registry.completion_cursor_snapshot();
            let recovered_cursors = meerkat_core::EpochCursorState::from_recovered(
                recovered_cursor_snapshot.agent_applied_cursor,
                recovered_cursor_snapshot.runtime_observed_seq,
                recovered_cursor_snapshot.runtime_last_injected_seq,
            );
            tracing::info!(
                %session_id,
                %runtime_id,
                epoch_id = %recovered_epoch,
                recovered_ops = recovered_ops_count,
                initialized,
                "ops lifecycle authority selected from durable store"
            );
            Ok((
                Arc::new(registry),
                recovered_epoch,
                Arc::new(recovered_cursors),
            ))
        } else {
            Ok((
                Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                meerkat_core::RuntimeEpochId::new(),
                Arc::new(meerkat_core::EpochCursorState::new()),
            ))
        }
    }

    fn fresh_ops_state() -> (
        Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
        meerkat_core::RuntimeEpochId,
        Arc<meerkat_core::EpochCursorState>,
    ) {
        let registry = Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new());
        let epoch = meerkat_core::RuntimeEpochId::new();
        let cursors = Arc::new(meerkat_core::EpochCursorState::new());
        (registry, epoch, cursors)
    }

    #[allow(clippy::large_futures)]
    fn execute_meerkat_machine_command(
        &self,
        self_handle: Option<Arc<Self>>,
        command: MeerkatMachineCommand,
    ) -> MeerkatMachineCommandFuture<'_> {
        // Each arm builds its dispatch future in its own frame and returns it
        // boxed. One `async` block over the whole match reserved every arm's
        // awaited dispatch future in a single poll frame (350,784 bytes at
        // opt-level 0), beneath every machine command.
        match command {
            MeerkatMachineCommand::EnsureSessionWithExecutor { .. } => {
                crate::stack_relief::box_in_own_frame(|| async move {
                    let self_handle = self_handle.ok_or_else(|| {
                        MeerkatMachineCommandError::Driver(RuntimeDriverError::Internal(
                            "EnsureSessionWithExecutor requires Arc<Self> machine handle".into(),
                        ))
                    })?;
                    self_handle
                        .execute_meerkat_machine_ensure_session_command(command)
                        .await
                        .map_err(Into::into)
                })
            }
            MeerkatMachineCommand::RegisterSession { .. }
            | MeerkatMachineCommand::UnregisterSession { .. }
            | MeerkatMachineCommand::SetSilentIntents { .. }
            | MeerkatMachineCommand::CancelAfterBoundary { .. }
            | MeerkatMachineCommand::StopRuntimeExecutor { .. }
            | MeerkatMachineCommand::StopRuntimeExecutorUntilTerminal { .. }
            | MeerkatMachineCommand::ContainsSession { .. }
            | MeerkatMachineCommand::SessionHasExecutor { .. }
            | MeerkatMachineCommand::SessionHasComms { .. }
            | MeerkatMachineCommand::OpsLifecycleRegistry { .. }
            | MeerkatMachineCommand::PrepareBindings { .. }
            | MeerkatMachineCommand::PrepareLocalSessionBindings { .. }
            | MeerkatMachineCommand::InputState { .. }
            | MeerkatMachineCommand::InputStateByIdempotencyKey { .. }
            | MeerkatMachineCommand::DurableInputStateByIdempotencyKey { .. }
            | MeerkatMachineCommand::InteractionTerminalStatus { .. }
            | MeerkatMachineCommand::RunTerminalStatus { .. }
            | MeerkatMachineCommand::ListActiveInputs { .. }
            | MeerkatMachineCommand::ReconfigureSessionLlmIdentity { .. }
            | MeerkatMachineCommand::StagePersistentFilter { .. }
            | MeerkatMachineCommand::RequestDeferredTools { .. }
            | MeerkatMachineCommand::PublishCommittedVisibleSet { .. } => {
                crate::stack_relief::box_in_own_frame(|| async move {
                    self.execute_meerkat_machine_session_command(command)
                        .await
                        .map_err(Into::into)
                })
            }
            MeerkatMachineCommand::SetPeerIngressContext { .. }
            | MeerkatMachineCommand::NotifyDrainExited { .. } => {
                crate::stack_relief::box_in_own_frame(|| async move {
                    let self_handle = self_handle.ok_or_else(|| {
                        MeerkatMachineCommandError::Driver(RuntimeDriverError::Internal(
                            "drain command requires Arc<Self> machine handle".into(),
                        ))
                    })?;
                    self_handle
                        .execute_meerkat_machine_drain_command(command)
                        .await
                        .map_err(Into::into)
                })
            }
            MeerkatMachineCommand::AbortAll
            | MeerkatMachineCommand::Abort { .. }
            | MeerkatMachineCommand::Wait { .. } => {
                crate::stack_relief::box_in_own_frame(|| async move {
                    self.execute_meerkat_machine_drain_local_command(command)
                        .await
                        .map_err(Into::into)
                })
            }
            MeerkatMachineCommand::Ingest { .. }
            | MeerkatMachineCommand::PublishEvent { .. }
            | MeerkatMachineCommand::Retire { .. }
            | MeerkatMachineCommand::Recycle { .. }
            | MeerkatMachineCommand::Reset { .. }
            | MeerkatMachineCommand::Recover { .. }
            | MeerkatMachineCommand::Destroy { .. }
            | MeerkatMachineCommand::RuntimeState { .. }
            | MeerkatMachineCommand::ResolvedSessionLlmCapabilities { .. }
            | MeerkatMachineCommand::ConfigureModelRoutingBaseline { .. }
            | MeerkatMachineCommand::SessionModelRoutingStatus { .. }
            | MeerkatMachineCommand::RequestSwitchTurn { .. }
            | MeerkatMachineCommand::RealizeCommittedModelRoutingHandoff { .. }
            | MeerkatMachineCommand::AdmitModelRoutingAssistantTurn { .. }
            | MeerkatMachineCommand::BeginImageOperation { .. }
            | MeerkatMachineCommand::DenyImageOperationPlan { .. }
            | MeerkatMachineCommand::ActivateImageOperationOverride { .. }
            | MeerkatMachineCommand::ClassifyImageOperationTerminal { .. }
            | MeerkatMachineCommand::CompleteImageOperation { .. }
            | MeerkatMachineCommand::RestoreImageOperationOverride { .. }
            | MeerkatMachineCommand::LoadBoundaryReceipt { .. } => {
                crate::stack_relief::box_in_own_frame(|| async move {
                    self.execute_meerkat_machine_control_command(command)
                        .await
                        .map_err(Into::into)
                })
            }
            MeerkatMachineCommand::AcceptWithCompletion { .. }
            | MeerkatMachineCommand::AcceptWithoutWake { .. } => {
                crate::stack_relief::box_in_own_frame(|| async move {
                    self.execute_meerkat_machine_ingress_command(command)
                        .await
                        .map_err(Into::into)
                })
            }
        }
    }

    /// Register a runtime driver for a session (no RuntimeLoop — inputs queue but
    /// nothing processes them automatically). Useful for tests and legacy mode.
    ///
    /// Registration is a control-plane prerequisite: a failed register must not be
    /// laundered to success. The inner command can fail recovery, so the typed
    /// error is propagated to the caller rather than discarded.
    pub async fn register_session(
        &self,
        session_id: SessionId,
    ) -> Result<(), RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RegisterSession { session_id },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "register_session: unexpected command result variant: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#[path = "../meerkat_machine_tests.rs"]
mod tests;

#[cfg(test)]
mod durable_steer_tests;

#[cfg(test)]
mod terminal_receipt_tests;

#[cfg(test)]
mod input_transition_wait_tests;

#[cfg(all(test, feature = "live"))]
mod live_custody_read_tests;

#[cfg(test)]
mod live_channel_active_read_tests;

#[cfg(test)]
mod registration_use_tests;

#[cfg(test)]
mod durability_gate_wait_tests;

#[cfg(test)]
mod commit_deadline_tests;

#[cfg(test)]
mod lost_admission_ack_tests;

#[cfg(all(test, feature = "live", feature = "test-support"))]
mod live_bridge_original_work_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod oauth_pair_tests;

impl Drop for MeerkatMachineShared {
    /// The owner of every registration is gone: no holder that outlives it
    /// (an exported configured-attachment authority) may still observe one of
    /// its agent bindings as current.
    fn drop(&mut self) {
        for entry in self.sessions.get_mut().values() {
            entry.agent_binding.retire();
            entry.tool_surface_gate.retire();
            entry.mcp_observation.retire();
        }
    }
}

impl MeerkatMachineShared {
    /// A fresh [`RuntimeSessionRegistrationUse`] stamp value: the next value
    /// of the machine-wide sequence.
    fn next_registration_use(&self) -> u64 {
        self.registration_use_sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    fn require_governed_execution_custody(&self) -> Result<(), RuntimeDriverError> {
        if self.store.is_none()
            || self
                .execution_custody
                .as_ref()
                .is_some_and(|claim| claim.is_governed())
        {
            Ok(())
        } else {
            Err(execution_custody_error(
                crate::store::RuntimeStoreExecutionCustodyError::Unsupported,
            ))
        }
    }
}

pub(crate) fn execution_custody_error(
    error: crate::store::RuntimeStoreExecutionCustodyError,
) -> RuntimeDriverError {
    use crate::store::RuntimeStoreExecutionCustodyError;
    let reason = match error {
        RuntimeStoreExecutionCustodyError::Busy => crate::traits::ControllerReadinessFailure::Busy,
        RuntimeStoreExecutionCustodyError::Unsupported => {
            crate::traits::ControllerReadinessFailure::UnsupportedScope
        }
        RuntimeStoreExecutionCustodyError::Unavailable => {
            crate::traits::ControllerReadinessFailure::AuthorityUnavailable
        }
    };
    RuntimeDriverError::ControllerReadinessUnavailable { reason }
}

/// Close operations executing per live channel (a count: close paths nest).
#[cfg(feature = "live")]
#[derive(Default)]
pub(crate) struct LiveChannelClosesInFlight {
    by_channel: StdMutex<HashMap<String, usize>>,
    ended: tokio::sync::Notify,
}

#[cfg(feature = "live")]
impl LiveChannelClosesInFlight {
    fn contains(&self, channel_id: &str) -> bool {
        self.by_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(channel_id)
    }
}

/// Holds one live channel close operation in flight; dropping it ends the
/// operation and wakes [`MeerkatMachine::live_channel_close_ended`] waiters.
#[cfg(feature = "live")]
#[must_use = "the close is in flight only while the guard lives"]
pub struct LiveChannelCloseInFlightGuard {
    registry: Arc<LiveChannelClosesInFlight>,
    channel_id: String,
}

#[cfg(feature = "live")]
impl LiveChannelCloseInFlightGuard {
    fn begin(registry: Arc<LiveChannelClosesInFlight>, channel_id: &str) -> Self {
        *registry
            .by_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(channel_id.to_string())
            .or_default() += 1;
        Self {
            registry,
            channel_id: channel_id.to_string(),
        }
    }
}

#[cfg(feature = "live")]
impl Drop for LiveChannelCloseInFlightGuard {
    fn drop(&mut self) {
        {
            let mut by_channel = self
                .registry
                .by_channel
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(count) = by_channel.get_mut(&self.channel_id) {
                *count -= 1;
                if *count == 0 {
                    by_channel.remove(&self.channel_id);
                }
            }
        }
        self.registry.ended.notify_waiters();
    }
}
