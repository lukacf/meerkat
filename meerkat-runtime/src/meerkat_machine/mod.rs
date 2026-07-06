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
    SessionLlmCapabilityDelta, SessionLlmCapabilitySurface, SessionLlmReconfigureHost,
    SessionLlmReconfigureReport, SessionLlmReconfigureRequest, SessionToolVisibilityDelta,
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

/// Build a generated visibility owner for standalone facade sessions.
///
/// Standalone sessions do not have a runtime loop, but durable tool visibility
/// is still a machine fact. This owner gives those sessions the same
/// MeerkatMachine authority path used by runtime-backed sessions instead of
/// falling back to a handwritten local mutator.
pub fn standalone_tool_visibility_owner(
    session_id: &SessionId,
    current_identity: &meerkat_core::SessionLlmIdentity,
    model_profile: Option<&meerkat_core::model_profile::ModelProfile>,
    capability_base_filter: &ToolFilter,
) -> Result<meerkat_core::GeneratedToolVisibilityOwner, String> {
    let mut authority = dsl_authority::recover_authority_from_runtime_observation(
        session_id,
        RuntimeState::Idle,
        None,
        None,
        None,
        BTreeSet::new(),
        None,
        None,
        None,
    )
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
    let authority = Arc::new(std::sync::Mutex::new(authority));
    let owner = Arc::new(MachineToolVisibilityOwner::new());
    owner.bind_dsl_authority(authority);
    generated_tool_visibility_owner(owner as Arc<dyn ToolVisibilityOwner>)
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
    let observed_state = dsl_authority::observed_runtime_lifecycle_state(state);
    let mut authority = projection_authority();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ClassifyRuntimeLifecycleDurability {
            state: observed_state,
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

pub(crate) use driver::{
    DriverEntry, SharedCompletionRegistry, SharedDriver, cancel_runtime_loop_run,
    commit_runtime_loop_run, fail_machine_run, fail_runtime_loop_run,
    machine_authorize_runtime_loop_batch, machine_batch_primitive_projections,
    machine_batch_runtime_semantics, machine_commit_prepared_destroy,
    machine_commit_service_turn_terminal_receipt, machine_prepare_bindings_projection,
    machine_prepare_destroy, machine_recover_ephemeral_driver, machine_recover_persistent_driver,
    machine_recycle_preserving_work, machine_reset, machine_retire, machine_stop_runtime,
    prepare_runtime_loop_batch_start,
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

pub use composition::{MeerkatCompositionSignalDispatcher, MeerkatConsumerSurface};

pub use comms_drain::{
    CommsDrainMode, CommsDrainPhase, DrainExitReason, PeerEndpointStageError, PeerIngressOwner,
    SupervisorBinding, SupervisorBindingStageError,
};
pub(crate) use comms_drain::{
    CommsDrainSlot, SupervisorAuthorizeAdmission, SupervisorBindAdmission,
    SupervisorBridgeCommandAdmission, abort_slot,
};
pub(crate) use dsl_effects::{DslTransitionEffects, apply_dsl_transition_on_authority};
pub(crate) use visibility::MachineToolVisibilityOwner;

struct StagedSessionDslInput {
    previous_snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    committed_snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    effects: DslTransitionEffects,
}

#[derive(Clone, Copy)]
enum CommittedEffectDispatchFailure {
    PreserveCommittedDslState,
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
    /// Runtime epoch identity — stable across rebuilds, rotated on reset/restart-without-recovery.
    epoch_id: meerkat_core::RuntimeEpochId,
    /// Mechanical close gate for handles minted from this session entry.
    ///
    /// The DSL still owns runtime terminality; this gate only invalidates cloned
    /// cross-crate handles after the entry is torn down.
    handle_teardown_gate: Arc<crate::handles::HandleTeardownGate>,
    /// Shared consumer cursor state for the epoch.
    cursor_state: Arc<meerkat_core::EpochCursorState>,
    /// Completion waiters (accessed by accept_input_with_completion and RuntimeLoop).
    completions: SharedCompletionRegistry,
    /// Canonical durable visibility owner for this session.
    tool_visibility_owner: Arc<MachineToolVisibilityOwner>,
    /// Runtime-loop channel publication slot.
    ///
    /// This is mechanical shell state only. The generated `MeerkatMachine`
    /// `registration_phase` is the semantic executor registration authority.
    attachment_slot: RuntimeLoopAttachmentSlot,
    /// Temporary live interrupt capability for prepared, session-owned turns
    /// that run before the runtime loop attachment is published.
    provisional_interrupt_handle:
        Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>>,
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

/// Capability bundle for an attached runtime loop.
///
/// Keep all loop-related handles together so "attached vs detached" cannot
/// drift into partially-populated shell state.
struct RuntimeLoopAttachment {
    wake_tx: mpsc::Sender<()>,
    effect_tx: mpsc::Sender<crate::effect::RuntimeEffect>,
    boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
    interrupt_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>>,
    loop_handle: tokio::task::JoinHandle<()>,
}

/// Mechanical runtime-loop channel slot.
enum RuntimeLoopAttachmentSlot {
    Empty,
    Attached(RuntimeLoopAttachment),
}

impl RuntimeSessionEntry {
    fn control_snapshot(&self) -> crate::driver::ephemeral::RuntimeControlProjection {
        self.control_projection
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner().clone()
            })
    }

    fn attachment_is_live(&self) -> bool {
        match &self.attachment_slot {
            RuntimeLoopAttachmentSlot::Attached(attachment) => {
                !attachment.wake_tx.is_closed() && !attachment.effect_tx.is_closed()
            }
            RuntimeLoopAttachmentSlot::Empty => false,
        }
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
        MeerkatMachine::stage_runtime_internal_dsl_transition_on_authority(
            &self.dsl_authority,
            crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::RuntimeExecutorExited,
        )
    }

    /// Returns `true` only if the executor is fully attached with live channels.
    /// Used by internal publish logic within `ensure_session_with_executor`.
    fn has_live_attachment(&self) -> bool {
        self.attachment_is_live()
    }

    fn attach_runtime_loop(
        &mut self,
        wake_tx: mpsc::Sender<()>,
        effect_tx: mpsc::Sender<crate::effect::RuntimeEffect>,
        boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
        interrupt_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>>,
        loop_handle: tokio::task::JoinHandle<()>,
    ) {
        self.provisional_interrupt_handle = None;
        self.attachment_slot = RuntimeLoopAttachmentSlot::Attached(RuntimeLoopAttachment {
            wake_tx,
            effect_tx,
            boundary_handle,
            interrupt_handle,
            loop_handle,
        });
    }

    /// Detach the runtime-loop channels, returning the loop's `JoinHandle` so a
    /// caller can await its quiescence.
    ///
    /// Dropping the returned `wake_tx`/`effect_tx` (held inside the attachment)
    /// closes the loop's receivers, which drives the loop through its canonical
    /// `StopRuntimeExecutor` + `RuntimeExecutorExited` exit. The slot is left
    /// `Empty`. Returns `None` when no loop is attached.
    fn take_loop_join_handle(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        match std::mem::replace(&mut self.attachment_slot, RuntimeLoopAttachmentSlot::Empty) {
            RuntimeLoopAttachmentSlot::Attached(attachment) => Some(attachment.loop_handle),
            RuntimeLoopAttachmentSlot::Empty => None,
        }
    }

    fn clear_dead_attachment(&mut self) -> bool {
        if matches!(self.attachment_slot, RuntimeLoopAttachmentSlot::Attached(_))
            && !self.attachment_is_live()
        {
            self.attachment_slot = RuntimeLoopAttachmentSlot::Empty;
            return true;
        }
        false
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

    fn install_provisional_interrupt_handle(
        &mut self,
        handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>,
    ) {
        if !self.attachment_is_live() {
            self.provisional_interrupt_handle = Some(handle);
        }
    }
}

impl MeerkatMachine {
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

    async fn current_session_driver_with_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(SharedDriver, crate::tokio::sync::OwnedMutexGuard<()>), RuntimeDriverError> {
        let gate_guard = self
            .lock_current_session_mutation_gate(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let driver = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?
                .driver
                .clone()
        };
        Ok((driver, gate_guard))
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

    async fn clear_dead_runtime_attachment(&self, session_id: &SessionId) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            let cleared = entry.clear_dead_attachment();
            if cleared && let Err(error) = entry.stage_generated_executor_exit_observation() {
                tracing::warn!(
                    %session_id,
                    error = %error,
                    "generated MeerkatMachine rejected executor-exit observation while clearing dead attachment"
                );
            }
        }
    }

    async fn dispatch_cancel_after_boundary_runtime_effect(
        &self,
        session_id: &SessionId,
        effect_tx: Option<mpsc::Sender<crate::effect::RuntimeEffect>>,
        boundary_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>>,
        projected_effect: crate::effect::ProjectedRuntimeEffect,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        let Some(effect_tx) = effect_tx else {
            let state = self
                .existing_session_runtime_state(session_id)
                .await
                .unwrap_or(RuntimeState::Destroyed);
            return Err(RuntimeDriverError::NotReady { state });
        };

        let reason = projected_effect.reason().to_string();
        if let Some(boundary_handle) = boundary_handle {
            boundary_handle
                .cancel_after_boundary(reason)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "{context}: failed to apply live boundary cancel: {err}"
                    ))
                })?;
        }

        match effect_tx.send(projected_effect.into_effect()).await {
            Ok(()) => Ok(()),
            Err(_) => {
                self.clear_dead_runtime_attachment(session_id).await;
                Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Idle,
                })
            }
        }
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
pub struct MeerkatMachine {
    /// Per-session entries.
    sessions: RwLock<HashMap<SessionId, RuntimeSessionEntry>>,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
    /// Blob store used by persistent drivers for durable input externalization.
    blob_store: Option<Arc<dyn BlobStore>>,
    /// Runtime-owned shell seam for live session LLM reconfiguration I/O.
    llm_reconfigure_host: StdRwLock<Option<Arc<dyn SessionLlmReconfigureHost>>>,
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
}

