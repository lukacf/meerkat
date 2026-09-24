use std::str::FromStr;

use crate::{
    FlowId, FlowRunConfig, MobDeliveryIdentity, MobError, MobExternalDeliveryAbandonOutcome,
    MobExternalDeliveryBeginOutcome, MobExternalDeliveryIntent, MobExternalDeliveryPhase,
    MobExternalDeliveryRecord, MobExternalDeliveryTargetKind, MobExternalDeliveryTerminal,
    MobExternalFlowLaunchOutcome, MobId, MobRun, MobRunStatus, RunId,
};
use async_trait::async_trait;
use meerkat::{
    CloseWorkItemRequest, WorkExecutionAuthority, WorkExecutionBinding, WorkExecutionBindingId,
    WorkExecutionEvidenceKind, WorkExecutionEvidenceProjection, WorkExecutionLifecycleEffect,
    WorkExecutionMachine, WorkExecutionObservation, WorkExecutionTarget, WorkGraphError,
    WorkGraphMachine, WorkItem, WorkItemId, WorkItemRef, WorkNamespace, WorkStatus,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait WorkGraphFlowHost: Send + Sync {
    fn workgraph_execution_bridge(&self) -> Option<meerkat::WorkExecutionBridge>;

    async fn acquire_workgraph_flow_custody(
        &self,
        binding_id: &WorkExecutionBindingId,
    ) -> WorkGraphFlowCustodyGuard;

    /// Admit Flow execution under the calling surface principal before any
    /// durable WorkGraph obligation is committed, then return the immutable
    /// run configuration selected by the current Mob definition.
    async fn admit_workgraph_flow(
        &self,
        mob_id: &MobId,
        flow_id: &FlowId,
    ) -> Result<WorkGraphFlowAdmission, MobError>;

    async fn admit_workgraph_flow_caller(
        &self,
        mob_id: &MobId,
    ) -> Result<WorkExecutionAuthority, MobError>;

    async fn workgraph_flow_config(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
    ) -> Result<FlowRunConfig, MobError>;

    async fn workgraph_flow_begin_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryBeginOutcome, MobError>;

    async fn workgraph_flow_complete_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<(), MobError>;

    async fn workgraph_flow_complete_external_flow_realization(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        run: &MobRun,
    ) -> Result<(), MobError>;

    async fn workgraph_flow_abandon_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<MobExternalDeliveryAbandonOutcome, MobError>;

    async fn workgraph_flow_load_external_delivery(
        &self,
        observation: &WorkGraphFlowObservationAuthority,
        idempotency_key: &str,
    ) -> Result<Option<MobExternalDeliveryRecord>, MobError>;

    async fn workgraph_flow_run_with_external_identity(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        params: serde_json::Value,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalFlowLaunchOutcome, MobError>;

    async fn workgraph_flow_status(
        &self,
        observation: &WorkGraphFlowObservationAuthority,
    ) -> Result<Option<MobRun>, MobError>;
}

/// Result of target-runtime admission under the current surface caller.
/// The authority is persisted in the binding and must be re-presented exactly
/// for every recovery realization.
pub struct WorkGraphFlowAdmission {
    pub config: FlowRunConfig,
    pub execution_authority: WorkExecutionAuthority,
}

/// Exact bridge-owned witness for realizing one bound Mob Flow attempt under
/// its durably admitted target-runtime principal.
pub struct WorkGraphFlowExecutionAuthority {
    binding_id: WorkExecutionBindingId,
    mob_id: MobId,
    flow_id: FlowId,
    run_id: RunId,
    execution_authority: WorkExecutionAuthority,
}

impl WorkGraphFlowExecutionAuthority {
    pub fn binding_id(&self) -> &WorkExecutionBindingId {
        &self.binding_id
    }

    pub fn mob_id(&self) -> &MobId {
        &self.mob_id
    }

    pub fn flow_id(&self) -> &FlowId {
        &self.flow_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn execution_authority(&self) -> &WorkExecutionAuthority {
        &self.execution_authority
    }
}

/// Exact, bridge-owned authority to observe one run already fixed by a
/// canonical WorkGraph execution binding.
///
/// Only `WorkGraphFlowBridge` can construct this witness. Host adapters may
/// use it to bypass an unrelated broad `List` grant without gaining authority
/// to observe arbitrary Mob state.
pub struct WorkGraphFlowObservationAuthority {
    binding_id: WorkExecutionBindingId,
    mob_id: MobId,
    run_id: RunId,
}

impl WorkGraphFlowObservationAuthority {
    pub fn binding_id(&self) -> &WorkExecutionBindingId {
        &self.binding_id
    }

    pub fn mob_id(&self) -> &MobId {
        &self.mob_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

/// Host-owned single-realizer fence for one durable execution binding.
///
/// The guard is held across Mob admission and explicit abandonment. A host
/// restart drops every old realizer before new custody can be acquired, while
/// concurrent tasks in one host cannot race absence proof against run launch.
pub struct WorkGraphFlowCustodyGuard {
    #[cfg(not(target_arch = "wasm32"))]
    _guard: tokio::sync::OwnedMutexGuard<()>,
    #[cfg(target_arch = "wasm32")]
    _guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

impl WorkGraphFlowCustodyGuard {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(guard: tokio::sync::OwnedMutexGuard<()>) -> Self {
        Self { _guard: guard }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new(guard: crate::tokio::sync::OwnedMutexGuard<()>) -> Self {
        Self { _guard: guard }
    }
}

pub struct WorkGraphFlowBridge<'a, H: WorkGraphFlowHost + ?Sized> {
    host: &'a H,
}

impl<'a, H: WorkGraphFlowHost + ?Sized> WorkGraphFlowBridge<'a, H> {
    pub fn new(host: &'a H) -> Self {
        Self { host }
    }

    async fn mob_begin_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryBeginOutcome, MobError> {
        self.host
            .workgraph_flow_begin_external_delivery(authority, intent)
            .await
    }

    async fn mob_complete_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<(), MobError> {
        self.host
            .workgraph_flow_complete_external_delivery(authority, intent, terminal)
            .await
    }

    async fn mob_complete_external_flow_realization(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        run: &MobRun,
    ) -> Result<(), MobError> {
        self.host
            .workgraph_flow_complete_external_flow_realization(authority, intent, run)
            .await
    }

    async fn mob_abandon_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<MobExternalDeliveryAbandonOutcome, MobError> {
        self.host
            .workgraph_flow_abandon_external_delivery(authority, intent, terminal)
            .await
    }

    async fn mob_load_external_delivery(
        &self,
        observation: &WorkGraphFlowObservationAuthority,
        idempotency_key: &str,
    ) -> Result<Option<MobExternalDeliveryRecord>, MobError> {
        self.host
            .workgraph_flow_load_external_delivery(observation, idempotency_key)
            .await
    }

    async fn mob_run_flow_with_external_identity(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        params: serde_json::Value,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalFlowLaunchOutcome, MobError> {
        self.host
            .workgraph_flow_run_with_external_identity(authority, params, intent)
            .await
    }

    async fn mob_flow_status(
        &self,
        binding: &WorkExecutionBinding,
    ) -> Result<Option<MobRun>, MobError> {
        let authority = execution_authority(binding).map_err(|error| {
            MobError::Internal(format!(
                "invalid WorkGraph Flow observation binding: {error}"
            ))
        })?;
        self.host
            .workgraph_flow_status(&WorkGraphFlowObservationAuthority {
                binding_id: binding.binding_id.clone(),
                mob_id: authority.mob_id,
                run_id: authority.run_id,
            })
            .await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchWorkGraphFlowRequest {
    pub work_item_id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_item_revision: u64,
    pub mob_id: MobId,
    pub flow_id: FlowId,
    #[serde(default)]
    pub activation_params: serde_json::Value,
    /// Caller-stable launch key. Replays with the same key select the same
    /// binding and deterministic Mob run id.
    pub idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<WorkExecutionBindingId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkGraphFlowLaunchResult {
    pub binding: WorkExecutionBinding,
    pub run: Option<MobRun>,
    pub item: WorkItem,
    pub evidence_projected: bool,
    pub work_item_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkGraphFlowReconcileResult {
    pub binding: WorkExecutionBinding,
    pub run: Option<MobRun>,
    pub item: WorkItem,
    pub evidence_projected: bool,
    pub work_item_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbandonUncertainWorkGraphFlowRequest {
    pub binding_id: WorkExecutionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_binding_revision: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkGraphFlowAbandonResult {
    pub binding: WorkExecutionBinding,
    pub item: WorkItem,
    pub evidence_projected: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkGraphFlowBridgeError {
    #[error("WorkGraph service is not configured for this mob surface")]
    WorkGraphUnavailable,
    #[error(transparent)]
    WorkGraph(#[from] WorkGraphError),
    #[error(transparent)]
    Mob(#[from] MobError),
    #[error("invalid WorkGraph Flow binding: {0}")]
    InvalidBinding(String),
    #[error(
        "Flow launch for execution binding {binding_id} is ambiguous: durable launch intent exists but run {run_id} is absent; refusing blind redrive"
    )]
    AmbiguousLaunch {
        binding_id: WorkExecutionBindingId,
        run_id: String,
    },
    #[error("Flow launch for execution binding {binding_id} already failed: {detail}")]
    LaunchFailed {
        binding_id: WorkExecutionBindingId,
        detail: String,
    },
    #[error("Flow run {run_id} for execution binding {binding_id} is missing")]
    RunMissing {
        binding_id: WorkExecutionBindingId,
        run_id: String,
    },
    #[error(
        "Mob returned conflicting run {observed_run_id} for execution binding {binding_id}, which selected {expected_run_id}"
    )]
    ConflictingRun {
        binding_id: WorkExecutionBindingId,
        expected_run_id: String,
        observed_run_id: String,
    },
    #[error("Flow launch for execution binding {binding_id} is quarantined: {detail}")]
    LaunchQuarantined {
        binding_id: WorkExecutionBindingId,
        detail: String,
    },
}

impl<H: WorkGraphFlowHost + ?Sized> WorkGraphFlowBridge<'_, H> {
    /// Bind a WorkGraph item to one exact Flow run, durably record the binding
    /// before launch, and reconcile any immediately visible terminal outcome.
    pub async fn launch_workgraph_flow(
        &self,
        request: LaunchWorkGraphFlowRequest,
    ) -> Result<WorkGraphFlowLaunchResult, WorkGraphFlowBridgeError> {
        let workgraph = self
            .host
            .workgraph_execution_bridge()
            .ok_or(WorkGraphFlowBridgeError::WorkGraphUnavailable)?;
        if request
            .realm_id
            .as_deref()
            .is_some_and(|realm| realm != workgraph.default_realm_id())
        {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "realm '{}' does not match this bridge's owning realm '{}'",
                request.realm_id.as_deref().unwrap_or_default(),
                workgraph.default_realm_id()
            )));
        }
        let item = workgraph
            .get(
                request.realm_id.clone(),
                request.namespace.clone(),
                request.work_item_id.clone(),
            )
            .await?;
        let binding_identity = serde_json::to_vec(&(
            "meerkat:workgraph:execution:v1",
            item.realm_id.as_str(),
            item.namespace.as_str(),
            item.id.as_str(),
            request.idempotency_key.as_str(),
        ))
        .map_err(|error| {
            WorkGraphFlowBridgeError::InvalidBinding(format!(
                "execution binding identity serialization failed: {error}"
            ))
        })?;
        let binding_uuid = Uuid::new_v5(&Uuid::NAMESPACE_URL, &binding_identity);
        let binding_id = WorkExecutionBindingId::new(format!("execution_{binding_uuid}"))?;
        let _custody = self.host.acquire_workgraph_flow_custody(&binding_id).await;
        // Admission precedes both idempotent replay and a new durable WorkGraph
        // obligation. Replay must not become an exact-run read bypass for a
        // different or since-revoked surface principal.
        let admission = self
            .host
            .admit_workgraph_flow(&request.mob_id, &request.flow_id)
            .await?;
        if let Some(existing) = workgraph
            .find_execution_binding(
                Some(item.realm_id.clone()),
                Some(item.namespace.clone()),
                binding_id.clone(),
            )
            .await?
        {
            if !request_matches_binding(&existing, &request) {
                return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "idempotency key selected binding {} with different execution semantics",
                    existing.binding_id
                )));
            }
            if !admission_matches_binding(&admission, &existing)? {
                return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "execution binding {} was admitted under a different principal or Flow configuration",
                    existing.binding_id
                )));
            }
            let reconciled = self.drive_work_execution(existing).await?;
            return Ok(WorkGraphFlowLaunchResult {
                binding: reconciled.binding,
                run: reconciled.run,
                item: reconciled.item,
                evidence_projected: reconciled.evidence_projected,
                work_item_closed: reconciled.work_item_closed,
            });
        }

        if item.revision != request.expected_item_revision {
            return Err(WorkGraphError::StaleRevision {
                id: item.id.clone(),
                expected: request.expected_item_revision,
                actual: item.revision,
            }
            .into());
        }
        if WorkGraphMachine::classify_terminality(&item)? {
            return Err(WorkGraphError::InvalidTransition(format!(
                "terminal work item {} cannot launch a Flow",
                item.id
            ))
            .into());
        }
        if let Some(predecessor_id) = request.supersedes.as_ref() {
            let predecessor = workgraph
                .execution_binding(
                    Some(item.realm_id.clone()),
                    Some(item.namespace.clone()),
                    predecessor_id.clone(),
                )
                .await?;
            if !WorkExecutionMachine::retry_eligible(&predecessor)? {
                return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "execution binding {predecessor_id} is not terminal and cannot be superseded"
                )));
            }
        }

        let flow_config_digest = admission.config.definition_digest()?;
        let delivery_key = binding_id.as_str().to_string();
        let run_id = RunId::for_work_execution(&request.mob_id, &request.flow_id, &delivery_key);
        let correlation_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("meerkat:workgraph-execution:{binding_id}:correlation").as_bytes(),
        )
        .to_string();
        MobDeliveryIdentity::new(delivery_key.clone(), correlation_id.clone())
            .map_err(MobError::from)?;
        let target = WorkExecutionTarget::mob_flow(
            request.mob_id.to_string(),
            request.flow_id.to_string(),
            flow_config_digest,
            run_id.to_string(),
            admission.execution_authority,
            request.activation_params,
        )?;
        let (machine_state, _) = WorkExecutionMachine::bind(&binding_id, &run_id.to_string())?;
        let candidate = WorkExecutionBinding {
            binding_id: binding_id.clone(),
            work_ref: WorkItemRef {
                realm_id: item.realm_id.clone(),
                namespace: item.namespace.clone(),
                item_id: item.id.clone(),
            },
            target,
            idempotency_key: request.idempotency_key,
            correlation_id,
            supersedes: request.supersedes,
            machine_state,
            created_at: workgraph.store().get_store_time_utc().await?,
        };

        let binding = workgraph
            .bind_execution(candidate, request.expected_item_revision)
            .await?
            .binding;

        let reconciled = self.drive_work_execution(binding).await?;
        Ok(WorkGraphFlowLaunchResult {
            binding: reconciled.binding,
            run: reconciled.run,
            item: reconciled.item,
            evidence_projected: reconciled.evidence_projected,
            work_item_closed: reconciled.work_item_closed,
        })
    }

    /// Re-read the canonical Mob run selected by a durable execution binding.
    /// Completed runs project one deterministic evidence reference. Only the
    /// WorkGraph machine may then admit closure under the item's completion
    /// policy.
    pub async fn reconcile_workgraph_flow(
        &self,
        realm_id: Option<String>,
        namespace: Option<WorkNamespace>,
        binding_id: WorkExecutionBindingId,
    ) -> Result<WorkGraphFlowReconcileResult, WorkGraphFlowBridgeError> {
        let workgraph = self
            .host
            .workgraph_execution_bridge()
            .ok_or(WorkGraphFlowBridgeError::WorkGraphUnavailable)?;
        if realm_id
            .as_deref()
            .is_some_and(|realm| realm != workgraph.default_realm_id())
        {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "realm '{}' does not match this bridge's owning realm '{}'",
                realm_id.as_deref().unwrap_or_default(),
                workgraph.default_realm_id()
            )));
        }
        let binding = workgraph
            .execution_binding(realm_id, namespace, binding_id)
            .await?;
        let _custody = self
            .host
            .acquire_workgraph_flow_custody(&binding.binding_id)
            .await;
        let binding = workgraph
            .execution_binding(
                Some(binding.work_ref.realm_id.clone()),
                Some(binding.work_ref.namespace.clone()),
                binding.binding_id,
            )
            .await?;
        self.drive_work_execution(binding).await
    }

    /// Public-surface reconciliation path. The current caller must still hold
    /// SendCommand for the exact Mob/Flow and must match the principal durably
    /// recorded on the binding before any run payload is observed or returned.
    pub async fn reconcile_workgraph_flow_for_caller(
        &self,
        realm_id: Option<String>,
        namespace: Option<WorkNamespace>,
        binding_id: WorkExecutionBindingId,
    ) -> Result<WorkGraphFlowReconcileResult, WorkGraphFlowBridgeError> {
        let binding = self
            .caller_authorized_binding(realm_id.clone(), namespace.clone(), binding_id.clone())
            .await?;
        if binding.binding_id != binding_id {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(
                "authorized execution binding identity changed".to_string(),
            ));
        }
        self.reconcile_workgraph_flow(realm_id, namespace, binding_id)
            .await
    }

    /// Resolve an ambiguous Begin-without-run boundary without re-executing the
    /// same delivery identity. This is an explicit CAS-fenced abandonment, not
    /// an automatic retry: a later attempt must create a superseding binding.
    pub async fn abandon_uncertain_workgraph_flow(
        &self,
        request: AbandonUncertainWorkGraphFlowRequest,
    ) -> Result<WorkGraphFlowAbandonResult, WorkGraphFlowBridgeError> {
        let workgraph = self
            .host
            .workgraph_execution_bridge()
            .ok_or(WorkGraphFlowBridgeError::WorkGraphUnavailable)?;
        if request
            .realm_id
            .as_deref()
            .is_some_and(|realm| realm != workgraph.default_realm_id())
        {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "realm '{}' does not match this bridge's owning realm '{}'",
                request.realm_id.as_deref().unwrap_or_default(),
                workgraph.default_realm_id()
            )));
        }
        if request.detail.trim().is_empty()
            || request.detail.len() > 4096
            || request.detail.chars().any(char::is_control)
        {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(
                "uncertain launch abandonment requires non-empty single-line detail no longer than 4096 bytes"
                    .to_string(),
            ));
        }
        let binding = workgraph
            .execution_binding(request.realm_id, request.namespace, request.binding_id)
            .await?;
        let _custody = self
            .host
            .acquire_workgraph_flow_custody(&binding.binding_id)
            .await;
        let binding = workgraph
            .execution_binding(
                Some(binding.work_ref.realm_id.clone()),
                Some(binding.work_ref.namespace.clone()),
                binding.binding_id,
            )
            .await?;
        if binding.machine_state.revision != request.expected_binding_revision {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "stale execution binding revision: expected {}, actual {}",
                request.expected_binding_revision, binding.machine_state.revision
            )));
        }
        let recovery_effect = WorkExecutionMachine::recover_effect(&binding)?;
        let resolvable = matches!(
            &recovery_effect,
            WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }
        );
        if !resolvable {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "execution binding {} is not awaiting an authorized launch resolution",
                binding.binding_id
            )));
        }
        if self
            .flow_run_for_binding_optional(&binding)
            .await?
            .is_some()
        {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "execution binding {} now has a run and cannot be abandoned",
                binding.binding_id
            )));
        }

        let intent = delivery_intent(&binding)?;
        let authority = execution_authority(&binding)?;
        let terminal =
            MobExternalDeliveryTerminal::failed(&MobError::Internal(request.detail.clone()));
        let abandonment = self
            .mob_abandon_external_delivery(&authority, &intent, &terminal)
            .await?;
        match abandonment {
            MobExternalDeliveryAbandonOutcome::Abandoned => {}
            MobExternalDeliveryAbandonOutcome::ExistingRealizing { .. } => {
                return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "execution binding {} crossed the durable realization fence and cannot be abandoned while the realizer may still run",
                    binding.binding_id
                )));
            }
            MobExternalDeliveryAbandonOutcome::ExistingTerminal(existing)
                if existing == terminal => {}
            MobExternalDeliveryAbandonOutcome::ExistingTerminal(_) => {
                return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "execution binding {} already has a different delivery terminal",
                    binding.binding_id
                )));
            }
        }
        let observation = WorkExecutionObservation::LaunchFailed {
            detail: request.detail,
        };
        let binding = observe_binding(&workgraph, &binding, observation).await?;
        let WorkExecutionLifecycleEffect::LaunchFailureEvidenceProjectionRequested {
            detail, ..
        } = WorkExecutionMachine::recover_effect(&binding)?
        else {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "execution binding {} did not request launch-failure evidence",
                binding.binding_id
            )));
        };
        project_launch_failure_evidence(&workgraph, &binding, &detail).await?;
        let binding = observe_binding(
            &workgraph,
            &binding,
            WorkExecutionObservation::LaunchFailureEvidenceProjected,
        )
        .await?;
        let item = workgraph
            .get(
                Some(binding.work_ref.realm_id.clone()),
                Some(binding.work_ref.namespace.clone()),
                binding.work_ref.item_id.clone(),
            )
            .await?;
        let evidence_projected = item.evidence_refs.iter().any(|evidence| {
            evidence.execution_binding_id.as_ref() == Some(&binding.binding_id)
                && evidence.id == binding.evidence_id()
        });
        Ok(WorkGraphFlowAbandonResult {
            binding,
            item,
            evidence_projected,
        })
    }

    /// Public-surface abandonment path with the same durable-principal check as
    /// launch replay and reconciliation.
    pub async fn abandon_uncertain_workgraph_flow_for_caller(
        &self,
        request: AbandonUncertainWorkGraphFlowRequest,
    ) -> Result<WorkGraphFlowAbandonResult, WorkGraphFlowBridgeError> {
        self.caller_authorized_binding(
            request.realm_id.clone(),
            request.namespace.clone(),
            request.binding_id.clone(),
        )
        .await?;
        self.abandon_uncertain_workgraph_flow(request).await
    }

    async fn caller_authorized_binding(
        &self,
        realm_id: Option<String>,
        namespace: Option<WorkNamespace>,
        binding_id: WorkExecutionBindingId,
    ) -> Result<WorkExecutionBinding, WorkGraphFlowBridgeError> {
        let workgraph = self
            .host
            .workgraph_execution_bridge()
            .ok_or(WorkGraphFlowBridgeError::WorkGraphUnavailable)?;
        let binding = workgraph
            .execution_binding(realm_id, namespace, binding_id)
            .await?;
        let (mob_id, _, _, _) = binding_target(&binding)?;
        let caller = self.host.admit_workgraph_flow_caller(&mob_id).await?;
        let WorkExecutionTarget::MobFlow {
            execution_authority,
            ..
        } = &binding.target;
        if caller != WorkExecutionAuthority::TargetOwner && caller != *execution_authority {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "execution binding {} is owned by a different principal or Flow configuration",
                binding.binding_id
            )));
        }
        Ok(binding)
    }

    async fn drive_work_execution(
        &self,
        mut binding: WorkExecutionBinding,
    ) -> Result<WorkGraphFlowReconcileResult, WorkGraphFlowBridgeError> {
        let workgraph = self
            .host
            .workgraph_execution_bridge()
            .ok_or(WorkGraphFlowBridgeError::WorkGraphUnavailable)?;

        for _ in 0..8 {
            let effect = WorkExecutionMachine::recover_effect(&binding)?;
            match effect {
                WorkExecutionLifecycleEffect::FlowLaunchRequested { .. } => {
                    let run = match self.ensure_flow_binding_launched(&binding).await {
                        Ok(run) => run,
                        Err(error @ WorkGraphFlowBridgeError::AmbiguousLaunch { .. }) => {
                            observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::LaunchUncertain {
                                    detail: error.to_string(),
                                },
                            )
                            .await?;
                            return Err(error);
                        }
                        Err(error @ WorkGraphFlowBridgeError::LaunchFailed { .. }) => {
                            binding = observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::LaunchFailed {
                                    detail: error.to_string(),
                                },
                            )
                            .await?;
                            continue;
                        }
                        Err(error @ WorkGraphFlowBridgeError::RunMissing { .. }) => {
                            binding = observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::FlowStarted,
                            )
                            .await?;
                            binding = observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::FlowRunLost {
                                    detail: error.to_string(),
                                },
                            )
                            .await?;
                            continue;
                        }
                        Err(error @ WorkGraphFlowBridgeError::ConflictingRun { .. }) => {
                            observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::LaunchQuarantined {
                                    detail: error.to_string(),
                                },
                            )
                            .await?;
                            return Err(error);
                        }
                        Err(error @ WorkGraphFlowBridgeError::LaunchQuarantined { .. }) => {
                            observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::LaunchQuarantined {
                                    detail: error.to_string(),
                                },
                            )
                            .await?;
                            return Err(error);
                        }
                        Err(error @ WorkGraphFlowBridgeError::InvalidBinding(_)) => {
                            observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::LaunchUncertain {
                                    detail: error.to_string(),
                                },
                            )
                            .await?;
                            return Err(error);
                        }
                        Err(WorkGraphFlowBridgeError::Mob(error))
                            if error.failure_class() == crate::MobFailureClass::TargetMissing
                                || matches!(&error, MobError::ScopeDenied(_)) =>
                        {
                            observe_binding(
                                &workgraph,
                                &binding,
                                WorkExecutionObservation::LaunchUncertain {
                                    detail: error.to_string(),
                                },
                            )
                            .await?;
                            return Err(WorkGraphFlowBridgeError::AmbiguousLaunch {
                                binding_id: binding.binding_id.clone(),
                                run_id: binding.target.run_id().to_string(),
                            });
                        }
                        Err(error) => return Err(error),
                    };
                    binding = observe_flow_run(&workgraph, binding, &run).await?;
                }
                WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. } => {
                    let run = self.flow_run_for_binding_optional(&binding).await?;
                    let Some(run) = run else {
                        return Err(WorkGraphFlowBridgeError::AmbiguousLaunch {
                            binding_id: binding.binding_id.clone(),
                            run_id: binding.target.run_id().to_string(),
                        });
                    };
                    binding = observe_flow_run(&workgraph, binding, &run).await?;
                }
                WorkExecutionLifecycleEffect::FlowLaunchQuarantined { detail, .. } => {
                    if let Some(run) = self.flow_run_for_binding_optional(&binding).await? {
                        binding = observe_flow_run(&workgraph, binding, &run).await?;
                        continue;
                    }
                    return Err(WorkGraphFlowBridgeError::LaunchQuarantined {
                        binding_id: binding.binding_id,
                        detail,
                    });
                }
                WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. } => {
                    let Some(run) = self.flow_run_for_binding_optional(&binding).await? else {
                        binding = observe_binding(
                            &workgraph,
                            &binding,
                            WorkExecutionObservation::FlowRunLost {
                                detail: format!(
                                    "Mob Flow run {} is absent from its owning runtime",
                                    binding.target.run_id()
                                ),
                            },
                        )
                        .await?;
                        continue;
                    };
                    if matches!(run.status, MobRunStatus::Pending | MobRunStatus::Running) {
                        return execution_result(&workgraph, binding, Some(run)).await;
                    }
                    binding = observe_flow_run(&workgraph, binding, &run).await?;
                }
                WorkExecutionLifecycleEffect::EvidenceProjectionRequested { kind, .. } => {
                    if kind != WorkExecutionEvidenceKind::Completed {
                        return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                            "execution binding {} requested success evidence with machine kind {kind:?}",
                            binding.binding_id
                        )));
                    }
                    if execution_evidence_projected(&workgraph, &binding).await? {
                        binding = observe_binding(
                            &workgraph,
                            &binding,
                            WorkExecutionObservation::EvidenceProjected,
                        )
                        .await?;
                        continue;
                    }
                    let Some(run) = self.flow_run_for_binding_optional(&binding).await? else {
                        binding = observe_binding(
                            &workgraph,
                            &binding,
                            WorkExecutionObservation::FlowRunLost {
                                detail: format!(
                                    "Mob Flow run {} disappeared before completion evidence was durably projected",
                                    binding.target.run_id()
                                ),
                            },
                        )
                        .await?;
                        continue;
                    };
                    if !matches!(run.status, MobRunStatus::Completed) {
                        return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                            "execution binding {} requested evidence for non-completed run {}",
                            binding.binding_id, run.run_id
                        )));
                    }
                    project_flow_evidence(&workgraph, &binding, &run, kind).await?;
                    binding = observe_binding(
                        &workgraph,
                        &binding,
                        WorkExecutionObservation::EvidenceProjected,
                    )
                    .await?;
                }
                WorkExecutionLifecycleEffect::FlowFailureEvidenceProjectionRequested {
                    kind,
                    ..
                } => {
                    if execution_evidence_projected(&workgraph, &binding).await? {
                        binding = observe_binding(
                            &workgraph,
                            &binding,
                            WorkExecutionObservation::FlowFailureEvidenceProjected,
                        )
                        .await?;
                        continue;
                    }
                    match kind {
                        WorkExecutionEvidenceKind::Failed => {
                            let run = self.flow_run_for_binding_optional(&binding).await?;
                            if let Some(run) = run.as_ref()
                                && !matches!(run.status, MobRunStatus::Failed)
                            {
                                return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                                    "execution binding {} requested failure evidence for run {} in status {:?}",
                                    binding.binding_id, run.run_id, run.status
                                )));
                            }
                            project_observed_terminal_evidence(
                                &workgraph,
                                &binding,
                                run.as_ref(),
                                kind,
                            )
                            .await?;
                        }
                        WorkExecutionEvidenceKind::RunLost => {
                            project_run_lost_evidence(
                                &workgraph,
                                &binding,
                                binding
                                    .machine_state
                                    .last_failure_detail
                                    .as_deref()
                                    .unwrap_or("Mob Flow run was lost"),
                            )
                            .await?;
                        }
                        other => {
                            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                                "execution binding {} requested failure evidence with machine kind {other:?}",
                                binding.binding_id
                            )));
                        }
                    }
                    binding = observe_binding(
                        &workgraph,
                        &binding,
                        WorkExecutionObservation::FlowFailureEvidenceProjected,
                    )
                    .await?;
                }
                WorkExecutionLifecycleEffect::FlowCancellationEvidenceProjectionRequested {
                    kind,
                    ..
                } => {
                    if execution_evidence_projected(&workgraph, &binding).await? {
                        binding = observe_binding(
                            &workgraph,
                            &binding,
                            WorkExecutionObservation::FlowCancellationEvidenceProjected,
                        )
                        .await?;
                        continue;
                    }
                    if kind != WorkExecutionEvidenceKind::Canceled {
                        return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                            "execution binding {} requested cancellation evidence with machine kind {kind:?}",
                            binding.binding_id
                        )));
                    }
                    let run = self.flow_run_for_binding_optional(&binding).await?;
                    if let Some(run) = run.as_ref()
                        && !matches!(run.status, MobRunStatus::Canceled)
                    {
                        return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                            "execution binding {} requested cancellation evidence for run {} in status {:?}",
                            binding.binding_id, run.run_id, run.status
                        )));
                    }
                    project_observed_terminal_evidence(&workgraph, &binding, run.as_ref(), kind)
                        .await?;
                    binding = observe_binding(
                        &workgraph,
                        &binding,
                        WorkExecutionObservation::FlowCancellationEvidenceProjected,
                    )
                    .await?;
                }
                WorkExecutionLifecycleEffect::LaunchFailureEvidenceProjectionRequested {
                    detail,
                    kind,
                    ..
                } => {
                    if kind != WorkExecutionEvidenceKind::LaunchFailed {
                        return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                            "execution binding {} requested launch-failure evidence with machine kind {kind:?}",
                            binding.binding_id
                        )));
                    }
                    if !execution_evidence_projected(&workgraph, &binding).await? {
                        project_launch_failure_evidence(&workgraph, &binding, &detail).await?;
                    }
                    binding = observe_binding(
                        &workgraph,
                        &binding,
                        WorkExecutionObservation::LaunchFailureEvidenceProjected,
                    )
                    .await?;
                }
                WorkExecutionLifecycleEffect::WorkClosureRequested { .. } => {
                    let mut closure_observation = None;
                    for attempt in 0..8 {
                        let item = workgraph
                            .get(
                                Some(binding.work_ref.realm_id.clone()),
                                Some(binding.work_ref.namespace.clone()),
                                binding.work_ref.item_id.clone(),
                            )
                            .await?;
                        if WorkGraphMachine::classify_terminality(&item)? {
                            closure_observation = Some(if item.status == WorkStatus::Completed {
                                WorkExecutionObservation::WorkClosed
                            } else {
                                WorkExecutionObservation::WorkClosureRefused {
                                    detail: format!(
                                        "work item {} terminalized as {:?}, not completed",
                                        item.id, item.status
                                    ),
                                }
                            });
                            break;
                        }
                        match workgraph
                            .close(CloseWorkItemRequest {
                                id: item.id,
                                realm_id: Some(item.realm_id),
                                namespace: Some(item.namespace),
                                expected_revision: item.revision,
                                status: WorkStatus::Completed,
                            })
                            .await
                        {
                            Ok(_) => {
                                closure_observation = Some(WorkExecutionObservation::WorkClosed);
                                break;
                            }
                            Err(WorkGraphError::StaleRevision { .. }) if attempt + 1 < 8 => {
                                // WorkGraph item changes are not Mob machine
                                // wakeups. Resolve this CAS race here so a quiet
                                // terminal Flow cannot strand its closure
                                // obligation until an unrelated Mob event.
                                crate::tokio::task::yield_now().await;
                                continue;
                            }
                            Err(error @ WorkGraphError::InvalidTransition(_)) => {
                                let current = workgraph
                                    .get(
                                        Some(binding.work_ref.realm_id.clone()),
                                        Some(binding.work_ref.namespace.clone()),
                                        binding.work_ref.item_id.clone(),
                                    )
                                    .await?;
                                closure_observation =
                                    Some(if current.status == WorkStatus::Completed {
                                        WorkExecutionObservation::WorkClosed
                                    } else {
                                        WorkExecutionObservation::WorkClosureRefused {
                                            detail: error.to_string(),
                                        }
                                    });
                                break;
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                    let observation = closure_observation.ok_or_else(|| {
                        WorkGraphFlowBridgeError::WorkGraph(WorkGraphError::Conflict(format!(
                            "work closure for execution binding {} exceeded the bounded CAS retry budget",
                            binding.binding_id
                        )))
                    })?;
                    binding = observe_binding(&workgraph, &binding, observation).await?;
                }
                WorkExecutionLifecycleEffect::FlowFailed { .. }
                | WorkExecutionLifecycleEffect::FlowCanceled { .. }
                | WorkExecutionLifecycleEffect::EvidenceProjected { .. }
                | WorkExecutionLifecycleEffect::WorkClosed { .. } => {
                    let run = self.flow_run_for_binding_optional(&binding).await?;
                    return execution_result(&workgraph, binding, run).await;
                }
                WorkExecutionLifecycleEffect::LaunchFailed { detail, .. } => {
                    return Err(WorkGraphFlowBridgeError::LaunchFailed {
                        binding_id: binding.binding_id,
                        detail,
                    });
                }
                WorkExecutionLifecycleEffect::RetryEligibilityClassified { .. } => {
                    return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                        "execution binding {} recovery emitted a classifier effect",
                        binding.binding_id
                    )));
                }
            }
        }

        Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
            "execution binding {} exceeded the bounded reconciliation transition budget",
            binding.binding_id
        )))
    }

    async fn ensure_flow_binding_launched(
        &self,
        binding: &WorkExecutionBinding,
    ) -> Result<MobRun, WorkGraphFlowBridgeError> {
        let (mob_id, flow_id, run_id, activation_params) = binding_target(binding)?;
        let authority = execution_authority(binding)?;
        let intent = delivery_intent(binding)?;

        // Recovery observes the exact deterministic run before attempting a
        // fresh principal-gated Begin. A grant revoked after the run committed
        // must not hide that run and launder it into a launch failure.
        if let Some(run) = self.mob_flow_status(binding).await? {
            let run = verify_run_matches_binding(run, binding)?;
            return self
                .admit_observed_external_flow_run(binding, &authority, &intent, run)
                .await;
        }

        let observation_authority = observation_authority(binding)?;
        if let Some(record) = self
            .mob_load_external_delivery(&observation_authority, &intent.identity.idempotency_key)
            .await?
        {
            if record.intent != intent {
                return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "delivery ledger authority differs for execution binding {}",
                    binding.binding_id
                )));
            }
            return match record.phase {
                MobExternalDeliveryPhase::Begun { .. } => {
                    Err(WorkGraphFlowBridgeError::AmbiguousLaunch {
                        binding_id: binding.binding_id.clone(),
                        run_id: run_id.to_string(),
                    })
                }
                MobExternalDeliveryPhase::Realizing { .. } => {
                    Err(WorkGraphFlowBridgeError::LaunchQuarantined {
                        binding_id: binding.binding_id.clone(),
                        detail: format!(
                            "durable realization fence was crossed before run {run_id} became observable"
                        ),
                    })
                }
                MobExternalDeliveryPhase::Terminal {
                    terminal: MobExternalDeliveryTerminal::Completed,
                } => Err(WorkGraphFlowBridgeError::RunMissing {
                    binding_id: binding.binding_id.clone(),
                    run_id: run_id.to_string(),
                }),
                MobExternalDeliveryPhase::Terminal {
                    terminal: MobExternalDeliveryTerminal::Failed { detail, .. },
                } => Err(WorkGraphFlowBridgeError::LaunchFailed {
                    binding_id: binding.binding_id.clone(),
                    detail,
                }),
            };
        }

        let begin = self
            .mob_begin_external_delivery(&authority, &intent)
            .await
            .map_err(|error| {
                if error.failure_class() == crate::MobFailureClass::TargetMissing
                    || matches!(&error, MobError::ScopeDenied(_))
                {
                    WorkGraphFlowBridgeError::LaunchFailed {
                        binding_id: binding.binding_id.clone(),
                        detail: error.to_string(),
                    }
                } else {
                    WorkGraphFlowBridgeError::Mob(error)
                }
            })?;
        match begin {
            MobExternalDeliveryBeginOutcome::Begun => {}
            MobExternalDeliveryBeginOutcome::ExistingBegun { .. } => {
                return Err(WorkGraphFlowBridgeError::AmbiguousLaunch {
                    binding_id: binding.binding_id.clone(),
                    run_id: run_id.to_string(),
                });
            }
            MobExternalDeliveryBeginOutcome::ExistingRealizing {
                run_id: realizing_run_id,
                ..
            } => {
                return Err(WorkGraphFlowBridgeError::LaunchQuarantined {
                    binding_id: binding.binding_id.clone(),
                    detail: format!(
                        "durable realization fence names run {realizing_run_id} but exact run {run_id} is absent"
                    ),
                });
            }
            MobExternalDeliveryBeginOutcome::ExistingTerminal(
                MobExternalDeliveryTerminal::Completed,
            ) => {
                return Err(WorkGraphFlowBridgeError::RunMissing {
                    binding_id: binding.binding_id.clone(),
                    run_id: run_id.to_string(),
                });
            }
            MobExternalDeliveryBeginOutcome::ExistingTerminal(
                MobExternalDeliveryTerminal::Failed { detail, .. },
            ) => {
                return Err(WorkGraphFlowBridgeError::LaunchFailed {
                    binding_id: binding.binding_id.clone(),
                    detail,
                });
            }
        }

        let current = self.host.workgraph_flow_config(&authority).await?;
        let current_digest = current.definition_digest()?;
        let WorkExecutionTarget::MobFlow {
            flow_config_digest, ..
        } = &binding.target;
        if current_digest != *flow_config_digest {
            let detail = format!(
                "Flow definition {} changed after execution binding {} was committed",
                flow_id, binding.binding_id
            );
            self.mob_complete_external_delivery(
                &authority,
                &intent,
                &MobExternalDeliveryTerminal::failed(&MobError::Internal(detail.clone())),
            )
            .await?;
            return Err(WorkGraphFlowBridgeError::LaunchFailed {
                binding_id: binding.binding_id.clone(),
                detail,
            });
        }

        let launched = match self
            .mob_run_flow_with_external_identity(&authority, activation_params, &intent)
            .await
        {
            Ok(MobExternalFlowLaunchOutcome::Started(launched)) => launched,
            Ok(MobExternalFlowLaunchOutcome::Uncertain { detail }) => {
                if let Some(run) = self.mob_flow_status(binding).await? {
                    let run = verify_run_matches_binding(run, binding)?;
                    return self
                        .admit_observed_external_flow_run(binding, &authority, &intent, run)
                        .await;
                }
                return Err(WorkGraphFlowBridgeError::LaunchQuarantined {
                    binding_id: binding.binding_id.clone(),
                    detail,
                });
            }
            Ok(MobExternalFlowLaunchOutcome::ExistingTerminal(
                MobExternalDeliveryTerminal::Completed,
            )) => {
                return Err(WorkGraphFlowBridgeError::RunMissing {
                    binding_id: binding.binding_id.clone(),
                    run_id: run_id.to_string(),
                });
            }
            Ok(MobExternalFlowLaunchOutcome::ExistingTerminal(
                MobExternalDeliveryTerminal::Failed { detail, .. },
            )) => {
                return Err(WorkGraphFlowBridgeError::LaunchFailed {
                    binding_id: binding.binding_id.clone(),
                    detail,
                });
            }
            Err(error) => {
                let detail = error.to_string();
                if let Some(run) = self.mob_flow_status(binding).await? {
                    let run = verify_run_matches_binding(run, binding)?;
                    return self
                        .admit_observed_external_flow_run(binding, &authority, &intent, run)
                        .await;
                }
                self.mob_complete_external_delivery(
                    &authority,
                    &intent,
                    &MobExternalDeliveryTerminal::failed(&error),
                )
                .await?;
                return Err(WorkGraphFlowBridgeError::LaunchFailed {
                    binding_id: binding.binding_id.clone(),
                    detail,
                });
            }
        };
        if launched != run_id {
            return Err(WorkGraphFlowBridgeError::ConflictingRun {
                binding_id: binding.binding_id.clone(),
                expected_run_id: run_id.to_string(),
                observed_run_id: launched.to_string(),
            });
        }
        let run = self.mob_flow_status(binding).await?.ok_or_else(|| {
            WorkGraphFlowBridgeError::RunMissing {
                binding_id: binding.binding_id.clone(),
                run_id: run_id.to_string(),
            }
        })?;
        let run = verify_run_matches_binding(run, binding)?;
        self.admit_observed_external_flow_run(binding, &authority, &intent, run)
            .await
    }

    async fn admit_observed_external_flow_run(
        &self,
        binding: &WorkExecutionBinding,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        run: MobRun,
    ) -> Result<MobRun, WorkGraphFlowBridgeError> {
        let observation = observation_authority(binding)?;
        let record = self
            .mob_load_external_delivery(&observation, &intent.identity.idempotency_key)
            .await?
            .ok_or_else(|| {
                WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "execution binding {} has an exact run but no external-delivery ledger",
                    binding.binding_id
                ))
            })?;
        if record.intent != *intent {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "delivery ledger authority differs for execution binding {}",
                binding.binding_id
            )));
        }
        match record.phase {
            MobExternalDeliveryPhase::Terminal {
                terminal: MobExternalDeliveryTerminal::Completed,
            } => Ok(run),
            MobExternalDeliveryPhase::Terminal {
                terminal: MobExternalDeliveryTerminal::Failed { detail, .. },
            } => Err(WorkGraphFlowBridgeError::LaunchFailed {
                binding_id: binding.binding_id.clone(),
                detail,
            }),
            MobExternalDeliveryPhase::Realizing {
                run_id: realizing_run_id,
                ..
            } if realizing_run_id == run.run_id && run.status != MobRunStatus::Pending => {
                self.mob_complete_external_flow_realization(authority, intent, &run)
                    .await?;
                Ok(run)
            }
            MobExternalDeliveryPhase::Realizing {
                run_id: realizing_run_id,
                ..
            } => Err(WorkGraphFlowBridgeError::LaunchQuarantined {
                binding_id: binding.binding_id.clone(),
                detail: format!(
                    "external delivery is realizing run {realizing_run_id}, but exact run {} has not durably crossed Pending",
                    run.run_id
                ),
            }),
            MobExternalDeliveryPhase::Begun { .. } => {
                let (_, _, _, activation_params) = binding_target(binding)?;
                let _ = self
                    .mob_run_flow_with_external_identity(authority, activation_params, intent)
                    .await?;
                let repaired = self
                    .mob_load_external_delivery(&observation, &intent.identity.idempotency_key)
                    .await?
                    .ok_or_else(|| {
                        WorkGraphFlowBridgeError::InvalidBinding(format!(
                            "execution binding {} lost its delivery ledger during repair",
                            binding.binding_id
                        ))
                    })?;
                match repaired.phase {
                    MobExternalDeliveryPhase::Terminal {
                        terminal: MobExternalDeliveryTerminal::Completed,
                    } => Ok(run),
                    MobExternalDeliveryPhase::Terminal {
                        terminal: MobExternalDeliveryTerminal::Failed { detail, .. },
                    } => Err(WorkGraphFlowBridgeError::LaunchFailed {
                        binding_id: binding.binding_id.clone(),
                        detail,
                    }),
                    phase => Err(WorkGraphFlowBridgeError::LaunchQuarantined {
                        binding_id: binding.binding_id.clone(),
                        detail: format!(
                            "exact run {} repair left delivery in phase {phase:?}",
                            run.run_id
                        ),
                    }),
                }
            }
        }
    }

    async fn flow_run_for_binding(
        &self,
        binding: &WorkExecutionBinding,
    ) -> Result<MobRun, WorkGraphFlowBridgeError> {
        let (_, _, run_id, _) = binding_target(binding)?;
        let run = self.mob_flow_status(binding).await?.ok_or_else(|| {
            WorkGraphFlowBridgeError::RunMissing {
                binding_id: binding.binding_id.clone(),
                run_id: run_id.to_string(),
            }
        })?;
        let run = verify_run_matches_binding(run, binding)?;
        let authority = execution_authority(binding)?;
        let intent = delivery_intent(binding)?;
        self.admit_observed_external_flow_run(binding, &authority, &intent, run)
            .await
    }

    async fn flow_run_for_binding_optional(
        &self,
        binding: &WorkExecutionBinding,
    ) -> Result<Option<MobRun>, WorkGraphFlowBridgeError> {
        match self.mob_flow_status(binding).await {
            Ok(Some(run)) => {
                let run = verify_run_matches_binding(run, binding)?;
                let authority = execution_authority(binding)?;
                let intent = delivery_intent(binding)?;
                self.admit_observed_external_flow_run(binding, &authority, &intent, run)
                    .await
                    .map(Some)
            }
            Ok(None) => Ok(None),
            Err(error) if error.failure_class() == crate::MobFailureClass::TargetMissing => {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn delivery_intent(
    binding: &WorkExecutionBinding,
) -> Result<MobExternalDeliveryIntent, WorkGraphFlowBridgeError> {
    let (mob_id, _, _, _) = binding_target(binding)?;
    let identity = MobDeliveryIdentity::new(
        binding.binding_id.as_str().to_string(),
        binding.correlation_id.clone(),
    )
    .map_err(MobError::from)?;
    let action = serde_json::to_vec(&binding.target).map_err(|error| {
        WorkGraphFlowBridgeError::InvalidBinding(format!(
            "execution target serialization failed: {error}"
        ))
    })?;
    MobExternalDeliveryIntent::new(
        mob_id,
        identity,
        MobExternalDeliveryTargetKind::Flow,
        &action,
    )
    .map_err(MobError::from)
    .map_err(Into::into)
}

async fn observe_flow_run(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: WorkExecutionBinding,
    run: &MobRun,
) -> Result<WorkExecutionBinding, WorkGraphFlowBridgeError> {
    let observation = match run.status {
        MobRunStatus::Pending | MobRunStatus::Running => WorkExecutionObservation::FlowRunning,
        MobRunStatus::Completed => WorkExecutionObservation::FlowCompleted,
        MobRunStatus::Failed => WorkExecutionObservation::FlowFailed {
            detail: flow_failure_detail(run),
        },
        MobRunStatus::Canceled => WorkExecutionObservation::FlowCanceled {
            detail: flow_failure_detail(run),
        },
    };
    observe_binding(workgraph, &binding, observation).await
}

async fn observe_binding(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: &WorkExecutionBinding,
    observation: WorkExecutionObservation,
) -> Result<WorkExecutionBinding, WorkGraphFlowBridgeError> {
    let observation = normalize_execution_observation(observation);
    Ok(workgraph
        .observe_execution(
            Some(binding.work_ref.realm_id.clone()),
            Some(binding.work_ref.namespace.clone()),
            binding.binding_id.clone(),
            binding.machine_state.revision,
            observation,
        )
        .await?
        .binding)
}

fn normalize_execution_observation(
    observation: WorkExecutionObservation,
) -> WorkExecutionObservation {
    fn detail(value: String) -> String {
        const MAX_BYTES: usize = 4096;
        let mut normalized = String::with_capacity(value.len().min(MAX_BYTES));
        let mut previous_space = false;
        for ch in value.chars() {
            let ch = if ch.is_control() || ch.is_whitespace() {
                ' '
            } else {
                ch
            };
            if ch == ' ' && previous_space {
                continue;
            }
            if normalized.len() + ch.len_utf8() > MAX_BYTES {
                break;
            }
            previous_space = ch == ' ';
            normalized.push(ch);
        }
        normalized.trim().to_string()
    }

    match observation {
        WorkExecutionObservation::FlowFailed { detail: value } => {
            WorkExecutionObservation::FlowFailed {
                detail: value.map(detail),
            }
        }
        WorkExecutionObservation::FlowCanceled { detail: value } => {
            WorkExecutionObservation::FlowCanceled {
                detail: value.map(detail),
            }
        }
        WorkExecutionObservation::FlowRunLost { detail: value } => {
            WorkExecutionObservation::FlowRunLost {
                detail: detail(value),
            }
        }
        WorkExecutionObservation::LaunchUncertain { detail: value } => {
            WorkExecutionObservation::LaunchUncertain {
                detail: detail(value),
            }
        }
        WorkExecutionObservation::LaunchQuarantined { detail: value } => {
            WorkExecutionObservation::LaunchQuarantined {
                detail: detail(value),
            }
        }
        WorkExecutionObservation::LaunchFailed { detail: value } => {
            WorkExecutionObservation::LaunchFailed {
                detail: detail(value),
            }
        }
        WorkExecutionObservation::WorkClosureRefused { detail: value } => {
            WorkExecutionObservation::WorkClosureRefused {
                detail: detail(value),
            }
        }
        other => other,
    }
}

async fn execution_result(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: WorkExecutionBinding,
    run: Option<MobRun>,
) -> Result<WorkGraphFlowReconcileResult, WorkGraphFlowBridgeError> {
    let item = workgraph
        .get(
            Some(binding.work_ref.realm_id.clone()),
            Some(binding.work_ref.namespace.clone()),
            binding.work_ref.item_id.clone(),
        )
        .await?;
    let evidence_projected = item.evidence_refs.iter().any(|evidence| {
        evidence.execution_binding_id.as_ref() == Some(&binding.binding_id)
            && evidence.id == binding.evidence_id()
    });
    let work_item_closed = WorkGraphMachine::classify_terminality(&item)?;
    Ok(WorkGraphFlowReconcileResult {
        binding,
        run,
        item,
        evidence_projected,
        work_item_closed,
    })
}

async fn project_flow_evidence(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: &WorkExecutionBinding,
    run: &MobRun,
    kind: WorkExecutionEvidenceKind,
) -> Result<(), WorkGraphFlowBridgeError> {
    let outcome = match kind {
        WorkExecutionEvidenceKind::Completed => "completed",
        WorkExecutionEvidenceKind::Failed => "failed",
        WorkExecutionEvidenceKind::Canceled => "canceled",
        WorkExecutionEvidenceKind::LaunchFailed | WorkExecutionEvidenceKind::RunLost => {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "execution evidence class {kind:?} is not a Flow terminal outcome"
            )));
        }
    };
    workgraph
        .project_execution_evidence(
            Some(binding.work_ref.realm_id.clone()),
            Some(binding.work_ref.namespace.clone()),
            binding.binding_id.clone(),
            WorkExecutionEvidenceProjection {
                kind,
                label: Some(format!("Mob Flow {} / {}", run.mob_id, run.flow_id)),
                summary: Some(format!(
                    "Flow run {} {outcome} with {} root outputs, {} loop output groups, and {} recorded failures",
                    run.run_id,
                    run.root_step_outputs.len(),
                    run.loop_iteration_outputs.len(),
                    run.failure_ledger.len()
                )),
            },
        )
        .await?;
    Ok(())
}

