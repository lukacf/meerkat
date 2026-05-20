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

use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::{InputId, RunId};
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
    HydratedSessionLlmState, MeerkatAdmittedInputSnapshot, MeerkatBindingSnapshot,
    MeerkatCompletionWaiterSnapshot, MeerkatCompletionWaitersSnapshot, MeerkatControlSnapshot,
    MeerkatCursorSnapshot, MeerkatDrainSnapshot, MeerkatDriverKind, MeerkatFormalStateProjection,
    MeerkatInputsSnapshot, MeerkatLedgerSnapshot, MeerkatMachineCommand,
    MeerkatMachineCommandError, MeerkatMachineCommandResult, MeerkatMachineRunFailure,
    MeerkatMachineRunPrepared, MeerkatMachineSpineSnapshot, MeerkatOpsSnapshot,
    SessionLlmCapabilityDelta, SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus,
    SessionLlmReconfigureHost, SessionLlmReconfigureReport, SessionLlmReconfigureRequest,
    SessionToolVisibilityDelta,
};
use crate::runtime_state::RuntimeState;
use crate::service_ext::{RuntimeMode, SessionServiceRuntimeExt};
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
struct UnavailableOAuthFlowAuthority;

#[cfg(not(target_arch = "wasm32"))]
impl UnavailableOAuthFlowAuthority {
    fn rejected(operation: &'static str) -> meerkat_auth_core::oauth_flow::OAuthFlowError {
        meerkat_auth_core::oauth_flow::OAuthFlowError::LifecycleRejected {
            operation,
            detail: "custom auth lease handle does not provide a runtime OAuth flow authority"
                .to_string(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl meerkat_auth_core::oauth_flow::OAuthFlowAuthority for UnavailableOAuthFlowAuthority {
    fn start(
        &self,
        _target: meerkat_core::AuthBindingRef,
        _provider: meerkat_auth_core::oauth_flow::OAuthProviderIdentity,
        _redirect_uri: String,
        _pkce_verifier: String,
    ) -> Result<String, meerkat_auth_core::oauth_flow::OAuthFlowError> {
        Err(Self::rejected("admit_oauth_browser_flow"))
    }

    fn verify(
        &self,
        _state: &str,
        _target: &meerkat_core::AuthBindingRef,
        _provider: meerkat_auth_core::oauth_flow::OAuthProviderIdentity,
        _redirect_uri: &str,
    ) -> Result<
        meerkat_auth_core::oauth_flow::OAuthFlowRecord,
        meerkat_auth_core::oauth_flow::OAuthFlowError,
    > {
        Err(Self::rejected("verify_oauth_browser_flow"))
    }

    fn consume(
        &self,
        _state: &str,
        _target: &meerkat_core::AuthBindingRef,
        _provider: meerkat_auth_core::oauth_flow::OAuthProviderIdentity,
        _redirect_uri: &str,
    ) -> Result<
        meerkat_auth_core::oauth_flow::OAuthFlowRecord,
        meerkat_auth_core::oauth_flow::OAuthFlowError,
    > {
        Err(Self::rejected("consume_oauth_browser_flow"))
    }

    fn admit_device_code(
        &self,
        _target: meerkat_core::AuthBindingRef,
        _provider: meerkat_auth_core::oauth_flow::OAuthProviderIdentity,
        _device_code: String,
        _expires_in: std::time::Duration,
    ) -> Result<(), meerkat_auth_core::oauth_flow::OAuthFlowError> {
        Err(Self::rejected("admit_oauth_device_flow"))
    }

    fn verify_device_code(
        &self,
        _device_code: &str,
        _target: &meerkat_core::AuthBindingRef,
        _provider: meerkat_auth_core::oauth_flow::OAuthProviderIdentity,
    ) -> Result<
        meerkat_auth_core::oauth_flow::OAuthDeviceFlowRecord,
        meerkat_auth_core::oauth_flow::OAuthFlowError,
    > {
        Err(Self::rejected("verify_oauth_device_flow"))
    }

    fn begin_device_code_poll(
        &self,
        _device_code: &str,
        _target: &meerkat_core::AuthBindingRef,
        _provider: meerkat_auth_core::oauth_flow::OAuthProviderIdentity,
    ) -> Result<
        meerkat_auth_core::oauth_flow::OAuthDevicePollLease,
        meerkat_auth_core::oauth_flow::OAuthFlowError,
    > {
        Err(Self::rejected("begin_oauth_device_poll"))
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
    machine_apply_run_return_projection, machine_batch_primitive_projections,
    machine_batch_runtime_semantics, machine_begin_run, machine_commit_prepared_destroy,
    machine_commit_service_turn_terminal_receipt, machine_input_boundary,
    machine_prepare_bindings_projection, machine_prepare_destroy, machine_recover_ephemeral_driver,
    machine_recover_persistent_driver, machine_recycle_preserving_work, machine_reset,
    machine_retire, machine_select_runtime_loop_batch, machine_stop_runtime,
    prepare_runtime_loop_batch_start, rollback_runtime_loop_run_after_boundary_commit_failure,
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
pub(crate) use comms_drain::{CommsDrainSlot, abort_slot};
pub(crate) use dsl_effects::{DslTransitionEffects, apply_dsl_transition_on_authority};
pub(crate) use visibility::MachineToolVisibilityOwner;

struct StagedSessionDslInput {
    previous_snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    effects: DslTransitionEffects,
}

#[derive(Clone, Copy)]
enum CommittedEffectDispatchFailure {
    PreserveCommittedDslState,
    RestorePreviousDslState,
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
    /// Shared consumer cursor state for the epoch.
    cursor_state: Arc<meerkat_core::EpochCursorState>,
    /// Completion waiters (accessed by accept_input_with_completion and RuntimeLoop).
    completions: SharedCompletionRegistry,
    /// Canonical durable visibility owner for this session.
    tool_visibility_owner: Arc<MachineToolVisibilityOwner>,
    /// Machine-owned current durable/live LLM identity for the registered session.
    current_llm_identity: Option<meerkat_core::SessionLlmIdentity>,
    /// Machine-owned current capability surface for the registered session.
    current_capability_surface: Option<SessionLlmCapabilitySurface>,
    /// Whether the machine has a resolved capability surface for the current identity.
    capability_surface_status: SessionLlmCapabilitySurfaceStatus,
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
    _loop_handle: tokio::task::JoinHandle<()>,
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
            authority.restore_snapshot(staged.previous_snapshot.clone());
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
            _loop_handle: loop_handle,
        });
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
            let Some(entry) = sessions.get(session_id) else {
                return None;
            };
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
            if !entry.generated_executor_registration_active() {
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
            CommittedEffectDispatchFailure::RestorePreviousDslState,
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
        session_id: &SessionId,
        staged: StagedSessionDslInput,
        context: &str,
        dispatch_failure: CommittedEffectDispatchFailure,
    ) -> Result<(), String> {
        if let Err(error) = self
            .dispatch_routed_signals_from_effects(&staged.effects)
            .await
        {
            match dispatch_failure {
                CommittedEffectDispatchFailure::PreserveCommittedDslState => {}
                CommittedEffectDispatchFailure::RestorePreviousDslState => {
                    self.restore_session_dsl_state(session_id, staged.previous_snapshot)
                        .await;
                }
            }
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

    fn restore_dsl_authority_snapshot(
        authority: &Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        snapshot: dsl::MeerkatMachineAuthoritySnapshot,
    ) {
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authority.restore_snapshot(snapshot);
    }
}

/// Capability token proving a session-control mutation is routed through
/// `MeerkatMachine` authority instead of a public store-only service path.
#[derive(Debug, Clone, Copy)]
pub struct MachineSessionControlAuthority {
    _private: (),
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
    /// Runtime mode.
    mode: RuntimeMode,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
    /// Blob store used by persistent drivers for durable input externalization.
    blob_store: Option<Arc<dyn BlobStore>>,
    /// Runtime-owned shell seam for live session LLM reconfiguration I/O.
    llm_reconfigure_host: StdRwLock<Option<Arc<dyn SessionLlmReconfigureHost>>>,
    /// AuthMachine lifecycle authority shared by runtime-backed auth
    /// resolution/refresh paths and public auth-status surfaces.
    auth_lease: StdRwLock<Arc<dyn meerkat_core::handles::AuthLeaseHandle>>,
    /// OAuth login-flow lifecycle authority shared by public auth surfaces
    /// that operate through this runtime adapter.
    #[cfg(not(target_arch = "wasm32"))]
    oauth_flows: StdRwLock<Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority>>,
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
        let auth_lease: Arc<dyn meerkat_core::handles::AuthLeaseHandle> = auth_lease;
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: None,
            blob_store: None,
            llm_reconfigure_host: StdRwLock::new(None),
            auth_lease: StdRwLock::new(auth_lease),
            #[cfg(not(target_arch = "wasm32"))]
            oauth_flows: StdRwLock::new(oauth_flows),
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
        let auth_lease: Arc<dyn meerkat_core::handles::AuthLeaseHandle> = auth_lease;
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: Some(store),
            blob_store: Some(blob_store),
            llm_reconfigure_host: StdRwLock::new(None),
            auth_lease: StdRwLock::new(auth_lease),
            #[cfg(not(target_arch = "wasm32"))]
            oauth_flows: StdRwLock::new(oauth_flows),
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
        let auth_lease: Arc<dyn meerkat_core::handles::AuthLeaseHandle> = auth_lease;
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: Some(store),
            blob_store: Some(Arc::new(UnavailableBlobStore)),
            llm_reconfigure_host: StdRwLock::new(None),
            auth_lease: StdRwLock::new(auth_lease),
            #[cfg(not(target_arch = "wasm32"))]
            oauth_flows: StdRwLock::new(oauth_flows),
            session_claims: Arc::new(crate::handles::RuntimeSessionClaimRegistry::new()),
            composition_signal_dispatcher: StdRwLock::new(None),
        }
    }

    /// Shared auth lifecycle handle used by all runtime-backed session
    /// bindings created by this adapter.
    pub fn auth_lease_handle(&self) -> Arc<dyn meerkat_core::handles::AuthLeaseHandle> {
        Arc::clone(
            &self
                .auth_lease
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Install the auth lifecycle authority that public surfaces also read.
    ///
    /// Surfaces construct the adapter before all state fields are available, so
    /// this setter lets them align the adapter's runtime-backed traffic with
    /// the surface-visible status handle without creating a competing registry.
    pub fn set_auth_lease_handle(&self, handle: Arc<dyn meerkat_core::handles::AuthLeaseHandle>) {
        #[cfg(not(target_arch = "wasm32"))]
        let handle = {
            let erased: Arc<dyn std::any::Any + Send + Sync> = handle.clone();
            if let Ok(runtime_handle) =
                Arc::downcast::<crate::handles::RuntimeAuthLeaseHandle>(erased)
            {
                self.set_runtime_auth_lease_handle(runtime_handle);
                return;
            }
            *self
                .oauth_flows
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Arc::new(UnavailableOAuthFlowAuthority);
            handle
        };
        *self
            .auth_lease
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = handle;
    }

    /// Install a custom credential lifecycle handle together with an explicit
    /// OAuth login-flow authority.
    ///
    /// This is the opt-in seam for tests/embedders that intentionally want a
    /// non-runtime credential handle. The plain setter does not silently create
    /// a second hidden AuthMachine authority for OAuth flow membership.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_auth_lease_handle_with_oauth_flow_authority(
        &self,
        handle: Arc<dyn meerkat_core::handles::AuthLeaseHandle>,
        oauth_flows: Arc<dyn meerkat_auth_core::oauth_flow::OAuthFlowAuthority>,
    ) {
        *self
            .oauth_flows
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = oauth_flows;
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
        let handle: Arc<dyn meerkat_core::handles::AuthLeaseHandle> = handle;
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
    pub async fn apply_routed_meerkat_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
    ) -> Result<(), String> {
        Self::reject_raw_fieldless_runtime_internal_dsl_input(&input)?;
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).ok_or_else(|| {
            format!(
                "session `{session_id}` is not registered with this MeerkatMachine; \
                 cannot deliver routed input"
            )
        })?;
        let authority = Arc::clone(&entry.dsl_authority);
        drop(sessions);

        let (previous_snapshot, effects) = {
            let mut guard = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous_snapshot = guard.snapshot();
            let effects = dsl::MeerkatMachineMutator::apply(&mut *guard, input)
                .map(|transition| transition.into_effects())
                .map_err(|err| format!("{err}"))?;
            (previous_snapshot, effects)
        };
        if let Err(error) = self.dispatch_routed_signals_from_effects(&effects).await {
            self.restore_session_dsl_state(session_id, previous_snapshot)
                .await;
            return Err(format!(
                "routed MeerkatMachine input committed effect dispatch failed: {error}"
            ));
        }
        Ok(())
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
                    tracing::warn!(
                        %session_id,
                        %runtime_id,
                        error = %err,
                        "failed to load ops lifecycle; epoch rotated"
                    );
                    return Ok((
                        Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                        meerkat_core::RuntimeEpochId::new(),
                        Arc::new(meerkat_core::EpochCursorState::new()),
                    ));
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
                | MeerkatMachineCommand::ActivateImageOperationOverride { .. }
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
                MeerkatMachineCommand::Prepare { .. }
                | MeerkatMachineCommand::Commit { .. }
                | MeerkatMachineCommand::Fail { .. } => self
                    .execute_meerkat_machine_legacy_run_command(command)
                    .await
                    .map_err(Into::into),
            }
        })
    }

    /// Register a runtime driver for a session (no RuntimeLoop — inputs queue but
    /// nothing processes them automatically). Useful for tests and legacy mode.
    pub async fn register_session(&self, session_id: SessionId) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RegisterSession { session_id },
            )
            .await;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#[path = "../meerkat_machine_tests.rs"]
mod tests;
