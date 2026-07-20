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
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex as StdMutex, OnceLock, Weak};

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

#[allow(clippy::expect_used)]
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
    let mut authority = dsl_authority::new_registered_authority(session_id)
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
#[derive(Debug, thiserror::Error)]
pub enum RuntimeBindingsError {
    /// Session was not found after registration (should not happen in practice).
    #[error("session {0} not found in runtime adapter after registration")]
    SessionNotFound(SessionId),
    /// Machine-owned binding preparation failed before bindings were published.
    #[error("failed to prepare runtime bindings for session {0}: {1}")]
    PrepareFailed(SessionId, String),
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
    let input_key = input_id.to_string();
    let (terminal_kind, abandon_reason) = input_terminality_parts(seed.terminal_outcome.as_ref());
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ResolveInputPublicTerminalOutcome {
            input_id: input_key.clone(),
            phase: observed_input_phase(seed.phase),
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

#[cfg(not(target_arch = "wasm32"))]
static PERSISTENT_AUTH_AUTHORITIES: OnceLock<
    StdMutex<HashMap<PersistentAuthAuthorityKey, Arc<PersistentAuthAuthorityBundle>>>,
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

#[cfg(not(target_arch = "wasm32"))]
fn persistent_auth_authorities(
    store: &Arc<dyn RuntimeStore>,
) -> Arc<PersistentAuthAuthorityBundle> {
    let key = runtime_store_identity(store);
    let authorities = PERSISTENT_AUTH_AUTHORITIES.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut authorities = authorities
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = authorities.get(&key) {
        let stored_store_alive = existing
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
            .is_some();
        if matches!(key, PersistentAuthAuthorityKey::Durable(_)) || stored_store_alive {
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
    let bundle = Arc::new(PersistentAuthAuthorityBundle {
        store: StdMutex::new(Arc::downgrade(store)),
        auth_lease,
        oauth_flows,
    });
    authorities.insert(key, Arc::clone(&bundle));
    bundle
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn clear_persistent_auth_authorities_for_test() {
    if let Some(authorities) = PERSISTENT_AUTH_AUTHORITIES.get() {
        authorities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
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
    DriverEntry, SharedCompletionRegistry, SharedDriver, cancel_runtime_loop_run,
    commit_machine_terminal_run, commit_runtime_loop_run, fail_runtime_loop_run,
    machine_authorize_runtime_loop_batch, machine_batch_primitive_projections,
    machine_batch_runtime_semantics, machine_commit_prepared_destroy,
    machine_commit_service_turn_terminal_receipt, machine_prepare_bindings_projection,
    machine_prepare_destroy, machine_recover_ephemeral_driver, machine_recover_persistent_driver,
    machine_recover_persistent_inputs, machine_recycle_preserving_work, machine_reset,
    machine_retire, machine_stop_runtime, prepare_runtime_loop_batch_start,
};

pub(crate) mod driver;

mod comms_drain;
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
mod llm_reconfigure;
mod runtime_control;
mod session_management;
mod traits;
mod visibility;

pub(crate) use session_management::{
    DeleteOpsFinalizationAuthority, RetainOpsFinalizationAuthority,
};

pub use composition::{MeerkatCompositionSignalDispatcher, MeerkatConsumerSurface};

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
}

#[derive(Clone, Copy)]
enum CommittedEffectDispatchFailure {
    PreserveCommittedDslState,
}

type UnregisterTeardownResult = Result<(), RuntimeDriverError>;
type RuntimeStopCleanupResult = Result<(), RuntimeDriverError>;

/// Joinable result channel for the one owned unregister saga of an epoch.
#[derive(Clone)]
struct UnregisterTeardownCoordinator {
    epoch_id: meerkat_core::RuntimeEpochId,
    coordinator_id: uuid::Uuid,
    result_rx: crate::tokio::sync::watch::Receiver<Option<UnregisterTeardownResult>>,
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

#[derive(Clone)]
struct PendingUnregisterFinalization {
    finalization_id: uuid::Uuid,
    durability_authority: session_management::RuntimeOpsLifecycleDurabilityAuthority,
    committed_snapshot: dsl::MeerkatMachineAuthoritySnapshot,
}

struct UnregisterTeardownMechanicalObservations {
    runtime_loop_forced_abort: std::sync::atomic::AtomicBool,
    comms_drain_forced_abort: std::sync::atomic::AtomicBool,
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

/// Per-session state: driver + generated authority binding + shell handles.
struct RuntimeSessionEntry {
    /// Canonical runtime control-plane identity for this registered session.
    runtime_id: LogicalRuntimeId,
    /// Per-session mutation gate.
    ///
    /// Serializes same-session mutating commands across the full
    /// DSL-stage → driver-mutate → DSL-sync span. Without this gate,
    /// two concurrent commands on the same session can interleave between
    /// the DSL projection sync (which releases `sessions` lock) and the
    /// driver mutation (which acquires `driver` lock independently).
    ///
    /// This is NOT a replacement for `sessions` RwLock or `driver` Mutex —
    /// it is an additional serialization point that spans the entire
    /// multi-step mutation window.
    mutation_gate: Arc<Mutex<()>>,
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
    /// A missing-live materialization normalized stale executor authority but
    /// has not yet committed its replacement attachment. The next exact
    /// attachment commit owns the matching durable lifecycle publication.
    pending_revival_lifecycle_persist: Arc<std::sync::atomic::AtomicBool>,
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
    /// at a time.
    post_stop_cleanup_gate: Arc<Mutex<()>>,
    /// Temporary live interrupt capability for prepared, session-owned turns
    /// that run before the runtime loop attachment is published.
    provisional_interrupt_handle:
        Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>>,
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
}

#[derive(Clone)]
struct MemberIncarnationRegistration {
    incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    session_mutation_gate: Arc<Mutex<()>>,
    tracked_turn_journal: Option<Arc<dyn crate::member_observation::TrackedTurnJournal>>,
}

enum MemberResidencyState {
    /// Ordinary runtime session with no host-placement authority.
    PeerOnly,
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
    session_mutation_gate: Arc<Mutex<()>>,
}

/// Exact effect interval for a host-member residency. The stable residency
/// slot blocks G1→G2/vacate publication while the captured session guard
/// blocks unregister/same-SessionId runtime replacement. Neither authority
/// may be dropped before the caller's effect completes.
pub(crate) struct MemberEffectAuthorityGuard {
    _slot_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    _session_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

impl MemberResidencySlot {
    fn peer_only() -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            state: StdRwLock::new(MemberResidencyState::PeerOnly),
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
    /// Stable absent-entry/register/unregister transaction boundary. It is
    /// acquired before the live and mutation leases and retained through any
    /// archive-only removal of the recovered entry.
    _registration_transaction_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    /// Shared live/lifecycle boundary retained from physical absence proof
    /// through the durable archive/retire marker.
    _live_lifecycle_lease: Option<crate::member_live::MemberLiveLifecycleLease>,
    _mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
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
    cleanup_spawner: MachineCleanupTaskSpawner,
    archived_resume_authorization_issued: bool,
    armed: bool,
}

/// Exact post-actor commit authority for an Archived+Retired materialization.
///
/// This non-clone lease retains the machine mutation gate while the session
/// owner promotes its durable document and while the runtime performs the
/// matching Retired -> Idle transition. A broad session-control token cannot
/// cross either boundary while an exact materialization claim is outstanding.
pub struct PreparedArchivedResumeCommitLease {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    epoch_id: meerkat_core::RuntimeEpochId,
    claim_id: uuid::Uuid,
    claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    _mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

/// Type-state proof that the session owner durably promoted the archived
/// document through the same runtime store authority as this exact machine
/// attachment claim.
///
/// Only this promoted form can realize Retired -> Idle, making the required
/// document-before-runtime ordering explicit at the API boundary.
pub struct PromotedArchivedResumeCommitLease {
    prepared: PreparedArchivedResumeCommitLease,
}

impl PreparedArchivedResumeCommitLease {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Convert the exact prepared lease into its post-document-promotion
    /// type state. The session service calls this only after its durable
    /// document write succeeds, and the shared-store check prevents a foreign
    /// service from authorizing this machine's runtime reset.
    pub fn confirm_document_promoted(
        self,
        runtime_store: &Arc<dyn crate::RuntimeStore>,
    ) -> Result<PromotedArchivedResumeCommitLease, RuntimeDriverError> {
        if !self.machine.shares_runtime_store_authority(runtime_store) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "archived document promotion for session {} used a runtime store not owned by the prepared machine",
                    self.session_id
                ),
            });
        }
        Ok(PromotedArchivedResumeCommitLease { prepared: self })
    }
}

impl PromotedArchivedResumeCommitLease {
    pub fn session_id(&self) -> &SessionId {
        self.prepared.session_id()
    }

    /// Realize the exact Retired -> Idle half after durable document
    /// promotion has been certified by the session owner.
    pub async fn reset_retired_runtime(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        self.prepared
            .machine
            .reset_runtime_for_promoted_archived_resume(self)
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
            .lock_current_session_mutation_gate(&self.session_id)
            .await
            .ok_or(crate::RuntimeActorMaterializationError::RegistrationClosed)?;
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
    registration_gate: std::sync::Weak<Mutex<()>>,
}

impl RuntimeSessionRegistrationWitness {
    fn new(
        machine: std::sync::Weak<MeerkatMachineShared>,
        session_id: SessionId,
        epoch_id: meerkat_core::RuntimeEpochId,
        registration_gate: std::sync::Weak<Mutex<()>>,
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

    fn belongs_to(&self, machine: &MeerkatMachine) -> bool {
        std::ptr::eq(self.machine.as_ptr(), Arc::as_ptr(&machine.shared))
    }

    fn matches_entry(&self, entry: &RuntimeSessionEntry) -> bool {
        self.registration_gate
            .upgrade()
            .is_some_and(|gate| Arc::ptr_eq(&gate, &entry.mutation_gate))
    }

    fn registration_gate(&self) -> Option<Arc<Mutex<()>>> {
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
            && std::sync::Weak::ptr_eq(&self.registration_gate, &other.registration_gate)
    }
}

impl Eq for RuntimeSessionRegistrationWitness {}

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

/// Process-lifetime cleanup dispatcher for surface transactions that must
/// outlive the ambient Tokio runtime which opened them.
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

        let candidate = crate::tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("meerkat-machine-cleanup")
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
    cleanup_spawner: MachineCleanupTaskSpawner,
    should_wake: bool,
    persist_lifecycle_on_commit: bool,
    armed: bool,
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
            .unregister_executor_attachment_if_current_with_guard(self.witness.clone(), guard)
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

impl PendingRuntimeExecutorAttachment {
    fn new(
        machine: Arc<MeerkatMachine>,
        witness: RuntimeExecutorAttachmentWitness,
        mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
        cleanup_spawner: MachineCleanupTaskSpawner,
        should_wake: bool,
        persist_lifecycle_on_commit: bool,
    ) -> Self {
        Self {
            machine,
            witness,
            mutation_guard: Some(mutation_guard),
            cleanup_spawner,
            should_wake,
            persist_lifecycle_on_commit,
            armed: true,
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
        // guard into the publication lease instead of releasing it.
        Ok(CommittedRuntimeExecutorAttachmentPublicationLease {
            machine: Arc::clone(&self.machine),
            witness,
            mutation_guard: Some(mutation_guard),
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
            let (predecessor_input_ids, candidate_owner_input_id) = {
                let mut driver = driver.lock().await;
                let predecessor_input_ids = driver.as_driver().active_input_ids();
                if predecessor_input_ids.is_empty() {
                    (predecessor_input_ids, None)
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
                    let candidate_owner_input_id = crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared);
                    (predecessor_input_ids, candidate_owner_input_id)
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
                if let Some(candidate_owner_input_id) = candidate_owner_input_id {
                    predecessor_terminal_publication = Some((
                        driver,
                        publication_handle,
                        predecessor_input_ids,
                        candidate_owner_input_id,
                    ));
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
            }
            self.armed = false;
            if let Some((driver, publication_handle, input_ids, candidate_owner_input_id)) =
                predecessor_terminal_publication
            {
                // External publication must never pin M or delay B's serving
                // linearization. The terminal/input rows above are already
                // durable; this bounded best-effort handoff records receipts
                // after M is released. Failure leaves the exact outbox for a
                // later attachment's ordinary startup recovery.
                let _terminal_publication = self.cleanup_spawner.spawn(async move {
                    let deadline = Instant::now() + std::time::Duration::from_secs(5);
                    if let Err(error) = crate::control_plane::publish_and_resolve_runless_runtime_termination_before(
                        &driver,
                        None,
                        publication_handle.as_deref(),
                        &input_ids,
                        Some(&candidate_owner_input_id),
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
        let completion = self.cleanup_spawner.spawn(async move {
            machine
                .abort_pending_executor_attachment(witness, guard)
                .await
        });
        self.armed = false;
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned runtime attachment abort ended without a result: {error}"
            ))
        })?
    }

    /// Abort while the caller owns the stable session turn boundary. The
    /// stored attachment-local cleanup handle is completed through its
    /// non-reentrant variant before the exact unregister saga runs.
    pub async fn abort_under_runtime_turn_finalization_boundary(
        mut self,
    ) -> Result<(), RuntimeDriverError> {
        self.machine
            .complete_executor_attachment_cleanup_under_runtime_turn_boundary(&self.witness)
            .await?;
        let guard = self.mutation_guard.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "pending runtime attachment lost its mutation fence before boundary-owned abort"
                    .to_string(),
            )
        })?;
        let machine = Arc::clone(&self.machine);
        let witness = self.witness.clone();
        let completion = self.cleanup_spawner.spawn(async move {
            machine
                .abort_pending_executor_attachment(witness, guard)
                .await
        });
        self.armed = false;
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned boundary-aware runtime attachment abort ended without a result: {error}"
            ))
        })?
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
            let predecessor_input_ids = driver.as_driver().active_input_ids();
            if predecessor_input_ids.is_empty() {
                return Ok(0);
            }
            let abandoned = driver
                .abandon_pending_inputs(crate::input_state::InputAbandonReason::Cancelled)
                .await?;
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
        let completion = self
            .machine
            .spawn_executor_attachment_retirement_with_guard(
                self.witness.clone(),
                guard,
                self.cleanup_spawner.clone(),
            );
        self.armed = false;
        Ok(completion)
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
        let completion = self.cleanup_spawner.spawn(async move {
            machine
                .unregister_executor_attachment_if_current_with_guard(witness, guard)
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
        self.cleanup_spawner.spawn(async move {
            if let Err(error) = machine
                .unregister_executor_attachment_if_current_with_guard(witness.clone(), guard)
                .await
            {
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
        self.cleanup_spawner.spawn(async move {
            if let Err(error) = machine
                .abort_pending_executor_attachment(witness.clone(), guard)
                .await
            {
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
            cleanup_spawner,
            archived_resume_authorization_issued: false,
            armed: true,
        })
    }

    pub fn bindings(&self) -> &meerkat_core::SessionRuntimeBindings {
        &self.bindings
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

    /// Whether this lease still owns the exact process-local materialization
    /// claim that fences same-session actor creation and executor attachment.
    #[must_use]
    pub async fn owns_current_materialization_claim(&self) -> bool {
        if !self.armed {
            return false;
        }
        let Some(_mutation_guard) = self
            .machine
            .lock_current_session_mutation_gate(self.session_id())
            .await
        else {
            return false;
        };
        let Ok(authority) = crate::validated_session_runtime_bindings_authority(&self.bindings)
        else {
            return false;
        };
        let sessions = self.machine.sessions.read().await;
        sessions.get(self.session_id()).is_some_and(|entry| {
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
        &self,
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
            .lock_current_session_mutation_gate(self.session_id())
            .await
            .ok_or_else(|| RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "archived-resume session {} disappeared before commit lease acquisition",
                    self.session_id()
                ),
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
            // start a second cleanup saga.
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
    /// transaction is fully unregistered.
    pub async fn rollback_now(&mut self) -> Result<bool, RuntimeDriverError> {
        self.rollback_now_with_turn_finalization_boundary(false)
            .await
    }

    /// Exact rollback for a caller that already owns the persistent service's
    /// turn-finalization boundary.
    pub async fn rollback_now_under_turn_finalization_boundary(
        &mut self,
    ) -> Result<bool, RuntimeDriverError> {
        self.rollback_now_with_turn_finalization_boundary(true)
            .await
    }

    async fn rollback_now_with_turn_finalization_boundary(
        &mut self,
        turn_finalization_boundary_already_held: bool,
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

impl MachineSessionArchiveLease {
    /// Whether archive preparation inserted the captured in-memory
    /// registration from durable authority.
    pub fn owns_recovered_registration(&self) -> bool {
        self.recovered_registration_for_archive
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
    mutation_gate: Arc<Mutex<()>>,
    driver: SharedDriver,
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
    attachment_id: RuntimeLoopAttachmentId,
    effect_tx: mpsc::Sender<crate::effect::RuntimeEffect>,
}

/// Exact executor attachment captured before a cooperative live-boundary
/// callback. Publication resumes only after every attachment-local component
/// is revalidated.
struct RuntimeLiveBoundaryAttachmentWitness {
    mutation_gate: Arc<Mutex<()>>,
    driver: SharedDriver,
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
    attachment_id: RuntimeLoopAttachmentId,
    boundary_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>,
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

/// Mechanical runtime-loop channel slot.
enum RuntimeLoopAttachmentSlot {
    Empty,
    Pending(RuntimeLoopAttachment),
    Attached(RuntimeLoopAttachment),
}

impl RuntimeSessionEntry {
    fn dsl_mutation_blocked_by_unregister(
        &self,
        session_id: &SessionId,
    ) -> Option<RuntimeDriverError> {
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
            let claim_changed = state.current.take().is_some();
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
    #[cfg(test)]
    pub(crate) async fn model_routing_handle_for_test(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::handles::RuntimeModelRoutingHandle>> {
        let (dsl_authority, visibility_owner) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            (
                Arc::clone(&entry.dsl_authority),
                Arc::clone(&entry.tool_visibility_owner),
            )
        };
        Some(Arc::new(
            crate::handles::RuntimeModelRoutingHandle::new_with_visibility_owner(
                Arc::new(crate::handles::HandleDslAuthority::from_shared(
                    dsl_authority,
                )),
                visibility_owner,
            ),
        ))
    }

    /// Acquire the per-session mutation gate.
    ///
    /// Returns an `Arc<Mutex<()>>` that the caller must `.lock().await` and
    /// hold across the full DSL-stage → driver-mutate → DSL-sync span.
    /// Returns `None` if the session is not registered.
    async fn session_mutation_gate(&self, session_id: &SessionId) -> Option<Arc<Mutex<()>>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| Arc::clone(&entry.mutation_gate))
    }

    async fn lock_current_session_mutation_gate(
        &self,
        session_id: &SessionId,
    ) -> Option<crate::tokio::sync::OwnedMutexGuard<()>> {
        loop {
            let gate = self.session_mutation_gate(session_id).await?;
            let gate_guard = Arc::clone(&gate).lock_owned().await;
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            if Arc::ptr_eq(&entry.mutation_gate, &gate) {
                return Some(gate_guard);
            }
        }
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
            .lock_current_session_mutation_gate(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
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

    #[cfg(feature = "live")]
    async fn lock_session_mutation_gate_for_live_lifecycle_lease(
        &self,
        session_id: &SessionId,
        lease: &crate::member_live::MemberLiveLifecycleLease,
    ) -> Option<crate::tokio::sync::OwnedMutexGuard<()>> {
        let mutation_gate = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            if !lease.matches_gate(&entry.live_lifecycle_gate) {
                return None;
            }
            Arc::clone(&entry.mutation_gate)
        };
        let guard = Arc::clone(&mutation_gate).lock_owned().await;
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        if lease.matches_gate(&entry.live_lifecycle_gate)
            && Arc::ptr_eq(&entry.mutation_gate, &mutation_gate)
        {
            Some(guard)
        } else {
            None
        }
    }

    pub(crate) async fn lock_current_session_driver_gate(
        &self,
        session_id: &SessionId,
        driver: &SharedDriver,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        let gate_guard = self
            .lock_current_session_mutation_gate(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
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
        let handle = std::sync::Arc::new(
            crate::handles::HandleDslAuthority::from_shared_with_teardown_gate(dsl, teardown_gate),
        );
        crate::handles::RuntimePeerCommsHandle::install_generated_on(handle, runtime)
    }

    fn preview_dsl_input_on_state(
        state: &dsl::MeerkatMachineState,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<dsl::MeerkatMachineEffect>, String> {
        let mut preview = dsl::MeerkatMachineAuthority::recover_from_state(state.clone())
            .map_err(|err| dsl_authority::map_error(err, context))?;
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
        let state = {
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            authority.state().clone()
        };
        Self::preview_dsl_input_on_state(&state, input, context)
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
            .dispatch_routed_signals_from_effects(&staged.effects)
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
        effects: &[dsl::MeerkatMachineEffect],
    ) -> Result<(), String> {
        let dispatcher = {
            self.composition_signal_dispatcher
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        let Some(dispatcher) = dispatcher else {
            return Ok(());
        };

        for effect in effects {
            if let Some(signal) = composition::lift_routed_signal(effect) {
                composition::dispatch_routed_signal(&dispatcher, signal).await?;
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

        let gate_guard = Arc::clone(&witness.mutation_gate).lock_owned().await;
        let current_effect_tx = {
            let sessions = self.sessions.read().await;
            match sessions.get(session_id) {
                None => Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "{context}: runtime session disappeared during boundary callback"
                    ),
                }),
                Some(entry)
                    if !Arc::ptr_eq(&entry.mutation_gate, &witness.mutation_gate)
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
                Some(entry) => match entry.effect_sender() {
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
                },
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
        self.reject_unregistration_drain_ingress(session_id, state)
            .await?;

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

    /// Own all post-admission boundary work for an accepted ingress request.
    /// Once the driver and DSL have accepted the input, caller cancellation may
    /// drop only the acknowledgement: cancel dispatch and wake remain
    /// process-owned.
    async fn dispatch_accepted_ingress_boundary_work(
        &self,
        session_id: &SessionId,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        cancel_plan: Option<RuntimeAcceptedBoundaryCancelPlan>,
        _completions: crate::meerkat_machine::driver::SharedCompletionRegistry,
        wake_tx: Option<mpsc::Sender<()>>,
        should_wake: bool,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;
        let machine = self.clone();
        let session_id = session_id.clone();
        let completion = cleanup_spawner.spawn(async move {
            let mut gate_guard = held_mutation_gate;
            if let Some(cancel_plan) = cancel_plan {
                gate_guard = machine
                    .dispatch_cancel_after_boundary_runtime_effect_owned(
                        &session_id,
                        cancel_plan.witness,
                        gate_guard,
                        cancel_plan.boundary_handle,
                        None,
                        cancel_plan.pending_dispatch,
                        Some(&cancel_plan.expected_run_id),
                        cancel_plan.projected_effect,
                        cancel_plan.dispatch_generation,
                        cancel_plan.dispatch_lifecycle_phase,
                        "AcceptWithCompletion",
                    )
                    .await?;
            }
            if should_wake && let Some(wake_tx) = wake_tx {
                let _ = wake_tx.try_send(());
            }
            Ok(gate_guard)
        });
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "process-owned accepted-ingress boundary transaction ended without a result: {error}"
            ))
        })?
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
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveWebrtcTokenAuthority {
    pub token: String,
    pub expires_at_ms: u64,
    pub sequence: u64,
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

/// Generated authority output for a WebSocket transport token issued by
/// MeerkatMachine.
///
/// Constructed only from `MeerkatMachineEffect::LiveWebsocketTokenIssued`.
/// The WebSocket transport supplies random bearer material, but it is not
/// returned until generated authority records channel binding and expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "live")]
pub struct LiveWebsocketTokenAuthority {
    pub token: String,
    pub expires_at_ms: u64,
    pub sequence: u64,
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
    ) -> Result<Self, String> {
        Ok(Self {
            status,
            sequence,
            status_observation_sequence,
            degradation_reason,
            degradation_detail,
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

/// Session-scoped execution kernel for the Meerkat runtime.
///
/// Owns per-session runtime state (driver, ops registry, completion waiters,
/// comms drain, epoch bindings) and routes all internal mutations through one
/// canonical command reducer, with smaller group handlers retained only as
/// implementation detail helpers.
#[doc(hidden)]
pub struct MeerkatMachineShared {
    /// Per-session entries.
    sessions: RwLock<HashMap<SessionId, RuntimeSessionEntry>>,
    /// Stable process-local serialization slots for session registration and
    /// final unregister publication. The index retains only weak references:
    /// every new lookup prunes dead slots, so historical session ids cannot
    /// accumulate while overlapping transactions still rendezvous on one gate.
    registration_transaction_slots: StdRwLock<HashMap<SessionId, std::sync::Weak<Mutex<()>>>>,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
    /// Blob store used by persistent drivers for durable input externalization.
    blob_store: Option<Arc<dyn BlobStore>>,
    /// Runtime-owned shell seam for live session LLM reconfiguration I/O.
    llm_reconfigure_host: StdRwLock<Option<Arc<dyn SessionLlmReconfigureHost>>>,
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
    /// Optional typed signal dispatcher for MeerkatMachine lifecycle
    /// effects routed by `meerkat_mob_seam` into MobMachine observation
    /// signals.
    composition_signal_dispatcher:
        StdRwLock<Option<composition::MeerkatCompositionSignalDispatcher>>,
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
    /// Deterministic test gate after fenced input captures its residency slot
    /// and exact session gate but before it locks that session gate.
    #[cfg(test)]
    test_fenced_accept_after_lease: StdMutex<
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
        session_mutation_gate: Arc<crate::tokio::sync::Mutex<()>>,
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
            .lock_current_session_mutation_gate(&self.session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let gate = {
            let sessions = self.adapter.sessions.read().await;
            Arc::clone(
                &sessions
                    .get(&self.session_id)
                    .ok_or(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })?
                    .mutation_gate,
            )
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

    /// Resolve the injected member live host, if composed.
    pub(crate) fn member_live_host(&self) -> Option<Arc<dyn crate::member_live::MemberLiveHost>> {
        self.member_live_host
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
            MemberResidencyState::PeerOnly | MemberResidencyState::VacantPlaced => None,
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
            MemberResidencyState::PeerOnly | MemberResidencyState::VacantPlaced => None,
        }
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
        let session_guard = Arc::clone(&session_mutation_gate).lock_owned().await;
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member effect expected incarnation {expected:?}; runtime session is absent"
                    ),
                });
            };
            if !Arc::ptr_eq(&entry.mutation_gate, &session_mutation_gate) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "member effect expected incarnation {expected:?}; runtime session was replaced"
                    ),
                });
            }
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
                    Some(Arc::clone(&registration.session_mutation_gate))
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
                    MemberResidencyState::PeerOnly | MemberResidencyState::VacantPlaced,
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
                (None, MemberResidencyState::PeerOnly) => None,
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
            if !Arc::ptr_eq(&entry.mutation_gate, &registered_gate) {
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
                    && Arc::ptr_eq(
                        &registration.session_mutation_gate,
                        &lease.session_mutation_gate,
                    ) =>
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
                MemberResidencyState::PeerOnly | MemberResidencyState::VacantPlaced,
            ) => Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "member effect expected incarnation {expected:?}; current residency is absent"
                ),
            }),
            (None, MemberResidencyState::PeerOnly) => Ok(()),
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

    /// Whether this adapter shares the same runtime persistence authority as
    /// another adapter. Runtime-backed composition surfaces use this to reject
    /// mismatched adapters before visible terminal events can outrun the store
    /// that owns their durable commit.
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
                sessions: RwLock::new(HashMap::new()),
                registration_transaction_slots: StdRwLock::new(HashMap::new()),
                store: None,
                blob_store: None,
                llm_reconfigure_host: StdRwLock::new(None),
                member_observation_host: StdRwLock::new(None),
                member_live_host: StdRwLock::new(None),
                live_commands_served: std::sync::atomic::AtomicU64::new(0),
                member_incarnation_slots: StdRwLock::new(HashMap::new()),
                auth_lease: StdRwLock::new(auth_lease),
                #[cfg(not(target_arch = "wasm32"))]
                oauth_flows: StdRwLock::new(oauth_flows),
                #[cfg(feature = "live")]
                live_unbound_rejection_authority: live_unbound_rejection_authority(),
                session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
                composition_signal_dispatcher: StdRwLock::new(None),
                #[cfg(feature = "test-support")]
                test_stop_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_pause_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_reached: crate::tokio::sync::Notify::new(),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_release: crate::tokio::sync::Notify::new(),
                #[cfg(test)]
                test_fenced_accept_after_lease: StdMutex::new(None),
                #[cfg(test)]
                test_registration_transaction_contention_probe: StdMutex::new(None),
                #[cfg(test)]
                test_fail_post_stop_unregister_after_fence: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_queue_authority: StdMutex::new(None),
                #[cfg(test)]
                test_control_command_after_logical_lookup: StdMutex::new(None),
            }),
        }
    }

    /// Create a persistent adapter with a RuntimeStore.
    ///
    /// One logical runtime id must have exactly one live `MeerkatMachine`
    /// authority. Separate machines may partition one store across distinct
    /// runtime ids, but must not concurrently register the same session. Share
    /// this machine (normally through `Arc`) when composing multiple surfaces.
    pub fn persistent(store: Arc<dyn RuntimeStore>, blob_store: Arc<dyn BlobStore>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let (auth_lease, oauth_flows) = {
            let authorities = persistent_auth_authorities(&store);
            (
                Arc::clone(&authorities.auth_lease),
                Arc::clone(&authorities.oauth_flows),
            )
        };
        #[cfg(target_arch = "wasm32")]
        let auth_lease = Arc::new(crate::handles::RuntimeAuthLeaseHandle::new());
        let auth_lease = generated_runtime_auth_lease_handle(auth_lease);
        Self {
            shared: Arc::new(MeerkatMachineShared {
                sessions: RwLock::new(HashMap::new()),
                registration_transaction_slots: StdRwLock::new(HashMap::new()),
                store: Some(store),
                blob_store: Some(blob_store),
                llm_reconfigure_host: StdRwLock::new(None),
                member_observation_host: StdRwLock::new(None),
                member_live_host: StdRwLock::new(None),
                live_commands_served: std::sync::atomic::AtomicU64::new(0),
                member_incarnation_slots: StdRwLock::new(HashMap::new()),
                auth_lease: StdRwLock::new(auth_lease),
                #[cfg(not(target_arch = "wasm32"))]
                oauth_flows: StdRwLock::new(oauth_flows),
                #[cfg(feature = "live")]
                live_unbound_rejection_authority: live_unbound_rejection_authority(),
                session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
                composition_signal_dispatcher: StdRwLock::new(None),
                #[cfg(feature = "test-support")]
                test_stop_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_pause_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_reached: crate::tokio::sync::Notify::new(),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_release: crate::tokio::sync::Notify::new(),
                #[cfg(test)]
                test_fenced_accept_after_lease: StdMutex::new(None),
                #[cfg(test)]
                test_registration_transaction_contention_probe: StdMutex::new(None),
                #[cfg(test)]
                test_fail_post_stop_unregister_after_fence: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_queue_authority: StdMutex::new(None),
                #[cfg(test)]
                test_control_command_after_logical_lookup: StdMutex::new(None),
            }),
        }
    }

    /// Create a persistent adapter with a RuntimeStore but no blob store.
    ///
    /// The driver remains persistent for session state. Blob-backed inputs fail
    /// explicitly at the blob-store boundary until a real [`BlobStore`] is
    /// supplied. As with [`Self::persistent`], one logical runtime id must have
    /// exactly one live machine authority.
    pub fn persistent_without_blobs(store: Arc<dyn RuntimeStore>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let (auth_lease, oauth_flows) = {
            let authorities = persistent_auth_authorities(&store);
            (
                Arc::clone(&authorities.auth_lease),
                Arc::clone(&authorities.oauth_flows),
            )
        };
        #[cfg(target_arch = "wasm32")]
        let auth_lease = Arc::new(crate::handles::RuntimeAuthLeaseHandle::new());
        let auth_lease = generated_runtime_auth_lease_handle(auth_lease);
        Self {
            shared: Arc::new(MeerkatMachineShared {
                sessions: RwLock::new(HashMap::new()),
                registration_transaction_slots: StdRwLock::new(HashMap::new()),
                store: Some(store),
                blob_store: Some(Arc::new(UnavailableBlobStore)),
                llm_reconfigure_host: StdRwLock::new(None),
                member_observation_host: StdRwLock::new(None),
                member_live_host: StdRwLock::new(None),
                live_commands_served: std::sync::atomic::AtomicU64::new(0),
                member_incarnation_slots: StdRwLock::new(HashMap::new()),
                auth_lease: StdRwLock::new(auth_lease),
                #[cfg(not(target_arch = "wasm32"))]
                oauth_flows: StdRwLock::new(oauth_flows),
                #[cfg(feature = "live")]
                live_unbound_rejection_authority: live_unbound_rejection_authority(),
                session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
                composition_signal_dispatcher: StdRwLock::new(None),
                #[cfg(feature = "test-support")]
                test_stop_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_pause_executor_after_ensure: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_reached: crate::tokio::sync::Notify::new(),
                #[cfg(feature = "test-support")]
                test_executor_after_ensure_pause_release: crate::tokio::sync::Notify::new(),
                #[cfg(test)]
                test_fenced_accept_after_lease: StdMutex::new(None),
                #[cfg(test)]
                test_registration_transaction_contention_probe: StdMutex::new(None),
                #[cfg(test)]
                test_fail_post_stop_unregister_after_fence: StdMutex::new(None),
                #[cfg(any(test, feature = "test-support"))]
                test_runtime_loop_before_queue_authority: StdMutex::new(None),
                #[cfg(test)]
                test_control_command_after_logical_lookup: StdMutex::new(None),
            }),
        }
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

    /// Install the auth lifecycle authority that public surfaces also read.
    ///
    /// Surfaces construct the adapter before all state fields are available, so
    /// this setter lets them align the adapter's runtime-backed traffic with
    /// the surface-visible status handle without creating a competing registry.
    pub fn set_auth_lease_handle(&self, handle: Arc<crate::handles::RuntimeAuthLeaseHandle>) {
        self.set_runtime_auth_lease_handle(handle);
    }

    /// Install the runtime credential lifecycle handle together with an
    /// explicit OAuth login-flow authority.
    ///
    /// The credential side still has to be a generated AuthMachine authority;
    /// the explicit OAuth authority only controls login-flow test seams.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_auth_lease_handle_with_oauth_flow_authority(
        &self,
        handle: Arc<crate::handles::RuntimeAuthLeaseHandle>,
        oauth_flows: Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority>,
    ) {
        *self
            .oauth_flows
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = oauth_flows;
        let handle = generated_runtime_auth_lease_handle(handle);
        *self
            .auth_lease
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = handle;
    }

    /// Install a runtime AuthMachine authority shared by auth leases and OAuth
    /// login-flow lifecycle transitions.
    pub fn set_runtime_auth_lease_handle(
        &self,
        handle: Arc<crate::handles::RuntimeAuthLeaseHandle>,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            *self
                .oauth_flows
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Arc::new(crate::handles::RuntimeOAuthFlowHandle::new_with_auth_lease(
                    std::time::Duration::from_secs(10 * 60),
                    Arc::clone(&handle),
                ));
        }
        let handle = generated_runtime_auth_lease_handle(handle);
        *self
            .auth_lease
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = handle;
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

    /// Attach the typed composition signal dispatcher used for
    /// MeerkatMachine -> MobMachine lifecycle observation routes.
    pub fn set_composition_signal_dispatcher(
        &self,
        dispatcher: composition::MeerkatCompositionSignalDispatcher,
    ) {
        let mut slot = self
            .composition_signal_dispatcher
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(dispatcher);
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
    pub(crate) async fn apply_routed_meerkat_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
    ) -> Result<(), dsl_authority::DslTransitionRefusal> {
        let _gate_guard = self
            .lock_current_session_mutation_gate(session_id)
            .await
            .ok_or_else(|| {
                dsl_authority::DslTransitionRefusal::other(
                    "routed_session_not_registered",
                    format!(
                        "session `{session_id}` is not registered with this MeerkatMachine; \
                         cannot deliver routed input"
                    ),
                )
            })?;
        self.apply_routed_session_dsl_input(session_id, input, "RoutedMeerkatInput")
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

    /// Create a driver entry for a session.
    fn make_driver(
        &self,
        runtime_id: LogicalRuntimeId,
        dsl_authority: crate::driver::ephemeral::SharedIngressDslAuthority,
        initial_runtime_state: RuntimeState,
    ) -> DriverEntry {
        let control_projection = Arc::new(StdRwLock::new(
            crate::driver::ephemeral::RuntimeControlProjection {
                phase: initial_runtime_state,
                current_run_id: None,
                pre_run_phase: None,
            },
        ));
        match (&self.store, &self.blob_store) {
            (Some(store), Some(blob_store)) => {
                DriverEntry::Persistent(PersistentRuntimeDriver::new_with_control(
                    runtime_id,
                    store.clone(),
                    blob_store.clone(),
                    control_projection,
                    dsl_authority,
                ))
            }
            _ => DriverEntry::Ephemeral(EphemeralRuntimeDriver::new_with_control_and_dsl(
                runtime_id,
                control_projection,
                dsl_authority,
            )),
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
        Box::pin(async move {
            match command {
                MeerkatMachineCommand::EnsureSessionWithExecutor { .. } => {
                    let self_handle = self_handle.ok_or_else(|| {
                        MeerkatMachineCommandError::Driver(RuntimeDriverError::Internal(
                            "EnsureSessionWithExecutor requires Arc<Self> machine handle".into(),
                        ))
                    })?;
                    self_handle
                        .execute_meerkat_machine_ensure_session_command(command)
                        .await
                        .map_err(Into::into)
                }
                MeerkatMachineCommand::RegisterSession { .. }
                | MeerkatMachineCommand::UnregisterSession { .. }
                | MeerkatMachineCommand::SetSilentIntents { .. }
                | MeerkatMachineCommand::CancelAfterBoundary { .. }
                | MeerkatMachineCommand::StopRuntimeExecutor { .. }
                | MeerkatMachineCommand::CommitServiceTurnTerminalReceipt { .. }
                | MeerkatMachineCommand::ContainsSession { .. }
                | MeerkatMachineCommand::SessionHasExecutor { .. }
                | MeerkatMachineCommand::SessionHasComms { .. }
                | MeerkatMachineCommand::OpsLifecycleRegistry { .. }
                | MeerkatMachineCommand::PrepareBindings { .. }
                | MeerkatMachineCommand::PrepareLocalSessionBindings { .. }
                | MeerkatMachineCommand::InputState { .. }
                | MeerkatMachineCommand::InputStateByIdempotencyKey { .. }
                | MeerkatMachineCommand::InteractionTerminalStatus { .. }
                | MeerkatMachineCommand::RunTerminalStatus { .. }
                | MeerkatMachineCommand::ListActiveInputs { .. }
                | MeerkatMachineCommand::ReconfigureSessionLlmIdentity { .. }
                | MeerkatMachineCommand::StagePersistentFilter { .. }
                | MeerkatMachineCommand::RequestDeferredTools { .. }
                | MeerkatMachineCommand::PublishCommittedVisibleSet { .. } => self
                    .execute_meerkat_machine_session_command(command)
                    .await
                    .map_err(Into::into),
                MeerkatMachineCommand::SetPeerIngressContext { .. }
                | MeerkatMachineCommand::NotifyDrainExited { .. } => {
                    let self_handle = self_handle.ok_or_else(|| {
                        MeerkatMachineCommandError::Driver(RuntimeDriverError::Internal(
                            "drain command requires Arc<Self> machine handle".into(),
                        ))
                    })?;
                    self_handle
                        .execute_meerkat_machine_drain_command(command)
                        .await
                        .map_err(Into::into)
                }
                MeerkatMachineCommand::AbortAll
                | MeerkatMachineCommand::Abort { .. }
                | MeerkatMachineCommand::Wait { .. } => self
                    .execute_meerkat_machine_drain_local_command(command)
                    .await
                    .map_err(Into::into),
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
                | MeerkatMachineCommand::AdmitModelRoutingAssistantTurn { .. }
                | MeerkatMachineCommand::BeginImageOperation { .. }
                | MeerkatMachineCommand::DenyImageOperationPlan { .. }
                | MeerkatMachineCommand::ActivateImageOperationOverride { .. }
                | MeerkatMachineCommand::ClassifyImageOperationTerminal { .. }
                | MeerkatMachineCommand::CompleteImageOperation { .. }
                | MeerkatMachineCommand::RestoreImageOperationOverride { .. }
                | MeerkatMachineCommand::LoadBoundaryReceipt { .. } => self
                    .execute_meerkat_machine_control_command(command)
                    .await
                    .map_err(Into::into),
                MeerkatMachineCommand::AcceptWithCompletion { .. }
                | MeerkatMachineCommand::AcceptWithoutWake { .. } => self
                    .execute_meerkat_machine_ingress_command(command)
                    .await
                    .map_err(Into::into),
            }
        })
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