async fn project_observed_terminal_evidence(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: &WorkExecutionBinding,
    run: Option<&MobRun>,
    kind: WorkExecutionEvidenceKind,
) -> Result<(), WorkGraphFlowBridgeError> {
    if let Some(run) = run {
        return project_flow_evidence(workgraph, binding, run, kind).await;
    }
    let outcome = match kind {
        WorkExecutionEvidenceKind::Failed => "failed",
        WorkExecutionEvidenceKind::Canceled => "was canceled",
        other => {
            return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
                "execution evidence class {other:?} is not an observed terminal outcome"
            )));
        }
    };
    let WorkExecutionTarget::MobFlow {
        mob_id, flow_id, ..
    } = &binding.target;
    workgraph
        .project_execution_evidence(
            Some(binding.work_ref.realm_id.clone()),
            Some(binding.work_ref.namespace.clone()),
            binding.binding_id.clone(),
            WorkExecutionEvidenceProjection {
                kind,
                label: Some(format!("Mob Flow {mob_id} / {flow_id}")),
                summary: Some(format!(
                    "Flow run {} {outcome}; the terminal run record was unavailable during evidence recovery",
                    binding.target.run_id()
                )),
            },
        )
        .await?;
    Ok(())
}

async fn project_launch_failure_evidence(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: &WorkExecutionBinding,
    detail: &str,
) -> Result<(), WorkGraphFlowBridgeError> {
    workgraph
        .project_execution_evidence(
            Some(binding.work_ref.realm_id.clone()),
            Some(binding.work_ref.namespace.clone()),
            binding.binding_id.clone(),
            WorkExecutionEvidenceProjection {
                kind: WorkExecutionEvidenceKind::LaunchFailed,
                label: Some("Mob Flow launch failed".to_string()),
                summary: Some(detail.to_string()),
            },
        )
        .await?;
    Ok(())
}