impl MeerkatMachine {
    /// Capability token for store-only session-control mutations routed
    /// through this machine authority.
    #[must_use]
    pub fn session_control_authority(&self) -> MachineSessionControlAuthority {
        MachineSessionControlAuthority { _private: () }
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
            sessions: RwLock::new(HashMap::new()),
            store: None,
            blob_store: None,
            llm_reconfigure_host: StdRwLock::new(None),
            auth_lease: StdRwLock::new(auth_lease),
            #[cfg(not(target_arch = "wasm32"))]
            oauth_flows: StdRwLock::new(oauth_flows),
            #[cfg(feature = "live")]
            live_unbound_rejection_authority: live_unbound_rejection_authority(),
            session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
            composition_signal_dispatcher: StdRwLock::new(None),
        }
    }

    /// Create a persistent adapter with a RuntimeStore.
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
            sessions: RwLock::new(HashMap::new()),
            store: Some(store),
            blob_store: Some(blob_store),
            llm_reconfigure_host: StdRwLock::new(None),
            auth_lease: StdRwLock::new(auth_lease),
            #[cfg(not(target_arch = "wasm32"))]
            oauth_flows: StdRwLock::new(oauth_flows),
            #[cfg(feature = "live")]
            live_unbound_rejection_authority: live_unbound_rejection_authority(),
            session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
            composition_signal_dispatcher: StdRwLock::new(None),
        }
    }

    /// Create a persistent adapter with a RuntimeStore but no blob store.
    ///
    /// The driver remains persistent for session state. Blob-backed inputs fail
    /// explicitly at the blob-store boundary until a real [`BlobStore`] is
    /// supplied.
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
            sessions: RwLock::new(HashMap::new()),
            store: Some(store),
            blob_store: Some(Arc::new(UnavailableBlobStore)),
            llm_reconfigure_host: StdRwLock::new(None),
            auth_lease: StdRwLock::new(auth_lease),
            #[cfg(not(target_arch = "wasm32"))]
            oauth_flows: StdRwLock::new(oauth_flows),
            #[cfg(feature = "live")]
            live_unbound_rejection_authority: live_unbound_rejection_authority(),
            session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
            composition_signal_dispatcher: StdRwLock::new(None),
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
            match store.load_ops_lifecycle(runtime_id).await {
                Ok(Some(snapshot)) => {
                    let recovered_epoch = snapshot.epoch_id.clone();
                    let recovered_ops_count = snapshot.completion_entries.len();
                    let registry =
                        match crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::from_recovered(
                            snapshot,
                        ) {
                            Ok(registry) => registry,
                            Err(err) => {
                                tracing::error!(
                                    %session_id,
                                    %runtime_id,
                                    error = %err,
                                    "failed to recover ops lifecycle through generated authority"
                                );
                                return Err(RuntimeDriverError::Internal(format!(
                                    "failed to recover ops lifecycle through generated authority: {err}"
                                )));
                            }
                        };
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
                        "ops lifecycle recovered from durable store (same epoch)"
                    );
                    return Ok((
                        Arc::new(registry),
                        recovered_epoch,
                        Arc::new(recovered_cursors),
                    ));
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::error!(
                        %session_id,
                        %runtime_id,
                        error = %err,
                        "failed to load ops lifecycle from durable store"
                    );
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to load ops lifecycle from durable store: {err}"
                    )));
                }
            }
            tracing::debug!(%session_id, "no persisted ops lifecycle; fresh epoch");
            Ok((
                Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                meerkat_core::RuntimeEpochId::new(),
                Arc::new(meerkat_core::EpochCursorState::new()),
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
