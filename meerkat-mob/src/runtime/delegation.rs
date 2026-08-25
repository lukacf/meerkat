//! Reusable execution composition for one synchronous delegated helper turn.
//!
//! This module owns the mechanical spawn -> optional parent wiring -> exact
//! work admission -> terminal wait -> retirement sequence. Canonical member,
//! work, and terminal state remain owned by the existing MobMachine and exact
//! turn authorities reached through [`MobHandle`].

use std::sync::Arc;

use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{CommsCommand, PeerId, PeerName, PeerRoute, TrustedPeerDescriptor};
use meerkat_core::interaction::{InteractionId, ObjectiveId};
use meerkat_core::service::SessionServiceCommsExt;
use meerkat_core::types::HandlingMode;
use serde::Serialize;

use crate::machines::mob_machine::HostId;
use crate::profile::Profile;
use crate::{
    AgentIdentity, MobDeliveryIdentity, MobError, MobRuntimeMode, ProfileName, WorkOrigin, WorkSpec,
};

use super::{
    BoundedResultSpec, BoundedTurnWaitError, MobHandle, SpawnMemberSpec, SpawnResult,
    WorkBoundedTurnResult, WorkDeliveryReceipt, WorkTurnHandle,
};

/// Visible best-effort reporting convention for bounded delegated work.
///
/// This is deliberately prompt text rather than a structured-result contract.
/// Callers receive ordinary [`super::BoundedHelperResult`] text and must not
/// infer validation or success beyond its typed terminal/truncation status.
pub const BOUNDED_DELEGATION_REPORT_INSTRUCTION_V1: &str = "MEERKAT_BOUNDED_DELEGATION_REPORT_V1\nAfter doing the requested work, end with a clear, concise report of the work performed and the final result. This is a loose best-effort completion report, not a structured schema or success guarantee.";

#[must_use]
pub fn render_bounded_delegation_task(task: &str) -> String {
    format!("{task}\n\n{BOUNDED_DELEGATION_REPORT_INSTRUCTION_V1}")
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
    pub placement: Option<HostId>,
    pub additional_instructions: Option<Vec<String>>,
    pub inherited_tool_filter: Option<meerkat_core::InheritedToolVisibilityAuthority>,
    pub override_profile: Option<Profile>,
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    pub objective_id: Option<ObjectiveId>,
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
    spawn: SpawnResult,
    wired: bool,
    identity: AgentIdentity,
    terminal: DelegationTurnTerminal,
    live: Option<LiveDelegationTerminalEvidence>,
}

impl DelegationTerminalizedExecution {
    #[must_use]
    pub fn spawn(&self) -> &SpawnResult {
        &self.spawn
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
    spawn: SpawnResult,
    wired: bool,
    identity: AgentIdentity,
    result_spec: BoundedResultSpec,
    turn_handle: WorkTurnHandle,
    live_admission: Option<meerkat_runtime::live_execution::LiveDelegationExecutionAdmission>,
}

/// Cloneable exact-worker cancellation endpoint retained while another task
/// awaits terminality.
#[derive(Clone)]
pub struct DelegationCancellationHandle {
    service: DelegationExecutionService,
    identity: AgentIdentity,
    live_admission: meerkat_runtime::live_execution::LiveDelegationExecutionAdmission,
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
    pub fn spawn(&self) -> &SpawnResult {
        &self.spawn
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
            }),
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
    #[error("delegated helper spawn failed: {0}")]
    Spawn(#[source] MobError),
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
}

/// Mob-owned reusable executor for one synchronous delegated helper turn.
#[derive(Clone)]
pub struct DelegationExecutionService {
    handle: MobHandle,
}

impl DelegationExecutionService {
    #[must_use]
    pub fn new(handle: MobHandle) -> Self {
        Self { handle }
    }

    /// Start one delegation through existing MobMachine and exact-turn
    /// authorities. Worker terminality and retirement remain separate.
    pub async fn start(
        &self,
        request: DelegationExecutionRequest,
    ) -> Result<DelegationExecutionHandle, DelegationExecutionError> {
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

        let role = match &source {
            DelegationExecutionSource::Fresh => ProfileName::from("delegate"),
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
        let mut spec = SpawnMemberSpec::new(role, identity.clone());
        spec.initial_message = None;
        spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
        spec.auto_wire_parent = false;
        spec.placement = member.placement;
        spec.additional_instructions = member.additional_instructions;
        spec.inherited_tool_filter = member.inherited_tool_filter;
        spec.override_profile = member.override_profile;
        spec.tool_access_policy = member.tool_access_policy;
        spec.objective_id = member.objective_id.clone();
        spec.tool_dispatch_admission = live_admission
            .as_ref()
            .map(meerkat_runtime::live_execution::LiveDelegationExecutionAdmission::tool_dispatch_admission);

        let spawn = match source {
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
                let fork = self
                    .handle
                    .fork_member(&source_identity, spec, message_count)
                    .await
                    .map_err(DelegationExecutionError::Spawn)?;
                SpawnResult::new(fork.agent_identity, fork.agent_runtime_id, fork.fence_token)
            }
        };

        let wired = match parent.as_ref() {
            Some(parent) => self.wire_parent(&identity, parent).await,
            None => false,
        };

        let mut work = WorkSpec::new(task, WorkOrigin::Internal);
        if let Some(objective_id) = member.objective_id {
            work = work.with_objective_id(objective_id);
        }
        if let Some(interaction_id) = delivery_interaction_id {
            work = work.with_interaction_id(interaction_id);
        }
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
        let turn_handle = match turn_result {
            Ok(turn) => turn,
            Err(error) => {
                // A live worker is already bound into the generated
                // delegation lifecycle. Its failed-start cleanup remains
                // pending until the machine emits exact retirement authority.
                let retirement_error = if live_admission.is_some() {
                    None
                } else {
                    self.handle.retire(identity).await.err()
                };
                return Err(DelegationExecutionError::WorkAdmission {
                    error,
                    retirement_error,
                });
            }
        };
        Ok(DelegationExecutionHandle {
            service: self.clone(),
            spawn,
            wired,
            identity,
            result_spec,
            turn_handle,
            live_admission,
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

    /// Retire an exact terminal live worker under generated retirement authority.
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
        let identity = AgentIdentity::from(admission.worker_identity());
        if self.handle.get_member(&identity).await?.is_none() {
            return Ok(());
        }
        self.handle.retire(identity).await
    }

    /// Compatibility composition for synchronous callers such as MCP.
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
        let handle = self.start(request).await?;
        let terminal = handle.await_terminal().await;
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
        } = terminal;
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