async fn project_run_lost_evidence(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: &WorkExecutionBinding,
    detail: &str,
) -> Result<(), WorkGraphFlowBridgeError> {
    workgraph
        .project_execution_evidence(
            Some(binding.work_ref.realm_id.clone()),
            Some(binding.work_ref.namespace.clone()),
            binding.binding_id.clone(),
            WorkExecutionEvidenceProjection {
                kind: WorkExecutionEvidenceKind::RunLost,
                label: Some("Mob Flow run lost".to_string()),
                summary: Some(detail.to_string()),
            },
        )
        .await?;
    Ok(())
}

async fn execution_evidence_projected(
    workgraph: &meerkat::WorkExecutionBridge,
    binding: &WorkExecutionBinding,
) -> Result<bool, WorkGraphFlowBridgeError> {
    Ok(workgraph
        .execution_evidence(
            Some(binding.work_ref.realm_id.clone()),
            Some(binding.work_ref.namespace.clone()),
            binding.binding_id.clone(),
        )
        .await?
        .is_some())
}

fn flow_failure_detail(run: &MobRun) -> Option<String> {
    let count = run.failure_ledger.len();
    (count > 0).then(|| format!("Flow run {} recorded {count} failures", run.run_id))
}

fn binding_target(
    binding: &WorkExecutionBinding,
) -> Result<(MobId, FlowId, RunId, serde_json::Value), WorkGraphFlowBridgeError> {
    match &binding.target {
        WorkExecutionTarget::MobFlow {
            mob_id,
            flow_id,
            run_id,
            activation_params,
            ..
        } => Ok((
            MobId::from(mob_id.as_str()),
            FlowId::from(flow_id.as_str()),
            RunId::from_str(run_id).map_err(|error| {
                WorkGraphFlowBridgeError::InvalidBinding(format!(
                    "flow run id '{run_id}' is invalid: {error}"
                ))
            })?,
            activation_params.clone(),
        )),
    }
}

