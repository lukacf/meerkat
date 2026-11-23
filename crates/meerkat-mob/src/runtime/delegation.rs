//! Reusable execution composition for one synchronous delegated helper turn.
//!
//! This module owns member construction or borrowing, optional parent wiring,
//! exact work admission, terminal observation, and custody release. Canonical
//! member, work, and terminal state remain owned by MobMachine and the exact
//! turn authorities reached through [`MobHandle`].

use std::sync::Arc;

use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{CommsCommand, PeerId, PeerName, PeerRoute, TrustedPeerDescriptor};
use meerkat_core::interaction::{InteractionId, ObjectiveId};
use meerkat_core::service::SessionServiceCommsExt;
use meerkat_core::types::HandlingMode;
use serde::Serialize;

use crate::ids::{AgentRuntimeId, FenceToken};
use crate::machines::mob_machine::HostId;
use crate::profile::Profile;
use crate::tokio;
use crate::{
    AgentIdentity, MobDeliveryIdentity, MobError, MobRuntimeMode, ProfileName, WorkOrigin, WorkSpec,
};

use super::{
    BoundedResultSpec, BoundedTurnWaitError, ForkMemberAtTurnBoundary, MemberDeliveryScope,
    MobHandle, SpawnMemberSpec, SpawnResult, WorkBoundedTurnResult, WorkDeliveryReceipt,
    WorkTurnHandle,
};

/// Visible best-effort reporting convention for bounded delegated work.
///
/// This is deliberately prompt text rather than a structured-result contract.
/// Callers receive ordinary [`super::BoundedHelperResult`] text and must not
/// infer validation or success beyond its typed terminal/truncation status.
pub const BOUNDED_DELEGATION_REPORT_INSTRUCTION_V1: &str = "MEERKAT_BOUNDED_DELEGATION_REPORT_V1\nAfter doing the requested work, end with a clear, concise report of the work performed and the final result. This is a loose best-effort completion report, not a structured schema or success guarantee.";

/// The profile a fresh delegation helper runs under: the one profile of a
/// session's implicit delegation mob.
pub(crate) const DELEGATION_HELPER_PROFILE: &str = "delegate";

#[must_use]
pub fn render_bounded_delegation_task(task: &str) -> String {
    format!("{task}\n\n{BOUNDED_DELEGATION_REPORT_INSTRUCTION_V1}")
}

#[cfg(feature = "runtime-adapter")]
pub(super) fn render_live_delegation_execution_context(task: &str) -> String {
    format!(
        "Live delegation execution context: execute this already committed voice request \
         (not a new user utterance).\n\n{task}"
    )
}

/// Canonical context source for one delegated worker.
///
/// `DurableFork` persists a real transcript fork through
/// [`MobHandle::fork_member`]. It is intentionally distinct from the legacy
/// prompt-context `MemberLaunchMode::Fork`, which creates a fresh session and
/// renders history into text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DelegationExecutionSource {
    #[default]
    Fresh,
    DurableFork {
        source_identity: AgentIdentity,
        message_count: Option<usize>,
    },
    /// Execute one exact turn without taking ownership of the member lifecycle.
    ExistingMember,
}

/// Mob-owned construction options for one delegated helper member.
///
/// The execution service fixes the delegation profile, turn-driven runtime,
/// explicit work admission, and disabled roster-parent auto-wiring. Callers
/// supply only policy and placement material already resolved by their owning
/// composition boundary.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct DelegationMemberOptions {
    /// Sealed runtime source proof, independent of optional comms wiring.
    pub creation_source: Option<crate::MemberCreationSourceWitness>,
    /// The calling member's seat as a mob's handle sealed it
    /// ([`crate::MobHandle::seal_seat_creator`]), for the helper's seat
    /// record alone.
    pub seat_creator: Option<crate::MemberSeatCreator>,
    pub placement: Option<HostId>,
    pub additional_instructions: Option<Vec<String>>,
    pub inherited_tool_filter: Option<meerkat_core::InheritedToolVisibilityAuthority>,
    pub override_profile: Option<Profile>,
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    pub objective_id: Option<ObjectiveId>,
    /// Expose the realm-scoped WorkGraph tools to the helper even when its
    /// resolved profile does not enable them. Live delegation workers track
    /// their own item in the mob's shared WorkGraph and need this grant.
    pub grant_workgraph_tools: bool,
}

/// Process-local parent comms material used to establish reciprocal trust.
///
/// Absence means delegation still executes but reports `wired = false`,
/// matching helpers created by parents without a comms runtime.
#[derive(Clone)]
pub struct DelegationParentContext {
    comms_name: String,
    peer_id: PeerId,
    comms_runtime: Arc<dyn CommsRuntime>,
}

impl DelegationParentContext {
    #[must_use]
    pub fn new(
        comms_name: impl Into<String>,
        peer_id: PeerId,
        comms_runtime: Arc<dyn CommsRuntime>,
    ) -> Self {
        Self {
            comms_name: comms_name.into(),
            peer_id,
            comms_runtime,
        }
    }
}

/// Typed request for one delegated helper execution.
pub struct DelegationExecutionRequest {
    pub identity: AgentIdentity,
    pub task: String,
    pub result_spec: BoundedResultSpec,
    pub member: DelegationMemberOptions,
    pub parent: Option<DelegationParentContext>,
    source: DelegationExecutionSource,
    delivery_identity: Option<(MobDeliveryIdentity, InteractionId)>,
    live_admission: Option<meerkat_runtime::live_execution::LiveDelegationExecutionAdmission>,
}

impl DelegationExecutionRequest {
    #[must_use]
    pub fn new(
        identity: AgentIdentity,
        task: impl Into<String>,
        result_spec: BoundedResultSpec,
    ) -> Self {
        Self {
            identity,
            task: task.into(),
            result_spec,
            member: DelegationMemberOptions::default(),
            parent: None,
            source: DelegationExecutionSource::Fresh,
            delivery_identity: None,
            live_admission: None,
        }
    }

    /// Construct a machine-admitted live delegation request.
    ///
    /// The admission is sealed by `meerkat-runtime` from the generated
    /// `LiveDelegationAdmitted` effect. This service cannot construct live
    /// execution from copied channel, interaction, or operation identifiers.
    #[must_use]
    pub fn new_live(
        identity: AgentIdentity,
        task: impl Into<String>,
        result_spec: BoundedResultSpec,
        admission: meerkat_runtime::live_execution::LiveDelegationExecutionAdmission,
    ) -> Self {
        Self {
            identity,
            task: task.into(),
            result_spec,
            member: DelegationMemberOptions::default(),
            parent: None,
            source: DelegationExecutionSource::Fresh,
            delivery_identity: None,
            live_admission: Some(admission),
        }
    }

    /// Execute on a real durable fork of `source_identity`.
    ///
    /// `message_count=None` selects the exact committed transcript end while
    /// the persistent session owner holds its mutation guard.
    #[must_use]
    pub fn with_durable_fork(
        mut self,
        source_identity: AgentIdentity,
        message_count: Option<usize>,
    ) -> Self {
        self.source = DelegationExecutionSource::DurableFork {
            source_identity,
            message_count,
        };
        self
    }

    /// Execute on `identity` itself. Requires a live admission whose generated
    /// custody is `ExistingMember`; member construction overrides are rejected.
    #[must_use]
    pub fn with_existing_member(mut self) -> Self {
        self.source = DelegationExecutionSource::ExistingMember;
        self
    }

    #[must_use]
    pub fn source(&self) -> &DelegationExecutionSource {
        &self.source
    }

    /// Bind exact stable delivery authority independently of the generated
    /// live-delegation lifecycle. Callers derive this carrier from their own
    /// sealed operation and interaction identities.
    #[must_use]
    pub fn with_delivery_identity(
        mut self,
        identity: MobDeliveryIdentity,
        interaction_id: InteractionId,
    ) -> Self {
        self.delivery_identity = Some((identity, interaction_id));
        self
    }
}

/// Exact machine-admitted live operation carried through worker terminality.
#[derive(Clone)]
pub struct LiveDelegationTerminalEvidence {
    operation: meerkat_core::ExactOperationIdentity<meerkat_core::LiveUserTurnCorrelation>,
    worker_ownership: meerkat_runtime::live_execution::LiveDelegationWorkerOwnership,
}

impl std::fmt::Debug for LiveDelegationTerminalEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDelegationTerminalEvidence")
            .field("operation_id", self.operation.operation_id())
            .field(
                "interaction_id",
                &self.operation.domain_correlation().interaction_id(),
            )
            .field("provider_correlation", &"[REDACTED]")
            .finish()
    }
}

impl LiveDelegationTerminalEvidence {
    #[must_use]
    pub fn operation(
        &self,
    ) -> &meerkat_core::ExactOperationIdentity<meerkat_core::LiveUserTurnCorrelation> {
        &self.operation
    }
}

/// Terminal outcome for the exact admitted worker turn.
#[derive(Debug)]
#[non_exhaustive]
pub enum DelegationTurnTerminal {
    Completed(WorkBoundedTurnResult),
    Failed(BoundedTurnWaitError<WorkDeliveryReceipt>),
}

/// Worker terminal evidence retained until generated retirement authority.
#[derive(Debug)]
pub struct DelegationTerminalizedExecution {
    spawn: Option<SpawnResult>,
    wired: bool,
    identity: AgentIdentity,
    terminal: DelegationTurnTerminal,
    live: Option<LiveDelegationTerminalEvidence>,
    /// Armed only by [`DelegationExecutionService::execute`]: retires the
    /// helper when the call is dropped before its own retirement.
    abandon_guard: Option<super::handle::ProvisionedChildRetireOnDrop>,
}

impl DelegationTerminalizedExecution {
    #[must_use]
    pub fn spawn(&self) -> Option<&SpawnResult> {
        self.spawn.as_ref()
    }

    #[must_use]
    pub const fn wired(&self) -> bool {
        self.wired
    }

    #[must_use]
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    #[must_use]
    pub fn terminal(&self) -> &DelegationTurnTerminal {
        &self.terminal
    }

    #[must_use]
    pub fn live_terminal_evidence(&self) -> Option<&LiveDelegationTerminalEvidence> {
        self.live.as_ref()
    }
}

/// Started worker lifecycle. Awaiting the turn does not retire the member;
/// generated composition authorizes retirement as a separate effect.
#[must_use = "started delegations must be awaited, cancelled, or deliberately detached"]
pub struct DelegationExecutionHandle {
    service: DelegationExecutionService,
    spawn: Option<SpawnResult>,
    wired: bool,
    identity: AgentIdentity,
    result_spec: BoundedResultSpec,
    turn_handle: WorkTurnHandle,
    live_admission: Option<meerkat_runtime::live_execution::LiveDelegationExecutionAdmission>,
    cancellation_target: DelegationCancellationTarget,
    abandon_guard: Option<super::handle::ProvisionedChildRetireOnDrop>,
}

#[derive(Clone)]
enum DelegationCancellationTarget {
    OwnedMember,
    #[cfg(feature = "runtime-adapter")]
    ExistingInput {
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
        session_id: meerkat_core::SessionId,
        delivery_identity: MobDeliveryIdentity,
    },
}

/// Cloneable exact-worker cancellation endpoint retained while another task
/// awaits terminality.
#[derive(Clone)]
pub struct DelegationCancellationHandle {
    service: DelegationExecutionService,
    identity: AgentIdentity,
    live_admission: meerkat_runtime::live_execution::LiveDelegationExecutionAdmission,
    target: DelegationCancellationTarget,
}

impl DelegationCancellationHandle {
    pub async fn cancel(
        &self,
        authority: &meerkat_runtime::live_execution::LiveDelegationCancellationAuthority,
    ) -> Result<meerkat_runtime::live_execution::LiveDelegationCancellationOutcome, MobError> {
        if authority.session_id() != self.live_admission.session_id()
            || authority.operation() != self.live_admission.operation()
            || authority.worker_identity() != self.live_admission.worker_identity()
            || authority.worker_identity() != self.identity.as_str()
        {
            return Err(MobError::Internal(
                "live cancellation authority does not match the exact worker binding".to_string(),
            ));
        }
        #[cfg(not(feature = "runtime-adapter"))]
        let DelegationCancellationTarget::OwnedMember = &self.target;
        #[cfg(feature = "runtime-adapter")]
        match &self.target {
            DelegationCancellationTarget::OwnedMember => {}
            #[cfg(feature = "runtime-adapter")]
            DelegationCancellationTarget::ExistingInput {
                runtime,
                session_id,
                delivery_identity,
            } => {
                use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
                let stored = runtime
                    .input_state_by_idempotency_key(session_id, &delivery_identity.idempotency_key)
                    .await
                    .map_err(|error| MobError::Internal(error.to_string()))?
                    .ok_or_else(|| {
                        MobError::Internal(
                            "accepted live delegation lost its exact input custody".to_string(),
                        )
                    })?;
                if stored.seed.terminal_outcome.is_some() {
                    return Ok(meerkat_runtime::live_execution::LiveDelegationCancellationOutcome::AlreadyTerminal);
                }
                let present = runtime
                    .cancel_input_if_present(
                        session_id,
                        &stored.state.input_id,
                        "live delegation cancelled",
                    )
                    .await
                    .map_err(|error| MobError::Internal(error.to_string()))?;
                if present {
                    return Ok(meerkat_runtime::live_execution::LiveDelegationCancellationOutcome::Cancelled);
                }
                // Terminal archival can win between the durable identity lookup
                // and the exact-input cancel. Re-read that same identity only.
                let terminal = runtime
                    .input_state_by_idempotency_key(session_id, &delivery_identity.idempotency_key)
                    .await
                    .map_err(|error| MobError::Internal(error.to_string()))?
                    .is_some_and(|row| row.seed.terminal_outcome.is_some());
                return Ok(if terminal {
                    meerkat_runtime::live_execution::LiveDelegationCancellationOutcome::AlreadyTerminal
                } else {
                    meerkat_runtime::live_execution::LiveDelegationCancellationOutcome::Failed
                });
            }
        }
        if self
            .service
            .handle
            .get_member(&self.identity)
            .await?
            .is_none()
        {
            return Ok(
                meerkat_runtime::live_execution::LiveDelegationCancellationOutcome::AlreadyTerminal,
            );
        }
        Ok(
            match self
                .service
                .handle
                .force_cancel_member(self.identity.clone())
                .await
            {
                Ok(()) => {
                    meerkat_runtime::live_execution::LiveDelegationCancellationOutcome::Cancelled
                }
                Err(_) => {
                    meerkat_runtime::live_execution::LiveDelegationCancellationOutcome::Failed
                }
            },
        )
    }
}

impl DelegationExecutionHandle {
    #[must_use]
    pub fn spawn(&self) -> Option<&SpawnResult> {
        self.spawn.as_ref()
    }

    #[must_use]
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    #[must_use]
    pub fn work_receipt(&self) -> &WorkDeliveryReceipt {
        self.turn_handle.receipt()
    }

    #[must_use]
    pub fn live_admission(
        &self,
    ) -> Option<&meerkat_runtime::live_execution::LiveDelegationExecutionAdmission> {
        self.live_admission.as_ref()
    }

    #[must_use]
    pub fn cancellation_handle(&self) -> Option<DelegationCancellationHandle> {
        self.live_admission
            .as_ref()
            .map(|live_admission| DelegationCancellationHandle {
                service: self.service.clone(),
                identity: self.identity.clone(),
                live_admission: live_admission.clone(),
                target: self.cancellation_target.clone(),
            })
    }

    /// Cancel this exact live worker only under machine-minted authority.
    pub async fn cancel(
        &self,
        authority: &meerkat_runtime::live_execution::LiveDelegationCancellationAuthority,
    ) -> Result<meerkat_runtime::live_execution::LiveDelegationCancellationOutcome, MobError> {
        self.cancellation_handle()
            .ok_or_else(|| {
                MobError::Internal(
                    "live cancellation authority cannot cancel a non-live delegation".to_string(),
                )
            })?
            .cancel(authority)
            .await
    }

    pub async fn await_terminal(self) -> DelegationTerminalizedExecution {
        let Self {
            service: _,
            spawn,
            wired,
            identity,
            result_spec,
            turn_handle,
            live_admission,
            cancellation_target: _,
            abandon_guard,
        } = self;
        let terminal = match turn_handle.wait_bounded(result_spec).await {
            Ok(turn) => DelegationTurnTerminal::Completed(turn),
            Err(error) => DelegationTurnTerminal::Failed(error),
        };
        DelegationTerminalizedExecution {
            spawn,
            wired,
            identity,
            terminal,
            live: live_admission.map(|admission| LiveDelegationTerminalEvidence {
                operation: admission.operation().clone(),
                worker_ownership: admission.worker_ownership(),
            }),
            abandon_guard,
        }
    }
}

/// Successful exact delegation result plus independent cleanup debt.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct DelegationExecutionOutcome {
    spawn: SpawnResult,
    wired: bool,
    turn: WorkBoundedTurnResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retirement_error: Option<String>,
    #[serde(skip)]
    live: Option<LiveDelegationTerminalEvidence>,
}

impl DelegationExecutionOutcome {
    #[must_use]
    pub fn spawn(&self) -> &SpawnResult {
        &self.spawn
    }

    #[must_use]
    pub const fn wired(&self) -> bool {
        self.wired
    }

    #[must_use]
    pub fn turn(&self) -> &WorkBoundedTurnResult {
        &self.turn
    }

    #[must_use]
    pub fn retirement_error(&self) -> Option<&str> {
        self.retirement_error.as_deref()
    }

    #[must_use]
    pub fn live_terminal_evidence(&self) -> Option<&LiveDelegationTerminalEvidence> {
        self.live.as_ref()
    }
}

/// Failure stage for one delegated helper execution.
///
/// Work failures retain retirement debt separately, so a surface can preserve
/// the existing exact error contract without parsing text.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DelegationExecutionError {
    #[error("live delegation requires explicit generated lifecycle realization")]
    LiveLifecycleRequired,
    #[error(
        "existing-member delegation requires exact runtime-backed live admission without member construction overrides"
    )]
    ExistingMemberAdmissionRequired,
    #[error("delegated helper spawn failed: {0}")]
    Spawn(#[source] MobError),
    #[error(
        "delegation source member {source_identity} was still running a turn after {waited_ms} ms; \
         the durable fork needs its committed turn boundary"
    )]
    SourceBusy {
        source_identity: AgentIdentity,
        waited_ms: u64,
    },
    #[error("delegated helper work admission failed: {error}")]
    WorkAdmission {
        #[source]
        error: MobError,
        retirement_error: Option<MobError>,
    },
    #[error("delegated helper turn failed: {error}")]
    Turn {
        #[source]
        error: BoundedTurnWaitError<WorkDeliveryReceipt>,
        retirement_error: Option<MobError>,
    },
    /// The run limit of [`DelegationExecutionService::execute_within`]
    /// elapsed before the helper's turn reached a terminal. The delegation
    /// ended its helper; `ended` says what became of it.
    #[error("delegated helper did not finish within {max_run:?}: {ended}")]
    MaxRunElapsed {
        max_run: std::time::Duration,
        ended: DelegationHelperEnded,
    },
    /// The task that owned a run of
    /// [`DelegationExecutionService::execute_within`] ended without
    /// answering (it unwound, or its runtime is shutting down). A helper it
    /// had seated is ended by the task's own guard, without the caller:
    /// what became of it is not known here.
    #[error(
        "the delegation's run was lost before it answered; what became of its helper is not \
         known"
    )]
    RunLost,
}

/// How long a run that was ended (its limit elapsed, or its caller stopped
/// listening) waits, after the cancel, for the helper's turn to show that it
/// had completed before the cancel landed. Such a turn shows at once. The
/// cancel takes effect at the run's next boundary, so a run that is inside a
/// model call or a tool call shows no terminal here; the retirement ends it.
const ENDED_RUN_TERMINAL_REREAD: std::time::Duration = std::time::Duration::from_millis(250);

/// How long the retirement of a helper whose run was ended is retried.
const ENDED_RUN_RETIRE_RETRY: std::time::Duration = std::time::Duration::from_secs(10);

/// The pause between two attempts of that retirement.
const ENDED_RUN_RETIRE_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