fn execution_authority(
    binding: &WorkExecutionBinding,
) -> Result<WorkGraphFlowExecutionAuthority, WorkGraphFlowBridgeError> {
    let (mob_id, flow_id, run_id, _) = binding_target(binding)?;
    let WorkExecutionTarget::MobFlow {
        execution_authority,
        ..
    } = &binding.target;
    Ok(WorkGraphFlowExecutionAuthority {
        binding_id: binding.binding_id.clone(),
        mob_id,
        flow_id,
        run_id,
        execution_authority: execution_authority.clone(),
    })
}

fn observation_authority(
    binding: &WorkExecutionBinding,
) -> Result<WorkGraphFlowObservationAuthority, WorkGraphFlowBridgeError> {
    let authority = execution_authority(binding)?;
    Ok(WorkGraphFlowObservationAuthority {
        binding_id: binding.binding_id.clone(),
        mob_id: authority.mob_id,
        run_id: authority.run_id,
    })
}

fn admission_matches_binding(
    admission: &WorkGraphFlowAdmission,
    binding: &WorkExecutionBinding,
) -> Result<bool, WorkGraphFlowBridgeError> {
    let WorkExecutionTarget::MobFlow {
        flow_config_digest,
        execution_authority,
        ..
    } = &binding.target;
    Ok(admission.config.definition_digest()? == *flow_config_digest
        && (admission.execution_authority == WorkExecutionAuthority::TargetOwner
            || admission.execution_authority == *execution_authority))
}