/// What became of a helper whose delegation ended without its turn's
/// terminal result: every value the `helper` field of a `delegate` answer
/// carries. [`DelegationExecutionService::execute_within`] reports the ends
/// of a run that reached its limit; a surface names the others for the
/// failures it answers itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DelegationHelperEnd {
    /// Its run was cancelled and it was retired.
    CancelledAndRetired,
    /// Its turn failed, or was not admitted, and it was retired.
    Retired,
    /// The incarnation this delegation seated was no longer the member's
    /// current one (retired or replaced by another hand). Nothing of a
    /// later incarnation was cancelled or retired.
    AlreadyGone,
    /// No helper was seated: the call's wait was used up before the spawn.
    /// There is nothing to cancel or retire.
    NotSeated,
    /// Its mob takes no commands any more (it completed, was destroyed or
    /// was shut down), so it could not be ended.
    MobNotRunning,
    /// Its retirement failed. It stays a member until it is retired.
    RetirementFailed,
    /// Ending it had not finished when the caller had to be answered: its
    /// spawn or its turn's admission was still in flight when the run limit
    /// elapsed, or its cancel and retirement took longer than the caller
    /// waits for them. The run's own task goes on and ends it without the
    /// caller; how that went is logged.
    EndPending,
    /// The run's task was lost before it answered. If a helper was seated,
    /// its end goes on without the caller; what became of it is not known.
    Unknown,
}

impl DelegationHelperEnd {
    /// Every end, in the order a tool's description lists them. A slice: a
    /// value added later changes its length, not its type.
    pub const ALL: &[Self] = &[
        Self::CancelledAndRetired,
        Self::Retired,
        Self::AlreadyGone,
        Self::NotSeated,
        Self::EndPending,
        Self::MobNotRunning,
        Self::RetirementFailed,
        Self::Unknown,
    ];

    /// The stable name of this end, as a tool answer carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CancelledAndRetired => "cancelled_and_retired",
            Self::Retired => "retired",
            Self::AlreadyGone => "already_gone",
            Self::NotSeated => "not_seated",
            Self::MobNotRunning => "mob_not_running",
            Self::RetirementFailed => "retirement_failed",
            Self::EndPending => "end_pending",
            Self::Unknown => "unknown",
        }
    }
}

/// What the scoped retire of the helper a run seated came to.
#[derive(Debug)]
enum SeatedRetire {
    /// The incarnation the run seated was retired.
    Retired,
    /// MobMachine holds another incarnation under the helper's identity, or
    /// none: nothing of this run's is left to retire.
    NotCurrent,
    /// The retire, or the read of the delivery scope it acts under, failed.
    Failed(MobError),
}

/// The refusal a shared lifecycle failure carries, or the error itself.
fn lifecycle_refusal(error: &MobError) -> &MobError {
    match error {
        MobError::SharedRetirementFailure(inner) | MobError::SharedLifecycleFailure(inner) => {
            lifecycle_refusal(inner)
        }
        other => other,
    }
}

/// Whether `error` says that the incarnation a fenced command named is not
/// the member's current one: the member is gone, or another incarnation
/// holds its identity.
fn names_another_incarnation(error: &MobError) -> bool {
    matches!(
        lifecycle_refusal(error),
        MobError::MemberNotFound(_)
            | MobError::StaleFenceToken { .. }
            | MobError::StaleDeliveryScope { .. }
            | MobError::StaleMemberOperatorAuthority { .. }
            | MobError::IncarnationMismatch { .. }
    )
}

/// Whether `error` says that the mob no longer runs.
fn mob_no_longer_runs(error: &MobError) -> bool {
    matches!(
        lifecycle_refusal(error),
        MobError::InvalidTransition { .. } | MobError::ActorCommandChannelClosed
    )
}

/// How a delegation ended its helper once the run had ended without the
/// helper's terminal result: the answers of the fenced cancel and of the
/// scoped retire, classified.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DelegationHelperEnded {
    /// What became of the helper.
    pub end: DelegationHelperEnd,
    /// The cancel's failure, when it failed for a reason other than the
    /// incarnation being gone or the mob no longer running.
    pub cancel_error: Option<String>,
    /// The retirement's failure, when [`DelegationHelperEnd::RetirementFailed`].
    pub retirement_error: Option<String>,
}

impl DelegationHelperEnded {
    /// Classify the answers of the fenced cancel and the scoped retire of
    /// one helper incarnation.
    fn of(cancel: Result<(), MobError>, retire: SeatedRetire) -> Self {
        let cancel_names_another = cancel.as_ref().is_err_and(names_another_incarnation);
        let cancel_error = cancel
            .err()
            .filter(|error| !names_another_incarnation(error) && !mob_no_longer_runs(error))
            .map(|error| error.to_string());
        let (end, retirement_error) = match retire {
            SeatedRetire::Retired if cancel_names_another => {
                (DelegationHelperEnd::AlreadyGone, None)
            }
            SeatedRetire::Retired => (DelegationHelperEnd::CancelledAndRetired, None),
            SeatedRetire::NotCurrent => (DelegationHelperEnd::AlreadyGone, None),
            SeatedRetire::Failed(error) if mob_no_longer_runs(&error) => {
                (DelegationHelperEnd::MobNotRunning, None)
            }
            SeatedRetire::Failed(error) => (
                DelegationHelperEnd::RetirementFailed,
                Some(error.to_string()),
            ),
        };
        Self {
            end,
            cancel_error,
            retirement_error,
        }
    }

    /// The helper's end had not finished when the caller had to be
    /// answered (see [`DelegationHelperEnd::EndPending`]).
    fn pending() -> Self {
        Self {
            end: DelegationHelperEnd::EndPending,
            cancel_error: None,
            retirement_error: None,
        }
    }
}

impl std::fmt::Display for DelegationHelperEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.end {
            DelegationHelperEnd::CancelledAndRetired => {
                f.write_str("the helper was cancelled and retired")
            }
            DelegationHelperEnd::AlreadyGone => f.write_str(
                "the helper was already gone (retired or replaced by another hand); nothing \
                 else was cancelled or retired",
            ),
            DelegationHelperEnd::MobNotRunning => f.write_str(
                "the helper's mob no longer takes commands, so the helper could not be ended",
            ),
            DelegationHelperEnd::RetirementFailed => {
                f.write_str("the helper could not be retired")?;
                match self.retirement_error.as_deref() {
                    Some(error) => write!(f, " ({error}); it is still a member"),
                    None => f.write_str("; it is still a member"),
                }
            }
            DelegationHelperEnd::EndPending => f.write_str(
                "ending the helper had not finished when the call had to answer; it goes on \
                 without the caller",
            ),
            DelegationHelperEnd::Retired => f.write_str("the helper was retired"),
            DelegationHelperEnd::NotSeated => f.write_str("no helper was seated"),
            DelegationHelperEnd::Unknown => f.write_str("what became of the helper is not known"),
        }
    }
}

/// The bounds of one run of [`DelegationExecutionService::execute_within`],
/// and the member its call was made for.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DelegationRunBounds {
    /// How long the caller waits for the helper's result, counted from the
    /// call.
    pub max_run: std::time::Duration,
    /// How long the caller of a run that ended without the helper's result
    /// waits, after `max_run`, for the helper's end before it is answered
    /// [`DelegationHelperEnd::EndPending`].
    pub end_within: std::time::Duration,
    /// The member the call was made for, as the mob's member tool audience
    /// is asked whether it hides the helper from it; `None` for a call made
    /// by no member of the mob.
    pub caller: Option<AgentIdentity>,
}

impl DelegationRunBounds {
    #[must_use]
    pub const fn new(max_run: std::time::Duration, end_within: std::time::Duration) -> Self {
        Self {
            max_run,
            end_within,
            caller: None,
        }
    }

    /// Name the member the call was made for.
    #[must_use]
    pub fn for_caller(mut self, caller: Option<AgentIdentity>) -> Self {
        self.caller = caller;
        self
    }
}

/// The failure of [`DelegationExecutionService::execute_within`], with what
/// was read of the helper the call seated while it was seated.
#[derive(Debug)]
#[non_exhaustive]
pub struct DelegationRunFailure {
    /// Why the delegation has no result.
    pub error: DelegationExecutionError,
    /// The session the helper ran on. `None` when no helper was seated, or
    /// when its session could not be read while it was seated.
    pub helper_session_id: Option<meerkat_core::SessionId>,
    /// Whether the mob's member tool audience hides the helper from the
    /// member the call was made for, asked when the helper was seated, with
    /// the labels of its roster row. `None` when no helper was seated: the
    /// audience was not asked.
    pub helper_hidden_from_caller: Option<bool>,
}

#[cfg(any(test, feature = "test-support"))]
impl DelegationRunFailure {
    /// The failure of a run that ended at its limit, as a surface's test
    /// hands it to the surface's answer. Test builds only.
    #[doc(hidden)]
    #[must_use]
    pub fn max_run_elapsed_for_test(
        end: DelegationHelperEnd,
        retirement_error: Option<String>,
        helper_session_id: Option<meerkat_core::SessionId>,
        helper_hidden_from_caller: Option<bool>,
    ) -> Self {
        Self {
            error: DelegationExecutionError::MaxRunElapsed {
                max_run: std::time::Duration::from_secs(1),
                ended: DelegationHelperEnded {
                    end,
                    cancel_error: None,
                    retirement_error,
                },
            },
            helper_session_id,
            helper_hidden_from_caller,
        }
    }
}

/// The helper one run seated, as that run's own spawn named it: the
/// selectors of the fenced cancel and of the scoped retire. Nothing else
/// identifies what the run may end.
#[derive(Clone)]
struct SeatedHelper {
    runtime_id: AgentRuntimeId,
    fence_token: FenceToken,
    /// The audience's answer for the helper, once it was asked (see
    /// [`DelegationRunFailure::helper_hidden_from_caller`]).
    hidden_from_caller: Option<bool>,
    /// The helper's session, from the last delivery scope read that named
    /// this incarnation.
    session_id: Option<meerkat_core::SessionId>,
}

/// What one bounded run's own spawn named, written by the run's `start` the
/// moment the spawn answered, so that it is known to the run's task and to
/// the guard that ends the helper of a lost run whatever `start` does next.
/// It is no lifecycle state: what the helper is, is MobMachine's to say.
struct RunSeat {
    caller: Option<AgentIdentity>,
    seated: std::sync::Mutex<Option<SeatedHelper>>,
}

impl RunSeat {
    fn new(caller: Option<AgentIdentity>) -> Self {
        Self {
            caller,
            seated: std::sync::Mutex::new(None),
        }
    }

    fn seat(&self, spawn: &SpawnResult) {
        *self
            .seated
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(SeatedHelper {
            runtime_id: spawn.agent_runtime_id.clone(),
            fence_token: spawn.fence_token,
            hidden_from_caller: None,
            session_id: None,
        });
    }

    fn note(&self, note: impl FnOnce(&mut SeatedHelper)) {
        if let Some(seated) = self
            .seated
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            note(seated);
        }
    }

    fn seated(&self) -> Option<SeatedHelper> {
        self.seated
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Ends the helper of a bounded run whose task ended without settling it:
/// the task unwound, or was dropped. Armed for the whole run; the fenced
/// cancel and the scoped retire it issues name the incarnation the run's own
/// spawn returned, as the run itself would.
struct EndHelperOfLostRun {
    service: Option<DelegationExecutionService>,
    identity: AgentIdentity,
    seat: Arc<RunSeat>,
    /// The run's owner lease (see [`DelegationExecutionService::own_run`]):
    /// held for the task that ends the helper of a lost run, and by nothing
    /// else of the guard.
    owner: Option<MobHandle>,
}

impl EndHelperOfLostRun {
    fn arm(
        service: DelegationExecutionService,
        identity: AgentIdentity,
        seat: Arc<RunSeat>,
        owner: Option<MobHandle>,
    ) -> Self {
        Self {
            service: Some(service),
            identity,
            seat,
            owner,
        }
    }

    /// The run settled its helper itself.
    fn disarm(mut self) {
        self.service = None;
    }
}

impl Drop for EndHelperOfLostRun {
    fn drop(&mut self) {
        let Some(service) = self.service.take() else {
            return;
        };
        let Some(seated) = self.seat.seated() else {
            return;
        };
        let identity = self.identity.clone();
        let seat = Arc::clone(&self.seat);
        let owner = self.owner.take();
        let end = async move {
            // The mob stays owned until the helper of the lost run is ended.
            let _owner = owner;
            let cleanup = service.handle.bounded_child_cleanup_authority();
            let cancel = cleanup
                .cancel_all_work(seated.runtime_id.clone(), seated.fence_token)
                .await;
            let retire = service
                .retire_seated(&cleanup, &identity, &seat, &seated)
                .await;
            let ended = DelegationHelperEnded::of(cancel, retire);
            tracing::warn!(
                mob_id = %service.handle.mob_id(),
                member = %identity,
                helper = ended.end.as_str(),
                cancel_error = ?ended.cancel_error,
                retirement_error = ?ended.retirement_error,
                "a delegation's run was lost before it settled; its helper was ended"
            );
        };
        // The browser runtime has one local executor and no runtime handle.
        #[cfg(target_arch = "wasm32")]
        {
            tokio::spawn(end);
        }
        #[cfg(not(target_arch = "wasm32"))]
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(end);
            }
            Err(_) => tracing::warn!(
                member = %self.identity,
                "the helper of a lost delegation run could not be ended: no async runtime at drop"
            ),
        }
    }
}

/// The answer one bounded run owes its caller.
type RunAnswer = Result<DelegationExecutionOutcome, DelegationRunFailure>;

/// Resolves when the caller of a run stopped listening; never, once the
/// caller was answered.
async fn caller_gone(caller: &mut Option<tokio::sync::oneshot::Sender<RunAnswer>>) {
    match caller.as_mut() {
        Some(caller) => caller.closed().await,
        None => std::future::pending().await,
    }
}

/// Mob-owned reusable executor for one synchronous delegated helper turn.
#[derive(Clone)]
pub struct DelegationExecutionService {
    handle: MobHandle,
    #[cfg(test)]
    source_turn_boundary_wait_override: Arc<std::sync::Mutex<Option<std::time::Duration>>>,
}

impl DelegationExecutionService {
    /// How long a delegation waits for a busy source member's turn boundary
    /// (and, once won, the recovery gate) before reporting `SourceBusy`. Long
    /// enough for a tool round to finish, short enough that a voice caller
    /// hears a truthful busy answer. The fork's store IO is outside the
    /// bound.
    pub const SOURCE_TURN_BOUNDARY_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