fn verify_run_matches_binding(
    run: MobRun,
    binding: &WorkExecutionBinding,
) -> Result<MobRun, WorkGraphFlowBridgeError> {
    let WorkExecutionTarget::MobFlow {
        mob_id,
        flow_id,
        flow_config_digest,
        run_id,
        activation_params,
        ..
    } = &binding.target;
    if run.mob_id.as_str() != mob_id
        || run.flow_id.as_str() != flow_id
        || run.run_id.to_string() != *run_id
        || run.activation_params != *activation_params
        || run.flow_definition_digest.as_deref() != Some(flow_config_digest.as_str())
    {
        return Err(WorkGraphFlowBridgeError::InvalidBinding(format!(
            "Mob run {} does not match execution binding {} identity, definition digest, or parameters",
            run.run_id, binding.binding_id
        )));
    }
    Ok(run)
}

fn request_matches_binding(
    existing: &WorkExecutionBinding,
    request: &LaunchWorkGraphFlowRequest,
) -> bool {
    let expected_delivery_key = existing.binding_id.as_str();
    let expected_run =
        RunId::for_work_execution(&request.mob_id, &request.flow_id, expected_delivery_key);
    existing.work_ref.item_id == request.work_item_id
        && existing.idempotency_key == request.idempotency_key
        && existing.supersedes == request.supersedes
        && matches!(
            &existing.target,
            WorkExecutionTarget::MobFlow {
                mob_id,
                flow_id,
                run_id,
                activation_params,
                ..
            } if mob_id == request.mob_id.as_str()
                && flow_id == request.flow_id.as_str()
                && run_id == &expected_run.to_string()
                && activation_params == &request.activation_params
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_observation_detail_is_bounded_and_single_line() {
        let hostile = format!("first\nsecond\0{}", "x".repeat(5000));
        let WorkExecutionObservation::LaunchFailed { detail } =
            normalize_execution_observation(WorkExecutionObservation::LaunchFailed {
                detail: hostile,
            })
        else {
            panic!("normalizer changed observation kind");
        };
        assert!(detail.len() <= 4096);
        assert!(!detail.chars().any(char::is_control));
        assert!(detail.starts_with("first second "));
    }
}