    fn source_turn_boundary_wait(&self) -> std::time::Duration {
        #[cfg(test)]
        if let Some(wait) = *self
            .source_turn_boundary_wait_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return wait;
        }
        Self::SOURCE_TURN_BOUNDARY_WAIT
    }

    /// Test-only: shorten the busy-source wait so a never-released source
    /// reports `SourceBusy` quickly.
    #[cfg(test)]
    pub(crate) fn with_source_turn_boundary_wait(self, wait: std::time::Duration) -> Self {
        *self
            .source_turn_boundary_wait_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(wait);
        self
    }

    #[must_use]
    pub fn new(handle: MobHandle) -> Self {
        Self {
            handle,
            #[cfg(test)]
            source_turn_boundary_wait_override: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Start one delegation through existing MobMachine and exact-turn
    /// authorities. Worker terminality and retirement remain separate.
    #[allow(clippy::result_large_err)]
    pub async fn start(
        &self,
        request: DelegationExecutionRequest,
    ) -> Result<DelegationExecutionHandle, DelegationExecutionError> {
        Box::pin(self.start_owning(request, false, None)).await
    }

    /// [`Self::start`]. With `retire_abandoned`, the started helper is
    /// retired on a detached task when its handle (or the terminal made
    /// from it) is dropped still armed: the caller owns the helper's whole
    /// life and disarms it after its own retirement.
    ///
    /// A bounded run instead supplies `seat` with `retire_abandoned` false:
    /// its run owner observes the exact spawn before further work and owns
    /// the helper's cancellation and retirement itself.
    #[allow(clippy::result_large_err)]
    async fn start_owning(
        &self,
        request: DelegationExecutionRequest,
        retire_abandoned: bool,
        seat: Option<&RunSeat>,
    ) -> Result<DelegationExecutionHandle, DelegationExecutionError> {
        tracing::debug!("delegation service received a bounded execution request");
        let DelegationExecutionRequest {
            identity,
            task,
            result_spec,
            member,
            parent,
            source,
            delivery_identity,
            live_admission,
        } = request;

        let existing_member = matches!(source, DelegationExecutionSource::ExistingMember);
        if live_admission.as_ref().is_some_and(|admission| {
            (admission.worker_ownership()
                == meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember)
                != existing_member
        }) {
            return Err(DelegationExecutionError::ExistingMemberAdmissionRequired);
        }

        if let Some(admission) = live_admission.as_ref()
            && admission.worker_identity() != identity.as_str()
        {
            return Err(DelegationExecutionError::WorkAdmission {
                error: MobError::Internal(
                    "live worker start authority does not match the requested member identity"
                        .to_string(),
                ),
                retirement_error: None,
            });
        }
        let live_delivery_identity = live_admission
            .as_ref()
            .map(|admission| {
                let operation = admission.operation();
                crate::store::MobDeliveryIdentity::new(
                    operation.operation_id().to_string(),
                    operation.domain_correlation().interaction_id().to_string(),
                )
                .map_err(|error| DelegationExecutionError::WorkAdmission {
                    error: MobError::Internal(format!(
                        "live delegation delivery identity rejected: {error}"
                    )),
                    retirement_error: None,
                })
            })
            .transpose()?;
        let explicit_delivery_identity = delivery_identity
            .as_ref()
            .map(|(identity, interaction_id)| {
                identity
                    .validate()
                    .map_err(|error| DelegationExecutionError::WorkAdmission {
                        error: MobError::Internal(format!(
                            "delegation delivery identity rejected: {error}"
                        )),
                        retirement_error: None,
                    })?;
                if identity.correlation_id != interaction_id.to_string() {
                    return Err(DelegationExecutionError::WorkAdmission {
                        error: MobError::Internal(
                            "delegation delivery identity does not match its typed interaction"
                                .to_string(),
                        ),
                        retirement_error: None,
                    });
                }
                Ok(identity.clone())
            })
            .transpose()?;
        if live_delivery_identity.is_some() && explicit_delivery_identity.is_some() {
            return Err(DelegationExecutionError::WorkAdmission {
                error: MobError::Internal(
                    "delegation cannot carry both live and explicit delivery authority".to_string(),
                ),
                retirement_error: None,
            });
        }
        let exact_delivery_identity = live_delivery_identity.or(explicit_delivery_identity);
        let delivery_interaction_id = live_admission
            .as_ref()
            .map(meerkat_runtime::live_execution::LiveDelegationExecutionAdmission::interaction_id)
            .or_else(|| {
                delivery_identity
                    .as_ref()
                    .map(|(_, interaction_id)| *interaction_id)
            });

        if existing_member {
            #[cfg(not(feature = "runtime-adapter"))]
            return Err(DelegationExecutionError::ExistingMemberAdmissionRequired);
            #[cfg(feature = "runtime-adapter")]
            {
                let admission = live_admission
                    .ok_or(DelegationExecutionError::ExistingMemberAdmissionRequired)?;
                admission
                    .require_released_execution()
                    .map_err(|_| DelegationExecutionError::ExistingMemberAdmissionRequired)?;
                if parent.is_some()
                    || member.placement.is_some()
                    || member.additional_instructions.is_some()
                    || member.inherited_tool_filter.is_some()
                    || member.override_profile.is_some()
                    || member.tool_access_policy.is_some()
                    || member.objective_id.is_some()
                {
                    return Err(DelegationExecutionError::ExistingMemberAdmissionRequired);
                }
                let runtime = self
                    .handle
                    .runtime_adapter
                    .clone()
                    .ok_or(DelegationExecutionError::ExistingMemberAdmissionRequired)?;
                let (runtime_id, fence_token) = self
                    .handle
                    .resolve_submit_work_runtime_binding(
                        &identity,
                        WorkOrigin::Internal,
                        "existing live delegation",
                    )
                    .await
                    .map_err(DelegationExecutionError::Spawn)?;
                let session_id = self
                    .handle
                    .resolve_bridge_session_id(&identity)
                    .await
                    .ok_or(DelegationExecutionError::ExistingMemberAdmissionRequired)?;
                if &session_id != admission.session_id() {
                    return Err(DelegationExecutionError::ExistingMemberAdmissionRequired);
                }
                let delivery_identity = exact_delivery_identity
                    .ok_or(DelegationExecutionError::ExistingMemberAdmissionRequired)?;
                let work = WorkSpec::new(
                    render_live_delegation_execution_context(&task),
                    WorkOrigin::Internal,
                )
                .with_interaction_id(admission.interaction_id())
                .with_request_reasoning(admission.member_turn_reasoning());
                let turn_handle = self
                    .handle
                    .start_runtime_work_with_delivery_identity_bounded(
                        runtime_id,
                        fence_token,
                        work,
                        delivery_identity.clone(),
                        result_spec.clone(),
                        crate::mob_machine::WorkContentAttribution::InjectedExecutionContext,
                    )
                    .await
                    .map_err(|error| DelegationExecutionError::WorkAdmission {
                        error,
                        retirement_error: None,
                    })?;
                return Ok(DelegationExecutionHandle {
                    service: self.clone(),
                    spawn: None,
                    wired: false,
                    identity,
                    result_spec,
                    turn_handle,
                    live_admission: Some(admission),
                    cancellation_target: DelegationCancellationTarget::ExistingInput {
                        runtime,
                        session_id,
                        delivery_identity,
                    },
                    abandon_guard: None,
                });
            }
        }

        tracing::debug!(
            delegated_identity = %identity,
            durable_fork = matches!(&source, DelegationExecutionSource::DurableFork { .. }),
            "delegation service resolving member role"
        );
        let role = match &source {
            DelegationExecutionSource::ExistingMember => {
                return Err(DelegationExecutionError::ExistingMemberAdmissionRequired);
            }
            DelegationExecutionSource::Fresh => ProfileName::from(DELEGATION_HELPER_PROFILE),
            DelegationExecutionSource::DurableFork {
                source_identity, ..
            } => {
                let roster = self.handle.roster().await;
                roster
                    .get_by_identity(source_identity)
                    .map(|entry| entry.role.clone())
                    .ok_or_else(|| {
                        DelegationExecutionError::Spawn(MobError::ForkSourceUnavailable {
                            source_member_id: source_identity.to_string(),
                            cause: crate::error::ForkSourceUnavailableCause::NoSession,
                        })
                    })?
            }
        };
        tracing::debug!(
            delegated_identity = %identity,
            "delegation service resolved member role"
        );
        let mut spec = SpawnMemberSpec::new(role, identity.clone());
        spec.creation_origin = crate::member_creation::MemberCreationOrigin::Source(
            member
                .creation_source
                .unwrap_or_else(crate::MemberCreationSourceWitness::unavailable),
        );
        spec.seat.creator = member.seat_creator;
        spec.initial_message = None;
        spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
        spec.auto_wire_parent = false;
        spec.placement = member.placement;
        spec.additional_instructions = member.additional_instructions;
        spec.inherited_tool_filter = member.inherited_tool_filter;
        spec.override_profile = member.override_profile;
        spec.tool_access_policy = member.tool_access_policy;
        spec.objective_id = member.objective_id;
        if member.grant_workgraph_tools {
            spec.tool_category_overrides.workgraph = meerkat_core::ToolCategoryOverride::Enable;
        }
        spec.tool_dispatch_admission = live_admission
            .as_ref()
            .map(meerkat_runtime::live_execution::LiveDelegationExecutionAdmission::tool_dispatch_admission);

        let spawn = match source {
            DelegationExecutionSource::ExistingMember => {
                return Err(DelegationExecutionError::ExistingMemberAdmissionRequired);
            }
            DelegationExecutionSource::Fresh => self
                .handle
                .spawn_spec(spec)
                .await
                .map_err(DelegationExecutionError::Spawn)?,
            DelegationExecutionSource::DurableFork {
                source_identity,
                message_count,
            } => {
                // Reuse the durable-fork constituent of
                // `fork_member_then_run_bounded`, while retaining a started
                // turn handle so generated callers can keep terminal custody
                // and retirement as separate lifecycle steps.
                tracing::debug!(
                    delegated_identity = %identity,
                    source_identity = %source_identity,
                    "delegation service starting durable fork"
                );
                // A live delegation arrives while the source member may be
                // mid-turn (the backing agent is busy). The durable fork
                // requires a committed turn boundary, and the live channel
                // does not own the member's turn, so `CallerTurn` admission
                // would not be honest here. Instead the fork owner waits,
                // bounded, for the running turn to reach its finalization
                // boundary and cuts the branch while still holding it, so a
                // runtime lap queued behind the wait cannot take the gate
                // first. The bound covers the waits, not the fork's store
                // IO. A still-running source after the bound is a typed busy
                // result, not an immediate refusal. Live callers pass
                // `message_count` as the count committed before their own
                // spoken turn, so the child branches at that pre-turn
                // boundary whatever the source committed meanwhile.
                let fork = match self
                    .handle
                    .fork_member_at_turn_boundary(
                        &source_identity,
                        spec,
                        message_count,
                        self.source_turn_boundary_wait(),
                    )
                    .await
                    .map_err(DelegationExecutionError::Spawn)?
                {
                    ForkMemberAtTurnBoundary::Forked(fork) => fork,
                    ForkMemberAtTurnBoundary::SourceBusy { waited } => {
                        return Err(DelegationExecutionError::SourceBusy {
                            source_identity,
                            waited_ms: waited.as_millis() as u64,
                        });
                    }
                };
                tracing::debug!(
                    delegated_identity = %identity,
                    "delegation service completed durable fork"
                );
                SpawnResult::new(fork.agent_identity, fork.agent_runtime_id, fork.fence_token)
            }
        };
        if let Some(seat) = seat {
            self.observe_seat(seat, &spawn).await;
        }

        // From the helper's spawn on, a call that owns the helper's whole
        // life and is dropped before its own retirement retires the helper.
        let abandon_guard = retire_abandoned.then(|| {
            super::handle::ProvisionedChildRetireOnDrop::arm(self.handle.clone(), identity.clone())
        });

        let wired = match parent.as_ref() {
            Some(parent) => self.wire_parent(&identity, parent).await,
            None => false,
        };

        let mut work = WorkSpec::new(task, WorkOrigin::Internal).with_request_reasoning(
            live_admission
                .as_ref()
                .and_then(|admission| admission.member_turn_reasoning()),
        );
        if let Some(objective_id) = member.objective_id {
            work = work.with_objective_id(objective_id);
        }
        if let Some(interaction_id) = delivery_interaction_id {
            work = work.with_interaction_id(interaction_id);
        }
        tracing::debug!(
            delegated_identity = %identity,
            "delegation service admitting bounded work turn"
        );
        let turn_result = if let Some(delivery_identity) = exact_delivery_identity {
            self.handle
                .start_work_for_identity_with_delivery_identity_bounded(
                    identity.clone(),
                    work,
                    HandlingMode::Queue,
                    delivery_identity,
                    result_spec.clone(),
                )
                .await
        } else {
            self.handle
                .start_work_for_identity_bounded(
                    identity.clone(),
                    work,
                    HandlingMode::Queue,
                    result_spec.clone(),
                )
                .await
        };
        tracing::debug!(
            delegated_identity = %identity,
            accepted = turn_result.is_ok(),
            "delegation service resolved bounded work admission"
        );
        let turn_handle = match turn_result {
            Ok(turn) => turn,
            Err(error) => {
                // A live worker is already bound into the generated
                // delegation lifecycle. Its failed-start cleanup remains
                // pending until the machine emits exact retirement authority.
                // A helper that is gone already (another hand retired it
                // before its turn was admitted) leaves no retirement debt.
                let retirement_error = if live_admission.is_some() {
                    None
                } else {
                    self.handle
                        .retire(identity)
                        .await
                        .err()
                        .filter(|error| !names_another_incarnation(error))
                };
                // The retirement was asked for here; its debt is reported.
                if let Some(guard) = abandon_guard {
                    guard.disarm();
                }
                return Err(DelegationExecutionError::WorkAdmission {
                    error,
                    retirement_error,
                });
            }
        };
        Ok(DelegationExecutionHandle {
            service: self.clone(),
            spawn: Some(spawn),
            wired,
            identity,
            result_spec,
            turn_handle,
            live_admission,
            cancellation_target: DelegationCancellationTarget::OwnedMember,
            abandon_guard,
        })
    }

    /// Retire a terminalized worker after generated composition authorizes
    /// that distinct lifecycle edge.
    pub async fn retire_terminalized(
        &self,
        terminal: &DelegationTerminalizedExecution,
    ) -> Result<(), MobError> {
        if terminal.live_terminal_evidence().is_some() {
            return Err(MobError::Internal(
                "live worker retirement requires generated retirement authority".to_string(),
            ));
        }
        self.handle.retire(terminal.identity.clone()).await
    }

    /// Release an exact terminal worker under generated retirement authority.
    /// Existing-member executions release only turn custody, never the member.
    pub async fn retire_live_terminalized(
        &self,
        terminal: &DelegationTerminalizedExecution,
        authority: &meerkat_runtime::live_execution::LiveDelegationWorkerRetirementAuthority,
    ) -> Result<(), MobError> {
        let Some(evidence) = terminal.live_terminal_evidence() else {
            return Err(MobError::Internal(
                "live retirement authority cannot retire a non-live delegation".to_string(),
            ));
        };
        if authority.operation() != evidence.operation()
            || authority.worker_identity() != terminal.identity.as_str()
        {
            return Err(MobError::Internal(
                "live retirement authority does not match the exact terminal worker".to_string(),
            ));
        }
        if evidence.worker_ownership
            == meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember
        {
            return Ok(());
        }
        self.handle.retire(terminal.identity.clone()).await
    }

    /// Retire a worker whose live turn failed before a terminal handle could
    /// be returned. Generated start resolution must classify the failure and
    /// mint this exact retirement authority first.
    pub async fn retire_live_failed_start(
        &self,
        admission: &meerkat_runtime::live_execution::LiveDelegationExecutionAdmission,
        authority: &meerkat_runtime::live_execution::LiveDelegationWorkerRetirementAuthority,
    ) -> Result<(), MobError> {
        if authority.operation() != admission.operation()
            || authority.worker_identity() != admission.worker_identity()
        {
            return Err(MobError::Internal(
                "live failed-start retirement authority does not match the exact worker binding"
                    .to_string(),
            ));
        }
        if admission.worker_ownership()
            == meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember
        {
            return Ok(());
        }
        let identity = AgentIdentity::from(admission.worker_identity());
        if self.handle.get_member(&identity).await?.is_none() {
            return Ok(());
        }
        self.handle.retire(identity).await
    }

    /// Compatibility composition for a synchronous caller that bounds the
    /// call itself. The run lives in the caller's future; dropping it after
    /// the spawn requests the helper's retirement on a detached task.
    /// [`Self::execute_within`] instead owns a bounded run, cancels and
    /// retires its exact helper, and reports how that helper ended.
    ///
    /// Machine-admitted live callers use [`Self::start`], machine-owned
    /// cancellation, explicit terminal resolution, and separately authorized
    /// retirement instead.
    pub async fn execute(
        &self,
        request: DelegationExecutionRequest,
    ) -> Result<DelegationExecutionOutcome, DelegationExecutionError> {
        if request.live_admission.is_some() {
            return Err(DelegationExecutionError::LiveLifecycleRequired);
        }
        let handle = Box::pin(self.start_owning(request, true, None)).await?;
        let terminal = handle.await_terminal().await;
        self.settle_terminal(terminal).await
    }

    /// Retire the helper of a turn that reached its terminal, and answer
    /// the delegation: the completed turn, or the turn's failure.
    async fn settle_terminal(
        &self,
        terminal: DelegationTerminalizedExecution,
    ) -> Result<DelegationExecutionOutcome, DelegationExecutionError> {
        let retirement_error = self
            .retire_terminalized(&terminal)
            .await
            .err()
            .map(|error| error.to_string());
        let DelegationTerminalizedExecution {
            spawn,
            wired,
            identity: _,
            terminal,
            live,
            abandon_guard,
        } = terminal;
        // The helper's retirement was asked for above; its debt is reported.
        if let Some(guard) = abandon_guard {
            guard.disarm();
        }
        let spawn = spawn.ok_or(DelegationExecutionError::ExistingMemberAdmissionRequired)?;
        match terminal {
            DelegationTurnTerminal::Completed(turn) => Ok(DelegationExecutionOutcome {
                spawn,
                wired,
                turn,
                retirement_error,
                live,
            }),
            DelegationTurnTerminal::Failed(error) => Err(DelegationExecutionError::Turn {
                error,
                retirement_error: retirement_error.map(MobError::Internal),
            }),
        }
    }

    /// [`Self::execute`] under a run limit, with the run owned by a task of
    /// its own instead of the caller's future.
    ///
    /// A delegation that ends without its helper's terminal result ends its
    /// helper: when `bounds.max_run` elapses (counted from this call), or
    /// when the caller stops listening (this future is dropped), the
    /// helper's run is cancelled and the helper is retired. Both go through
    /// MobMachine, fenced on the incarnation this call's own spawn named:
    /// the cancel (`CancelAllWork`) names its runtime id and fence token,
    /// the retire the delivery scope MobMachine holds for that incarnation
    /// when it is retired, so neither reaches the caller's own work, another
    /// delegation's helper, or a later member under the same identity.
    ///
    /// The caller is answered within `bounds.max_run` and
    /// `bounds.end_within` of this call, whatever the run's task waits for.
    /// A limit that elapses while the spawn or the turn's admission is
    /// still in flight is answered at the limit, and a helper whose end
    /// takes longer than `bounds.end_within` is answered then: both as
    /// [`DelegationHelperEnd::EndPending`], with the task going on to end
    /// the helper and logging how that went.
    ///
    /// The task holds the limit's timer, the two ends it listens on (the
    /// helper's turn and the caller) and what its own spawn named, and no
    /// lifecycle state: what the helper is, is MobMachine's to say. Spawn
    /// and turn admission run in the task too, so a caller dropped between
    /// them leaves no helper without a turn. A task that ends without
    /// settling its run (it unwound) is answered
    /// [`DelegationExecutionError::RunLost`], and its guard ends the helper
    /// it had seated.
    ///
    /// A turn that completed before the cancel landed is answered as the
    /// completed delegation, limit or not. With nobody left to answer, the
    /// way the delegation settled is logged.
    pub async fn execute_within(
        &self,
        request: DelegationExecutionRequest,
        bounds: DelegationRunBounds,
    ) -> Result<DelegationExecutionOutcome, DelegationRunFailure> {
        let unseated = |error| DelegationRunFailure {
            error,
            helper_session_id: None,
            helper_hidden_from_caller: None,
        };
        if request.live_admission.is_some() {
            return Err(unseated(DelegationExecutionError::LiveLifecycleRequired));
        }
        let max_run = bounds.max_run;
        let answer_by = max_run.saturating_add(bounds.end_within);
        let (answer_tx, answer_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(self.clone().own_run(request, bounds, answer_tx));
        match tokio::time::timeout(answer_by, answer_rx).await {
            Ok(Ok(answer)) => answer,
            // The run's task ended without answering: it unwound, or its
            // runtime is shutting down.
            Ok(Err(_)) => Err(unseated(DelegationExecutionError::RunLost)),
            // The run's task has not settled in the time the caller waits
            // for it. Dropping the receiver tells the task that nobody
            // listens any more, which ends the helper if that is not under
            // way already.
            Err(_) => Err(unseated(DelegationExecutionError::MaxRunElapsed {
                max_run,
                ended: DelegationHelperEnded::pending(),
            })),
        }
    }

    /// The task of one bounded delegation (see [`Self::execute_within`]):
    /// the run, under the guard that ends its helper if the run is lost.
    async fn own_run(
        self,
        request: DelegationExecutionRequest,
        bounds: DelegationRunBounds,
        answer_tx: tokio::sync::oneshot::Sender<RunAnswer>,
    ) {
        // The run holds its mob while it owes a helper its end: an owner
        // lease, taken before the helper is seated and dropped when the run
        // has settled it. The mob's actor counts a lease as a foreground
        // owner, so an idle close asked in the gaps where nothing is in
        // flight (before the turn's admission, between the helper's end and
        // its retirement, between two attempts of that retirement) answers
        // busy, and the retirement is not refused by a mob that closed under
        // the run. A mob whose admission is closed already gives no lease:
        // the run goes on without one and is answered that mob's own
        // refusals.
        //
        // The lease is a value of its own: this local, and one clone in the
        // guard below, which moves it into the task that ends the helper of
        // a lost run. It issues no command and goes nowhere else: not into
        // the service's handle, the request, the outcome or the failure. A
        // lease lasts until the leased handle and all its clones drop, so a
        // clone that outlived the run would keep the mob loaded and owned
        // for good. It is as long as this task, which has no deadline beyond
        // its steps; an actor that never answers the cancel or the
        // retirement holds the task, and cannot answer an idle close either.
        let owner = self.handle.acquire_owner().ok();
        let seat = Arc::new(RunSeat::new(bounds.caller));
        let lost = EndHelperOfLostRun::arm(
            self.clone(),
            request.identity.clone(),
            Arc::clone(&seat),
            owner.clone(),
        );
        self.settle_run(request, bounds.max_run, &seat, answer_tx)
            .await;
        lost.disarm();
        drop(owner);
    }

    /// The whole of one bounded delegation, from the helper's spawn to its
    /// retirement.
    async fn settle_run(
        &self,
        request: DelegationExecutionRequest,
        max_run: std::time::Duration,
        seat: &RunSeat,
        answer_tx: tokio::sync::oneshot::Sender<RunAnswer>,
    ) {
        let identity = request.identity.clone();
        // `None` once the caller was answered or stopped listening.
        let mut caller = Some(answer_tx);
        let limit = tokio::time::sleep(max_run);
        tokio::pin!(limit);
        let mut limit_elapsed = false;

        let started = {
            let start = self.start_owning(request, false, Some(seat));
            tokio::pin!(start);
            tokio::select! {
                biased;
                started = &mut start => started,
                // The limit elapsed while the spawn or the turn's admission
                // was still in flight: the caller is answered now, and the
                // task keeps the start, to end whatever it seats.
                () = &mut limit => {
                    limit_elapsed = true;
                    self.answer(
                        caller.take(),
                        &identity,
                        seat,
                        Err(DelegationExecutionError::MaxRunElapsed {
                            max_run,
                            ended: DelegationHelperEnded::pending(),
                        }),
                    );
                    start.await
                }
                () = caller_gone(&mut caller) => {
                    caller = None;
                    start.await
                }
            }
        };
        let handle = match started {
            Ok(handle) => handle,
            // Nothing is left seated: `start` retired a helper whose turn
            // it could not admit.
            Err(error) => {
                self.answer(caller.take(), &identity, seat, Err(error));
                return;
            }
        };
        #[cfg(any(test, feature = "test-support"))]
        lose_run_if_requested(&identity);
        let Some(seated) = seat.seated() else {
            // A run that owns no member of its own: not this composition's.
            self.answer(
                caller.take(),
                &identity,
                seat,
                Err(DelegationExecutionError::ExistingMemberAdmissionRequired),
            );
            return;
        };
        let wait = handle.await_terminal();
        tokio::pin!(wait);
        if !limit_elapsed && caller.is_some() {
            // A terminal that is ready wins over a limit or a lost caller
            // seen at the same time.
            tokio::select! {
                biased;
                terminal = &mut wait => {
                    let settled = self.settle_terminal(terminal).await;
                    self.answer(caller.take(), &identity, seat, settled);
                    return;
                }
                () = &mut limit => limit_elapsed = true,
                () = caller_gone(&mut caller) => caller = None,
            }
        }
        #[cfg(any(test, feature = "test-support"))]
        if limit_elapsed {
            hold_run_if_requested(&identity, RunHoldPoint::LimitElapsed).await;
        }
        // The cleanup of a member the engine itself seated for one call, as
        // for a fork child past its limit: not an agent's cancel.
        let cleanup = self.handle.bounded_child_cleanup_authority();
        let cancel = cleanup
            .cancel_all_work(seated.runtime_id.clone(), seated.fence_token)
            .await;
        if let Ok(terminal) = tokio::time::timeout(ENDED_RUN_TERMINAL_REREAD, &mut wait).await
            && matches!(terminal.terminal(), DelegationTurnTerminal::Completed(_))
        {
            // The turn had completed before the cancel landed: its result
            // is committed and is the delegation's answer.
            let settled = self.settle_terminal(terminal).await;
            self.answer(caller.take(), &identity, seat, settled);
            return;
        }
        let retire = self.retire_seated(&cleanup, &identity, seat, &seated).await;
        let ended = DelegationHelperEnded::of(cancel, retire);
        if limit_elapsed && caller.is_some() {
            self.answer(
                caller.take(),
                &identity,
                seat,
                Err(DelegationExecutionError::MaxRunElapsed { max_run, ended }),
            );
        } else {
            tracing::warn!(
                mob_id = %self.handle.mob_id(),
                member = %identity,
                limit_elapsed,
                helper = ended.end.as_str(),
                cancel_error = ?ended.cancel_error,
                retirement_error = ?ended.retirement_error,
                "a delegation ended without its helper's result; its caller had been answered \
                 or had stopped listening when it ended its helper"
            );
        }
    }

    /// Answer the caller of one bounded delegation. A caller that was
    /// answered already, or that stopped listening, gets nothing: the
    /// settlement is logged instead.
    fn answer(
        &self,
        caller: Option<tokio::sync::oneshot::Sender<RunAnswer>>,
        identity: &AgentIdentity,
        seat: &RunSeat,
        settled: Result<DelegationExecutionOutcome, DelegationExecutionError>,
    ) {
        let seated = seat.seated();
        let answer = settled.map_err(|error| DelegationRunFailure {
            error,
            helper_session_id: seated.as_ref().and_then(|seated| seated.session_id.clone()),
            helper_hidden_from_caller: seated.as_ref().and_then(|seated| seated.hidden_from_caller),
        });
        let unheard = match caller {
            Some(caller) => match caller.send(answer) {
                Ok(()) => return,
                Err(unheard) => unheard,
            },
            None => answer,
        };
        let (settled, retirement_error) = match &unheard {
            Ok(outcome) => (
                "the helper's turn completed".to_string(),
                outcome.retirement_error().map(str::to_owned),
            ),
            Err(failure) => (
                failure.error.to_string(),
                match &failure.error {
                    DelegationExecutionError::WorkAdmission {
                        retirement_error, ..
                    }
                    | DelegationExecutionError::Turn {
                        retirement_error, ..
                    } => retirement_error.as_ref().map(ToString::to_string),
                    _ => None,
                },
            ),
        };
        tracing::warn!(
            mob_id = %self.handle.mob_id(),
            member = %identity,
            settled = %settled,
            retirement_error = ?retirement_error,
            "a delegation settled with nobody left to answer"
        );
    }

    /// What `start` reads of the helper its spawn just seated, for the run
    /// that owns it: the incarnation at once, then, while the helper's
    /// roster row is there, whether the mob's member tool audience hides it
    /// from the member the call was made for, and its session.
    async fn observe_seat(&self, seat: &RunSeat, spawn: &SpawnResult) {
        seat.seat(spawn);
        let identity = &spawn.agent_identity;
        let hidden = !self
            .handle
            .members_hidden_from(seat.caller.as_ref(), std::slice::from_ref(identity))
            .await
            .is_empty();
        let session_id = self
            .seated_scope(identity, &spawn.agent_runtime_id, spawn.fence_token)
            .await
            .ok()
            .flatten()
            .map(|scope| scope.session_id().clone());
        seat.note(|seated| {
            seated.hidden_from_caller = Some(hidden);
            seated.session_id = session_id;
        });
        #[cfg(any(test, feature = "test-support"))]
        hold_run_if_requested(identity, RunHoldPoint::Seated).await;
    }

    /// The delivery scope MobMachine holds for `identity` now, if it names
    /// the incarnation `runtime_id` and `fence_token` name. `Ok(None)` when
    /// MobMachine holds another incarnation under the identity, or none:
    /// the scope of another incarnation is not this run's to act on. `Err`
    /// when the incarnation is current and its scope cannot be read.
    async fn seated_scope(
        &self,
        identity: &AgentIdentity,
        runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
    ) -> Result<Option<MemberDeliveryScope>, MobError> {
        match self.handle.capture_member_delivery_scope(identity).await {
            Ok(scope) if scope.runtime_id() == runtime_id && scope.fence_token() == fence_token => {
                Ok(Some(scope))
            }
            Ok(_) | Err(MobError::MemberNotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Retire the incarnation a run seated, under the delivery scope
    /// MobMachine holds for it when the run ends: a scope read at the seat
    /// names a session the member may have been bound away from since. A
    /// scope that names another incarnation, or a refusal that says another
    /// incarnation is current, ends it as not current; a mob that no longer
    /// takes commands is final. A refusal that says the scope moved has the
    /// scope read again. Any other failure is retried within
    /// [`ENDED_RUN_RETIRE_RETRY`]: a failed read by another read, a failed
    /// retire under the scope it was made under, since MobMachine may have
    /// admitted that retire (the member then has no session binding to read
    /// a scope from) and admits the same scope again.
    async fn retire_seated(
        &self,
        cleanup: &MobHandle,
        identity: &AgentIdentity,
        seat: &RunSeat,
        seated: &SeatedHelper,
    ) -> SeatedRetire {
        let started = meerkat_core::time_compat::Instant::now();
        // The scope of a retire that failed, kept for its retry.
        let mut kept: Option<MemberDeliveryScope> = None;
        loop {
            let scope = match kept.take() {
                Some(scope) => Ok(scope),
                None => match self
                    .seated_scope(identity, &seated.runtime_id, seated.fence_token)
                    .await
                {
                    Ok(Some(scope)) => {
                        seat.note(|seated| seated.session_id = Some(scope.session_id().clone()));
                        Ok(scope)
                    }
                    Ok(None) => return SeatedRetire::NotCurrent,
                    Err(error) => Err(error),
                },
            };
            let failure = match scope {
                Ok(scope) => match cleanup.retire_at_scope(&scope).await {
                    Ok(()) => return SeatedRetire::Retired,
                    // The member's session was bound again since the scope
                    // was read: the next attempt reads it anew.
                    Err(error)
                        if matches!(
                            lifecycle_refusal(&error),
                            MobError::StaleDeliveryScope { .. }
                        ) =>
                    {
                        error
                    }
                    Err(error) if names_another_incarnation(&error) => {
                        return SeatedRetire::NotCurrent;
                    }
                    Err(error) if mob_no_longer_runs(&error) => {
                        return SeatedRetire::Failed(error);
                    }
                    Err(error) => {
                        kept = Some(scope);
                        error
                    }
                },
                Err(error) => error,
            };
            if started.elapsed() >= ENDED_RUN_RETIRE_RETRY {
                return SeatedRetire::Failed(failure);
            }
            tracing::debug!(
                member = %identity,
                error = %failure,
                "retiring the helper of an ended delegation failed; retrying"
            );
            tokio::time::sleep(ENDED_RUN_RETIRE_RETRY_PAUSE).await;
        }
    }

    async fn wire_parent(
        &self,
        identity: &AgentIdentity,
        parent: &DelegationParentContext,
    ) -> bool {
        let roster = self.handle.roster().await;
        let Some(entry) = roster.get_by_identity(identity) else {
            return false;
        };
        let Some(helper_peer_id) = entry.peer_id() else {
            return false;
        };
        // Two runtimes built on one key (two session runtimes of one realm
        // under tcp or uds comms) have one peer id. Wiring them would file
        // each store's own key in it, so the helper is not wired.
        if helper_peer_id == parent.peer_id {
            return false;
        }
        let Ok(helper_comms_name) = meerkat_core::MemberCommsName::new(
            self.handle.definition().id.as_str(),
            entry.role.as_str(),
            identity.as_str(),
        ) else {
            return false;
        };
        let helper_comms_name = helper_comms_name.to_string();
        if helper_comms_name == parent.comms_name {
            return false;
        }
        let peer_description = self
            .handle
            .definition()
            .resolve_inline_profile(&entry.role)
            .map(|profile| profile.peer_description.as_str())
            .unwrap_or("delegate helper")
            .to_string();
        let helper_role = entry.role.to_string();
        drop(roster);

        let Some(helper_bridge_session_id) = self.handle.resolve_bridge_session_id(identity).await
        else {
            return false;
        };
        let Some(helper_runtime) = self
            .handle
            .session_service
            .comms_runtime(&helper_bridge_session_id)
            .await
        else {
            return false;
        };

        let Ok(parent_spec) = trusted_descriptor_from_runtime(
            &parent.comms_name,
            parent.peer_id,
            format!("inproc://{}", parent.comms_name),
            parent.comms_runtime.as_ref(),
        ) else {
            return false;
        };
        if self
            .handle
            .wire(identity.clone(), super::PeerTarget::External(parent_spec))
            .await
            .is_err()
        {
            return false;
        }

        let Ok(helper_spec) = trusted_descriptor_from_runtime(
            &helper_comms_name,
            helper_peer_id,
            format!("inproc://{helper_comms_name}"),
            helper_runtime.as_ref(),
        ) else {
            return false;
        };
        if self
            .handle
            .apply_external_peer_reciprocal_trust(
                identity,
                &parent.comms_name,
                Arc::clone(&parent.comms_runtime),
                helper_spec,
            )
            .await
            .is_err()
        {
            return false;
        }

        let notify_parent = notify_peer_added(
            &helper_runtime,
            &parent.comms_name,
            identity.as_str(),
            &helper_role,
            &peer_description,
        )
        .await;
        let (parent_peer, parent_role, parent_description) =
            synthetic_parent_peer_added_fields(&parent.comms_name);
        let notify_helper = notify_peer_added(
            &parent.comms_runtime,
            &helper_comms_name,
            &parent_peer,
            &parent_role,
            &parent_description,
        )
        .await;
        if !(notify_parent && notify_helper) {
            tracing::warn!(
                mob_id = %self.handle.definition().id,
                helper = %helper_comms_name,
                notify_parent,
                notify_helper,
                "delegate helper trust edges committed but peer_added notification(s) failed; \
                 reporting wired=true from committed edge authority"
            );
        }

        true
    }
}

/// Where a test holds a bounded delegation's run.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RunHoldPoint {
    /// In `start`, once the helper is seated and was read, before its
    /// turn's admission.
    Seated,
    /// Once the run limit has elapsed, before the helper is cancelled.
    LimitElapsed,
}

#[cfg(any(test, feature = "test-support"))]
type DelegationRunHold = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(any(test, feature = "test-support"))]
type DelegationRunHoldEnds = (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
);

#[cfg(any(test, feature = "test-support"))]
static DELEGATION_RUN_HOLDS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<(AgentIdentity, RunHoldPoint), DelegationRunHold>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(any(test, feature = "test-support"))]
static DELEGATION_RUNS_TO_LOSE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<AgentIdentity>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(any(test, feature = "test-support"))]
fn hold_run_for_test(identity: AgentIdentity, point: RunHoldPoint) -> DelegationRunHoldEnds {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    DELEGATION_RUN_HOLDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((identity, point), (entered_tx, release_rx));
    (entered_rx, release_tx)
}

/// Hold the next bounded delegation of the helper `identity` at the moment
/// its run limit has elapsed, before it cancels the helper. The receiver
/// resolves when a run reaches that point; sending on (or dropping) the
/// returned sender lets it go on. Exposed only by test builds, for the rows
/// that decide what happens between the limit and the cancel.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn hold_delegation_limit_for_test(identity: AgentIdentity) -> DelegationRunHoldEnds {
    hold_run_for_test(identity, RunHoldPoint::LimitElapsed)
}

/// Hold the next bounded delegation of the helper `identity` inside its
/// start: the helper is seated, its turn is not admitted yet. The ends are
/// those of [`hold_delegation_limit_for_test`]. Exposed only by test builds,
/// for the rows of a start that outlasts the run limit.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn hold_delegation_start_for_test(identity: AgentIdentity) -> DelegationRunHoldEnds {
    hold_run_for_test(identity, RunHoldPoint::Seated)
}

/// Make the task of the next bounded delegation of the helper `identity`
/// unwind once its helper is seated and its turn admitted, as a defect in
/// the run would. Exposed only by test builds, for the row of a lost run.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn lose_delegation_run_for_test(identity: AgentIdentity) {
    DELEGATION_RUNS_TO_LOSE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(identity);
}

#[cfg(any(test, feature = "test-support"))]
async fn hold_run_if_requested(identity: &AgentIdentity, point: RunHoldPoint) {
    let hold = DELEGATION_RUN_HOLDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&(identity.clone(), point));
    if let Some((entered, release)) = hold {
        let _ = entered.send(());
        let _ = release.await;
    }
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::panic)]
fn lose_run_if_requested(identity: &AgentIdentity) {
    let lose = DELEGATION_RUNS_TO_LOSE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(identity);
    assert!(
        !lose,
        "a test asked for the bounded delegation run of `{identity}` to be lost"
    );
}

fn trusted_descriptor_from_runtime(
    name: &str,
    peer_id: PeerId,
    address: String,
    runtime: &dyn CommsRuntime,
) -> Result<TrustedPeerDescriptor, String> {
    let pubkey = runtime
        .public_key_bytes()
        .ok_or_else(|| format!("comms runtime for '{name}' does not expose public key bytes"))?;
    let address = runtime.advertised_address().unwrap_or(address);
    TrustedPeerDescriptor::unsigned_with_pubkey(
        name.to_string(),
        peer_id.to_string(),
        pubkey,
        address,
    )
}

fn synthetic_parent_peer_added_fields(parent_name: &str) -> (String, String, String) {
    match parent_name.parse::<meerkat_core::MemberCommsName>() {
        Ok(comms_name) => {
            let role = meerkat_core::PeerRole::Member(comms_name.role().to_string());
            (
                comms_name.member().to_string(),
                role.as_label().to_string(),
                format!("peer {}", role.as_label()),
            )
        }
        Err(_) => {
            let role = meerkat_core::PeerRole::External;
            (
                parent_name.to_string(),
                role.as_label().to_string(),
                "external peer".to_string(),
            )
        }
    }
}

async fn notify_peer_added(
    sender: &Arc<dyn CommsRuntime>,
    recipient_comms_name: &str,
    peer: &str,
    role: &str,
    description: &str,
) -> bool {
    let Ok(to) = PeerName::new(recipient_comms_name) else {
        return false;
    };
    let Some(route) = sender
        .peers()
        .await
        .into_iter()
        .find(|entry| entry.name == to)
        .map(|entry| PeerRoute::with_display_name(entry.peer_id, entry.name))
    else {
        return false;
    };
    sender
        .send(CommsCommand::PeerRequest {
            objective_id: None,
            to: route,
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({
                "peer": peer,
                "role": role,
                "description": description,
            }),
            blocks: None,
            content_taint: None,
            handling_mode: HandlingMode::Queue,
            stream: meerkat_core::comms::InputStreamMode::None,
        })
        .await
        .is_ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod helper_end_tests {
    use super::*;
    use crate::MobState;

    fn member() -> AgentIdentity {
        AgentIdentity::from("helper")
    }

    fn not_running() -> MobError {
        MobError::InvalidTransition {
            from: MobState::Completed,
            to: MobState::Running,
        }
    }

    /// The ends a helper comes to once its delegation ended without its
    /// result, from the answer of the fenced cancel and what the scoped
    /// retire came to: each pair here is one typed value.
    #[test]
    fn the_cancel_and_the_retire_classify_the_helpers_end() {
        // Cancelled and retired: both went through.
        let ended = DelegationHelperEnded::of(Ok(()), SeatedRetire::Retired);
        assert_eq!(ended.end, DelegationHelperEnd::CancelledAndRetired);
        assert_eq!((ended.cancel_error, ended.retirement_error), (None, None));

        // The incarnation was gone when the cancel named it: whatever the
        // retire then comes to, nothing of this call's was left.
        for retire in [SeatedRetire::Retired, SeatedRetire::NotCurrent] {
            let ended = DelegationHelperEnded::of(Err(MobError::MemberNotFound(member())), retire);
            assert_eq!(ended.end, DelegationHelperEnd::AlreadyGone);
            assert_eq!((ended.cancel_error, ended.retirement_error), (None, None));
        }

        // The cancel went through (the run was idle) and MobMachine holds
        // another incarnation by the time of the retire.
        let ended = DelegationHelperEnded::of(Ok(()), SeatedRetire::NotCurrent);
        assert_eq!(ended.end, DelegationHelperEnd::AlreadyGone);

        // The mob no longer takes commands.
        for stopped in [not_running(), MobError::ActorCommandChannelClosed] {
            let ended =
                DelegationHelperEnded::of(Err(not_running()), SeatedRetire::Failed(stopped));
            assert_eq!(ended.end, DelegationHelperEnd::MobNotRunning);
            assert_eq!((ended.cancel_error, ended.retirement_error), (None, None));
        }

        // The retirement failed: the helper stays, and the answer says why.
        let ended = DelegationHelperEnded::of(
            Ok(()),
            SeatedRetire::Failed(MobError::Internal("archive failed".into())),
        );
        assert_eq!(ended.end, DelegationHelperEnd::RetirementFailed);
        assert!(
            ended
                .retirement_error
                .as_deref()
                .is_some_and(|error| error.contains("archive failed")),
            "{ended:?}"
        );
        assert!(ended.to_string().contains("still a member"), "{ended}");

        // A helper whose scope cannot be read is not reported gone: it
        // stays a member, and the answer says so.
        let ended = DelegationHelperEnded::of(
            Ok(()),
            SeatedRetire::Failed(MobError::DeliveryScopeUnavailable {
                agent_identity: member(),
                reason: "member has no session binding to scope a delivery to".to_string(),
            }),
        );
        assert_eq!(ended.end, DelegationHelperEnd::RetirementFailed);
        assert!(ended.to_string().contains("still a member"), "{ended}");

        // A cancel that failed for another reason is kept beside a retire
        // that went through: the helper is gone, and the failure is told.
        let ended = DelegationHelperEnded::of(
            Err(MobError::Internal("cancel failed".into())),
            SeatedRetire::Retired,
        );
        assert_eq!(ended.end, DelegationHelperEnd::CancelledAndRetired);
        assert!(
            ended
                .cancel_error
                .as_deref()
                .is_some_and(|error| error.contains("cancel failed")),
            "{ended:?}"
        );

        // An end that had not finished when the caller had to be answered.
        let ended = DelegationHelperEnded::pending();
        assert_eq!(ended.end, DelegationHelperEnd::EndPending);
        assert_eq!((ended.cancel_error, ended.retirement_error), (None, None));
    }

    /// A refusal carried under a shared lifecycle failure is read as the
    /// refusal it carries.
    #[test]
    fn a_shared_lifecycle_failure_is_classified_by_what_it_carries() {
        let shared =
            MobError::SharedRetirementFailure(Arc::new(MobError::MemberNotFound(member())));
        assert!(names_another_incarnation(&shared));
        assert!(!mob_no_longer_runs(&shared));
        let shared = MobError::SharedLifecycleFailure(Arc::new(not_running()));
        assert!(mob_no_longer_runs(&shared));
        assert!(!names_another_incarnation(&shared));
        assert!(!names_another_incarnation(&MobError::Internal("x".into())));
    }

    #[test]
    fn every_end_has_a_stable_name() {
        for (end, name) in [
            (
                DelegationHelperEnd::CancelledAndRetired,
                "cancelled_and_retired",
            ),
            (DelegationHelperEnd::AlreadyGone, "already_gone"),
            (DelegationHelperEnd::MobNotRunning, "mob_not_running"),
            (DelegationHelperEnd::RetirementFailed, "retirement_failed"),
            (DelegationHelperEnd::EndPending, "end_pending"),
            (DelegationHelperEnd::Retired, "retired"),
            (DelegationHelperEnd::NotSeated, "not_seated"),
            (DelegationHelperEnd::Unknown, "unknown"),
        ] {
            assert_eq!(end.as_str(), name);
            assert!(
                DelegationHelperEnd::ALL.contains(&end),
                "{name} is in the list of every end"
            );
        }
        let mut names: Vec<&str> = DelegationHelperEnd::ALL
            .iter()
            .map(|end| end.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            DelegationHelperEnd::ALL.len(),
            "every end is listed once"
        );
    }
}
