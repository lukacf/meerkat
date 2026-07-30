use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat_core::{ContentInput, Session, SessionId, skills::SkillRef, types::RenderMetadata};
use meerkat_runtime::{
    CompletionHandle, MeerkatMachine, SessionServiceRuntimeExt,
    completion::{CompletionOutcome, CompletionWaitError},
};
use meerkat_schedule::{
    DeliveryCompletion, DeliveryCompletionFailureReason, DeliveryDispatch, DeliveryFailureReason,
    DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal, HostRunnableInvocation,
    HostRunnableParams, HostRunnableTargetBinding, IdentityTargetBinding, MobTargetBinding,
    Occurrence, OccurrencePhase, RunnableProbe, ScheduleDeliveryIdentity, ScheduleDomainError,
    ScheduleDriver, ScheduleDriverConfig, ScheduleFilter, ScheduleRunnableHost, ScheduleService,
    ScheduleStoreKind, ScheduleStoreWakeMode, ScheduleTargetDelivery, ScheduleTargetProbe,
    ScheduledSessionAction, SessionMaterializationSpec, SessionTargetBinding, TargetBinding,
    TargetProbeOutcome, UpdateScheduleRequest,
};
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
use tokio as schedule_host_tokio;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::oneshot;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias as schedule_host_tokio;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::oneshot;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::task::JoinHandle;

pub struct ScheduleHostHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl ScheduleHostHandle {
    /// Whether the host supervisor task is still alive.
    ///
    /// Worker failures do not flip this while the supervisor is applying its
    /// bounded restart policy. A finished supervisor is observable by surface
    /// owners, which can replace the stale handle.
    pub fn is_running(&self) -> bool {
        !self.join.is_finished()
    }

    pub async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Err(error) = self.join.await {
            tracing::error!(%error, "schedule host supervisor task terminated");
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedScheduledSession {
    session_id: SessionId,
    materialized_session_id: Option<SessionId>,
}

enum AcceptedScheduledInputCompletion {
    Handle(CompletionHandle),
    Terminal(CompletionOutcome),
    AuthorityUnavailable { detail: String },
}

/// Exact target-side admission result for one stable schedule delivery
/// identity. Keeping this narrower than `RuntimeDeliveryOutcome` prevents
/// completion/failure outcomes from being stamped into a DispatchAccepted
/// receipt by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleAdmissionOutcome {
    Accepted,
    Deduplicated,
}

impl ScheduleAdmissionOutcome {
    fn runtime_outcome(self) -> meerkat_schedule::RuntimeDeliveryOutcome {
        match self {
            Self::Accepted => meerkat_schedule::RuntimeDeliveryOutcome::AdmissionAccepted,
            Self::Deduplicated => meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated,
        }
    }
}

pub struct AcceptedScheduledInput {
    correlation_id: Option<String>,
    admission_outcome: ScheduleAdmissionOutcome,
    completion: AcceptedScheduledInputCompletion,
}

impl AcceptedScheduledInput {
    pub fn with_runtime_handle(correlation_id: Option<String>, handle: CompletionHandle) -> Self {
        Self {
            correlation_id,
            admission_outcome: ScheduleAdmissionOutcome::Accepted,
            completion: AcceptedScheduledInputCompletion::Handle(handle),
        }
    }

    pub fn with_authority_unavailable(
        correlation_id: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            correlation_id,
            admission_outcome: ScheduleAdmissionOutcome::Accepted,
            completion: AcceptedScheduledInputCompletion::AuthorityUnavailable {
                detail: detail.into(),
            },
        }
    }

    pub(crate) fn with_runtime_terminal(
        correlation_id: Option<String>,
        terminal: CompletionOutcome,
    ) -> Self {
        Self {
            correlation_id,
            admission_outcome: ScheduleAdmissionOutcome::Accepted,
            completion: AcceptedScheduledInputCompletion::Terminal(terminal),
        }
    }

    pub fn with_admission_outcome(mut self, outcome: ScheduleAdmissionOutcome) -> Self {
        self.admission_outcome = outcome;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledPromptDispatch {
    pub prompt: ContentInput,
    /// Host-regenerated request-only context for this occurrence. Schedule
    /// definitions must not persist this value; it is attached at delivery.
    pub transient_turn_context: Option<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
    pub system_prompt: Option<String>,
    pub render_metadata: Option<RenderMetadata>,
    pub skill_refs: Vec<SkillRef>,
    pub additional_instructions: Vec<String>,
    pub materialized_session_id: Option<SessionId>,
}

#[derive(Serialize)]
struct MobMemberScheduleIdentityKey<'a> {
    schema: &'static str,
    mob_id: &'a str,
    member: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct MobMemberScheduleIdentity {
    pub mob_id: String,
    pub member: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OwnedMobMemberScheduleIdentityKey {
    schema: String,
    mob_id: String,
    member: String,
}

pub fn mob_member_schedule_identity(binding: &meerkat_core::MobMemberBinding) -> String {
    let key = MobMemberScheduleIdentityKey {
        schema: "meerkat.schedule.mob_member_identity.v2",
        mob_id: &binding.mob_id,
        member: &binding.member,
    };
    let json = serde_json::to_string(&key).unwrap_or_else(|_| {
        format!(
            "{{\"schema\":\"meerkat.schedule.mob_member_identity.v2\",\"mob_id\":\"{}\",\"member\":\"{}\"}}",
            binding.mob_id, binding.member
        )
    });
    format!("mob_member:{json}")
}

#[derive(Debug, Clone)]
pub struct MobMemberCurrentSessionScheduleResolver {
    binding: meerkat_core::MobMemberBinding,
}

impl MobMemberCurrentSessionScheduleResolver {
    pub fn new(binding: meerkat_core::MobMemberBinding) -> Self {
        Self { binding }
    }

    pub fn binding(&self) -> &meerkat_core::MobMemberBinding {
        &self.binding
    }
}

impl meerkat_schedule::CurrentSessionScheduleTargetResolver
    for MobMemberCurrentSessionScheduleResolver
{
    fn resolve_current_session_target(
        &self,
        _current_session_id: &SessionId,
        action: ScheduledSessionAction,
    ) -> TargetBinding {
        TargetBinding::identity(IdentityTargetBinding::resumable(
            mob_member_schedule_identity(&self.binding),
            action,
        ))
    }
}

#[allow(dead_code)]
pub fn parse_mob_member_schedule_identity(identity: &str) -> Option<MobMemberScheduleIdentity> {
    let json = identity.strip_prefix("mob_member:")?;
    let key: OwnedMobMemberScheduleIdentityKey = serde_json::from_str(json).ok()?;
    match key.schema.as_str() {
        "meerkat.schedule.mob_member_identity.v2" => Some(MobMemberScheduleIdentity {
            mob_id: key.mob_id,
            member: key.member,
        }),
        _ => None,
    }
}

pub fn recover_mob_member_identity_from_session_target(
    binding: &SessionTargetBinding,
    session: Option<&Session>,
) -> Option<IdentityTargetBinding> {
    let SessionTargetBinding::ResumableSession { action, .. } = binding else {
        return None;
    };
    let owner = session
        .and_then(Session::session_metadata)
        .and_then(|metadata| metadata.mob_member_binding)?;
    Some(IdentityTargetBinding::resumable(
        mob_member_schedule_identity(&owner),
        action.clone(),
    ))
}

#[async_trait]
pub trait SurfaceScheduleSessionHost: Send + Sync {
    async fn probe_session_target(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError>;

    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let _ = binding;
        Ok(TargetProbeOutcome::Missing {
            detail: Some(
                "scheduled identity targets are not supported by this session host".to_string(),
            ),
        })
    }

    async fn resolve_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<SessionId>, ScheduleDomainError> {
        let _ = binding;
        Ok(None)
    }

    async fn recover_session_target_identity(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<Option<IdentityTargetBinding>, ScheduleDomainError> {
        let _ = binding;
        Ok(None)
    }

    /// Materialize the on-demand session for `occurrence`.
    ///
    /// The session id MUST be derived deterministically from the occurrence
    /// identity (via [`Occurrence::materialized_session_id`]) so a redrive of
    /// the same occurrence reuses the existing session instead of minting a
    /// second orphan. Implementations are required to be create-or-reuse: a
    /// second materialize for an occurrence whose deterministic session id
    /// already exists is a no-op reuse, never a duplicate and never an error.
    /// Prompt-action System content is deliberately absent from this seam: it
    /// is an ordinary message delivered exactly once at the turn boundary.
    async fn materialize_session(
        &self,
        occurrence: &Occurrence,
        create: &SessionMaterializationSpec,
    ) -> Result<SessionId, ScheduleDomainError>;

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;

    #[allow(clippy::too_many_arguments)]
    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;
}

#[async_trait]
pub trait SurfaceScheduleMobHost: Send + Sync {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError>;

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;

    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
        let _ = binding;
        Ok(None)
    }

    /// Resolve a stable identity owned by the mob host into its exact current
    /// session. Returning `None` delegates to the ordinary session host.
    /// Implementations must project actor-owned residency rather than search
    /// durable sessions heuristically.
    async fn resolve_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<SessionId>, ScheduleDomainError> {
        let _ = binding;
        Ok(None)
    }

    async fn deliver_identity_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        let _ = (occurrence, identity, binding);
        Ok(None)
    }
}

pub struct NoopScheduleMobHost {
    detail: String,
}

impl NoopScheduleMobHost {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

#[async_trait]
impl SurfaceScheduleMobHost for NoopScheduleMobHost {
    async fn probe_mob_target(
        &self,
        _binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        Ok(TargetProbeOutcome::Missing {
            detail: Some(self.detail.clone()),
        })
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        _binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        Ok(immediate_delivery_failure(
            occurrence,
            self.detail.clone(),
            DeliveryFailureReason::MobRejected,
            Some(identity.correlation_id.clone()),
            None,
        ))
    }

    async fn probe_identity_target(
        &self,
        _binding: &IdentityTargetBinding,
    ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
        Ok(None)
    }

    async fn deliver_identity_target(
        &self,
        _occurrence: &Occurrence,
        _identity: &ScheduleDeliveryIdentity,
        _binding: &IdentityTargetBinding,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        Ok(None)
    }
}

pub struct SharedScheduleTargetAdapter {
    schedule_service: ScheduleService,
    session_host: Arc<dyn SurfaceScheduleSessionHost>,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
}

impl SharedScheduleTargetAdapter {
    pub fn new(
        schedule_service: ScheduleService,
        session_host: Arc<dyn SurfaceScheduleSessionHost>,
        mob_host: Arc<dyn SurfaceScheduleMobHost>,
    ) -> Self {
        Self {
            schedule_service,
            session_host,
            mob_host,
            runnable_host: None,
        }
    }

    /// Attach a host-runnable registry to this adapter.
    ///
    /// Default is no registry: `host_runnable` targets then probe `Missing`
    /// and deliveries fail with `TargetMissing`.
    pub fn with_runnable_host(mut self, runnable_host: Arc<dyn ScheduleRunnableHost>) -> Self {
        self.runnable_host = Some(runnable_host);
        self
    }

    async fn resolve_session(
        &self,
        occurrence: &Occurrence,
        delivery_identity: &ScheduleDeliveryIdentity,
        binding: &SessionTargetBinding,
    ) -> Result<ResolvedScheduledSession, DeliveryDispatch> {
        match binding {
            SessionTargetBinding::ExactSession { session_id, .. }
            | SessionTargetBinding::ResumableSession { session_id, .. } => {
                if let Ok(TargetProbeOutcome::Missing { .. }) =
                    self.session_host.probe_session_target(binding).await
                {
                    let recovered = self
                        .session_host
                        .recover_session_target_identity(binding)
                        .await
                        .map_err(|error| {
                            immediate_delivery_failure(
                                occurrence,
                                error.to_string(),
                                DeliveryFailureReason::TargetMaterializationFailed,
                                Some(delivery_identity.correlation_id.clone()),
                                None,
                            )
                        })?;
                    if let Some(identity) = recovered {
                        if let Some(dispatch) = self
                            .mob_host
                            .deliver_identity_target(occurrence, delivery_identity, &identity)
                            .await
                            .map_err(|error| {
                                immediate_delivery_failure(
                                    occurrence,
                                    error.to_string(),
                                    DeliveryFailureReason::TargetMaterializationFailed,
                                    Some(delivery_identity.correlation_id.clone()),
                                    None,
                                )
                            })?
                        {
                            return Err(dispatch);
                        }
                        return self
                            .resolve_identity(occurrence, delivery_identity, &identity)
                            .await;
                    }
                }
                Ok(ResolvedScheduledSession {
                    session_id: session_id.clone(),
                    materialized_session_id: None,
                })
            }
            SessionTargetBinding::MaterializeOnDemandSession {
                bound_session_id: Some(session_id),
                ..
            } => Ok(ResolvedScheduledSession {
                session_id: session_id.clone(),
                materialized_session_id: Some(session_id.clone()),
            }),
            SessionTargetBinding::MaterializeOnDemandSession {
                create,
                action: _,
                bound_session_id: None,
            } => {
                // Layer B: defensive contractual reuse guard. The in-flight
                // occurrence snapshot can be stale — a prior attempt may have
                // committed the bound id to the authoritative schedule target
                // (and pending occurrences) after this snapshot was claimed.
                // Re-read the authoritative binding for THIS occurrence before
                // materializing; if a session is already bound, reuse it and
                // never mint a second one.
                if let Some(bound) = self.authoritative_bound_session_id(occurrence).await {
                    return Ok(ResolvedScheduledSession {
                        session_id: bound.clone(),
                        materialized_session_id: Some(bound),
                    });
                }
                match self
                    .session_host
                    .materialize_session(occurrence, create)
                    .await
                {
                    Ok(session_id) => {
                        if let Err(error) = self
                            .schedule_service
                            .bind_materialized_session_for_occurrence(occurrence, &session_id)
                            .await
                        {
                            return Err(immediate_delivery_failure(
                                occurrence,
                                error.to_string(),
                                DeliveryFailureReason::InternalError,
                                Some(delivery_identity.correlation_id.clone()),
                                Some(session_id),
                            ));
                        }
                        Ok(ResolvedScheduledSession {
                            session_id: session_id.clone(),
                            materialized_session_id: Some(session_id),
                        })
                    }
                    Err(error) => Err(immediate_delivery_failure(
                        occurrence,
                        error.to_string(),
                        DeliveryFailureReason::TargetMaterializationFailed,
                        Some(delivery_identity.correlation_id.clone()),
                        None,
                    )),
                }
            }
        }
    }

    /// Re-read the authoritative bound session id for `occurrence`.
    ///
    /// `bind_materialized_session_for_occurrence` commits the materialized id
    /// to the schedule target (and pending occurrences). After a prior attempt
    /// committed that bind but died before the in-flight snapshot was synced,
    /// the occurrence handed to `resolve_session` can still report
    /// `bound_session_id: None`. This consults the freshest authoritative
    /// state — the re-read occurrence first, then the schedule target — so the
    /// adapter never materializes a second session for an already-bound
    /// occurrence. A read failure is treated as "no authoritative binding
    /// known": the caller falls through to deterministic-id materialization,
    /// which is itself create-or-reuse, so no orphan can result.
    async fn authoritative_bound_session_id(&self, occurrence: &Occurrence) -> Option<SessionId> {
        let store = self.schedule_service.store();

        if let Ok(Some(current)) = store.get_occurrence(&occurrence.occurrence_id).await
            && let TargetBinding::Session(binding) = &current.target_snapshot
            && let Some(session_id) = binding.resolved_session_id()
        {
            return Some(session_id.clone());
        }

        if let Ok(Some(schedule)) = store.get_schedule(&occurrence.schedule_id).await
            && schedule.revision == occurrence.schedule_revision
            && let TargetBinding::Session(binding) = &schedule.target
            && let Some(session_id) = binding.resolved_session_id()
        {
            return Some(session_id.clone());
        }

        None
    }

    async fn resolve_identity(
        &self,
        occurrence: &Occurrence,
        delivery_identity: &ScheduleDeliveryIdentity,
        binding: &IdentityTargetBinding,
    ) -> Result<ResolvedScheduledSession, DeliveryDispatch> {
        let mob_owned = self
            .mob_host
            .resolve_identity_target(binding)
            .await
            .map_err(|error| {
                immediate_delivery_failure(
                    occurrence,
                    error.to_string(),
                    DeliveryFailureReason::TargetMaterializationFailed,
                    Some(delivery_identity.correlation_id.clone()),
                    None,
                )
            })?;
        let resolved = match mob_owned {
            Some(session_id) => Ok(Some(session_id)),
            None => self.session_host.resolve_identity_target(binding).await,
        };
        match resolved {
            Ok(Some(session_id)) => Ok(ResolvedScheduledSession {
                session_id,
                materialized_session_id: None,
            }),
            Ok(None) => Err(immediate_delivery_failure(
                occurrence,
                format!(
                    "scheduled identity target not found: {}",
                    binding.identity()
                ),
                DeliveryFailureReason::TargetMaterializationFailed,
                Some(delivery_identity.correlation_id.clone()),
                None,
            )),
            Err(error) => Err(immediate_delivery_failure(
                occurrence,
                error.to_string(),
                DeliveryFailureReason::TargetMaterializationFailed,
                Some(delivery_identity.correlation_id.clone()),
                None,
            )),
        }
    }

    /// Explicit one-time compatibility boundary for released schedules whose
    /// durable target is a recoverable Session id rather than an identity.
    ///
    /// This intentionally performs catalog-wide work and therefore must be
    /// invoked by an owning activation/migration transaction, never by the
    /// long-lived schedule worker or its restart supervisor. The caller owns
    /// recording completion before starting the worker.
    pub async fn migrate_recoverable_session_targets(&self) -> Result<usize, ScheduleDomainError> {
        let schedules = self
            .schedule_service
            .store()
            .list_schedules(ScheduleFilter {
                include_deleted: false,
                ..ScheduleFilter::default()
            })
            .await?;
        let mut migrated = 0usize;

        for schedule in schedules {
            let TargetBinding::Session(binding) = &schedule.target else {
                continue;
            };
            let Some(identity) = self
                .session_host
                .recover_session_target_identity(binding)
                .await?
            else {
                continue;
            };
            self.schedule_service
                .update(
                    &schedule.schedule_id,
                    UpdateScheduleRequest {
                        expected_revision: Some(schedule.revision),
                        target: Some(TargetBinding::identity(identity)),
                        ..UpdateScheduleRequest::default()
                    },
                )
                .await?;
            migrated += 1;
        }

        Ok(migrated)
    }

    async fn deliver_session_action(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        resolved: ResolvedScheduledSession,
        action: &ScheduledSessionAction,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match action {
            ScheduledSessionAction::Prompt {
                prompt,
                system_prompt,
                render_metadata,
                skill_refs,
                additional_instructions,
            } => {
                self.session_host
                    .deliver_prompt(
                        &resolved.session_id,
                        occurrence,
                        identity,
                        ScheduledPromptDispatch {
                            prompt: prompt.clone(),
                            transient_turn_context: None,
                            system_prompt: system_prompt.clone(),
                            render_metadata: render_metadata.clone(),
                            skill_refs: skill_refs.clone(),
                            additional_instructions: additional_instructions.clone(),
                            materialized_session_id: resolved.materialized_session_id,
                        },
                    )
                    .await
            }
            ScheduledSessionAction::Event {
                event_type,
                payload,
                render_metadata,
            } => {
                self.session_host
                    .deliver_event(
                        &resolved.session_id,
                        occurrence,
                        identity,
                        event_type.clone(),
                        payload.clone(),
                        render_metadata.clone(),
                        resolved.materialized_session_id,
                    )
                    .await
            }
        }
    }

    /// Dispatch a `host_runnable` target through the in-process runnable seam.
    ///
    /// Failure mapping for an in-process callback (deliberate decision):
    /// - unregistered runnable or no configured registry → `TargetMissing`
    ///   (the named target does not exist on this host);
    /// - a `HostRunnableError` returned by the callback → `RuntimeRejected`
    ///   (the executing runtime refused or failed the work);
    /// - `TransportError` is deliberately NOT reachable: there is no
    ///   transport hop in an in-process invocation, so no outcome can
    ///   honestly be a transport fault. (Counter-precedent: mob targets own
    ///   the target-kind-specific `MobRejected` reason; host runnables map
    ///   onto the existing shared reasons instead of minting a new
    ///   machine-vocabulary variant.)
    fn deliver_host_runnable(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &HostRunnableTargetBinding,
    ) -> DeliveryDispatch {
        let Some(runnable_host) = &self.runnable_host else {
            return immediate_delivery_failure(
                occurrence,
                format!(
                    "host runnable '{}' is unavailable: no runnable registry is configured on this surface",
                    binding.runnable
                ),
                DeliveryFailureReason::TargetMissing,
                Some(identity.correlation_id.clone()),
                None,
            );
        };
        if runnable_host.probe_runnable(&binding.runnable) == RunnableProbe::Unknown {
            return immediate_delivery_failure(
                occurrence,
                format!("host runnable '{}' is not registered", binding.runnable),
                DeliveryFailureReason::TargetMissing,
                Some(identity.correlation_id.clone()),
                None,
            );
        }

        let invocation = HostRunnableInvocation {
            occurrence_id: occurrence.occurrence_id.clone(),
            schedule_id: occurrence.schedule_id.clone(),
            delivery_idempotency_key: identity.idempotency_key.clone(),
            runnable: binding.runnable.clone(),
            trigger_time: occurrence.due_at_utc,
            params: binding.params.clone().map(HostRunnableParams::into_raw),
        };
        let runnable_host = Arc::clone(runnable_host);
        async_completion_dispatch(
            occurrence,
            Some(identity.correlation_id.clone()),
            Box::pin(async move {
                Ok(match runnable_host.run_occurrence(invocation).await {
                    Ok(_) => DeliveryTerminal::completed(None),
                    // Unregistered is the same semantic condition the probe
                    // reports as Unknown: one condition, one terminal class,
                    // regardless of where it is detected.
                    Err(error @ meerkat_schedule::HostRunnableError::Unregistered { .. }) => {
                        DeliveryTerminal::delivery_failed(
                            error.to_string(),
                            DeliveryFailureReason::TargetMissing,
                        )
                    }
                    Err(error) => DeliveryTerminal::delivery_failed(
                        error.to_string(),
                        DeliveryFailureReason::RuntimeRejected,
                    ),
                })
            }),
        )
    }
}

#[async_trait]
impl ScheduleTargetProbe for SharedScheduleTargetAdapter {
    async fn probe_target(
        &self,
        occurrence: &Occurrence,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                let probe = self.session_host.probe_session_target(binding).await?;
                if matches!(probe, TargetProbeOutcome::Missing { .. })
                    && let Some(identity) = self
                        .session_host
                        .recover_session_target_identity(binding)
                        .await?
                {
                    if let Some(probe) = self.mob_host.probe_identity_target(&identity).await? {
                        return Ok(probe);
                    }
                    return self.session_host.probe_identity_target(&identity).await;
                }
                Ok(probe)
            }
            TargetBinding::Identity(binding) => {
                if let Some(probe) = self.mob_host.probe_identity_target(binding).await? {
                    return Ok(probe);
                }
                self.session_host.probe_identity_target(binding).await
            }
            TargetBinding::Mob(binding) => self.mob_host.probe_mob_target(binding).await,
            TargetBinding::HostRunnable(binding) => {
                let Some(runnable_host) = &self.runnable_host else {
                    return Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!(
                            "host runnable '{}' is unavailable: no runnable registry is configured on this surface",
                            binding.runnable
                        )),
                    });
                };
                Ok(match runnable_host.probe_runnable(&binding.runnable) {
                    RunnableProbe::Registered => TargetProbeOutcome::Ready,
                    RunnableProbe::Unknown => TargetProbeOutcome::Missing {
                        detail: Some(format!(
                            "host runnable '{}' is not registered",
                            binding.runnable
                        )),
                    },
                })
            }
        }
    }
}

#[async_trait]
impl ScheduleTargetDelivery for SharedScheduleTargetAdapter {
    async fn deliver_occurrence(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                let resolved = match self.resolve_session(occurrence, identity, binding).await {
                    Ok(resolved) => resolved,
                    Err(dispatch) => return Ok(dispatch),
                };

                self.deliver_session_action(occurrence, identity, resolved, binding.action())
                    .await
            }
            TargetBinding::Identity(binding) => {
                if let Some(dispatch) = self
                    .mob_host
                    .deliver_identity_target(occurrence, identity, binding)
                    .await?
                {
                    return Ok(dispatch);
                }
                let resolved = match self.resolve_identity(occurrence, identity, binding).await {
                    Ok(resolved) => resolved,
                    Err(dispatch) => return Ok(dispatch),
                };
                self.deliver_session_action(occurrence, identity, resolved, binding.action())
                    .await
            }
            TargetBinding::Mob(binding) => {
                self.mob_host
                    .deliver_mob_target(occurrence, identity, binding)
                    .await
            }
            TargetBinding::HostRunnable(binding) => {
                Ok(self.deliver_host_runnable(occurrence, identity, binding))
            }
        }
    }
}

pub fn schedule_host_supported(kind: ScheduleStoreKind) -> bool {
    !matches!(kind, ScheduleStoreKind::Disabled | ScheduleStoreKind::Jsonl)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduleHostIncidentClass {
    DriverTickFailed,
    DriverRowFaults,
    NextActionReadFailed,
    DurableWakeFailed,
    WorkerExited,
    WorkerPanicked,
}

impl ScheduleHostIncidentClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::DriverTickFailed => "driver_tick_failed",
            Self::DriverRowFaults => "driver_row_faults",
            Self::NextActionReadFailed => "next_action_read_failed",
            Self::DurableWakeFailed => "durable_wake_failed",
            Self::WorkerExited => "worker_exited",
            Self::WorkerPanicked => "worker_panicked",
        }
    }
}

#[derive(Debug)]
struct ScheduleHostIncident {
    class: ScheduleHostIncidentClass,
    detail: String,
}

impl ScheduleHostIncident {
    fn new(class: ScheduleHostIncidentClass, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
        }
    }
}

/// Host-health bookkeeping keyed by a bounded incident class, never by
/// volatile row ids, page contents, or transport prose. Detail is refreshed
/// for the next heartbeat, but a changing detail inside the same class cannot
/// punch through the rate limiter and create an ERROR/page flood.
struct ScheduleHostIncidentTracker {
    class: Option<ScheduleHostIncidentClass>,
    /// Observations in the CURRENT incident class (heartbeat counter).
    consecutive: u64,
    /// Total unhealthy observations since the loop was last healthy, across
    /// class changes — the outage length reported on recovery.
    outage_observations: u64,
    last_logged: Option<meerkat_core::time_compat::Instant>,
    heartbeat_every: std::time::Duration,
}

#[derive(Debug)]
enum ScheduleHostHealthLog {
    Quiet,
    /// A new incident class: log at ERROR.
    Incident {
        class: ScheduleHostIncidentClass,
        detail: String,
    },
    /// The same class persists: rate-limited WARN heartbeat with the latest
    /// bounded detail.
    Heartbeat {
        class: ScheduleHostIncidentClass,
        detail: String,
        consecutive: u64,
    },
    /// The loop recovered after `after` unhealthy observations.
    Recovered {
        class: ScheduleHostIncidentClass,
        after: u64,
    },
}

impl ScheduleHostIncidentTracker {
    fn new(heartbeat_every: std::time::Duration) -> Self {
        Self {
            class: None,
            consecutive: 0,
            outage_observations: 0,
            last_logged: None,
            heartbeat_every,
        }
    }

    fn observe(
        &mut self,
        incident: Option<ScheduleHostIncident>,
        now: meerkat_core::time_compat::Instant,
    ) -> ScheduleHostHealthLog {
        match incident {
            None => {
                let after = self.outage_observations;
                let class = self.class.take();
                self.consecutive = 0;
                self.outage_observations = 0;
                self.last_logged = None;
                match class {
                    Some(class) => ScheduleHostHealthLog::Recovered { class, after },
                    None => ScheduleHostHealthLog::Quiet,
                }
            }
            Some(incident) => {
                self.outage_observations += 1;
                self.consecutive += 1;
                let class_changed = self.class != Some(incident.class);
                if class_changed {
                    self.class = Some(incident.class);
                    self.consecutive = 1;
                    self.last_logged = Some(now);
                    return ScheduleHostHealthLog::Incident {
                        class: incident.class,
                        detail: incident.detail,
                    };
                }
                if self
                    .last_logged
                    .is_none_or(|last| now.duration_since(last) >= self.heartbeat_every)
                {
                    self.last_logged = Some(now);
                    return ScheduleHostHealthLog::Heartbeat {
                        class: incident.class,
                        detail: incident.detail,
                        consecutive: self.consecutive,
                    };
                }
                ScheduleHostHealthLog::Quiet
            }
        }
    }
}

fn log_schedule_host_health(action: ScheduleHostHealthLog) {
    match action {
        ScheduleHostHealthLog::Quiet => {}
        ScheduleHostHealthLog::Incident { class, detail } => {
            tracing::error!(
                incident_class = class.as_str(),
                %detail,
                "schedule host entered an unhealthy state"
            );
        }
        ScheduleHostHealthLog::Heartbeat {
            class,
            detail,
            consecutive,
        } => {
            tracing::warn!(
                incident_class = class.as_str(),
                %detail,
                consecutive,
                "schedule host incident persists"
            );
        }
        ScheduleHostHealthLog::Recovered { class, after } => {
            tracing::info!(
                incident_class = class.as_str(),
                after,
                "schedule host recovered"
            );
        }
    }
}

/// Consecutive no-progress ticks tolerated at the base interval before the
/// host starts backing off (errors escalate immediately). Sits beside the
/// driver policy literals (`claim_limit`, `lease_duration`) in
/// [`spawn_schedule_host`] as the host's tick-pacing policy.
const IDLE_TICKS_BEFORE_BACKOFF: u32 = 8;

const MAX_FAULT_LOG_SAMPLES: usize = 3;
const MAX_FAULT_LOG_SAMPLE_CHARS: usize = 512;

/// Exponential tick pacing for the schedule host loop (2026-07-29 incident:
/// a failing or no-progress tick retried at a fixed 4Hz forever — on a
/// remote store that is query spam priced in currency; a BigQuery-store
/// consumer flagged it as a parity blocker).
///
/// Policy: a failing tick escalates immediately; a run of
/// `idle_grace_ticks` no-progress ticks starts escalating too (doubling,
/// capped); any healthy tick that moves work snaps back to the base interval.
struct TickBackoff {
    base: Duration,
    cap: Duration,
    idle_grace_ticks: u32,
    current: Duration,
    consecutive_unproductive: u32,
}

impl TickBackoff {
    fn new(base: Duration, cap: Duration, idle_grace_ticks: u32) -> Self {
        Self {
            base,
            cap,
            idle_grace_ticks,
            current: base,
            consecutive_unproductive: 0,
        }
    }

    /// Observe one tick outcome and return the delay to sleep before the
    /// next tick.
    #[cfg(test)]
    fn observe(
        &mut self,
        outcome: &Result<
            meerkat_schedule::ScheduleTickReport,
            meerkat_schedule::ScheduleDomainError,
        >,
    ) -> Duration {
        match outcome {
            Err(_) => self.observe_failure(),
            Ok(report) => self.observe_report(report),
        }
        self.current
    }

    fn observe_failure(&mut self) {
        // A failing tick is retried with immediate escalation: at the fixed
        // interval every retry hammered the failing store.
        self.consecutive_unproductive = self.consecutive_unproductive.saturating_add(1);
        self.escalate();
    }

    fn observe_report(&mut self, report: &meerkat_schedule::ScheduleTickReport) {
        if report.made_progress() {
            self.consecutive_unproductive = 0;
            self.current = self.base;
        } else {
            // Successful but unproductive: keep the fast cadence through the
            // grace run for exact due work, then back off. Idle durable stores
            // use their declared wake mode rather than this cadence.
            self.consecutive_unproductive = self.consecutive_unproductive.saturating_add(1);
            if self.consecutive_unproductive > self.idle_grace_ticks {
                self.escalate();
            }
        }
    }

    fn escalate(&mut self) {
        self.current = self.current.saturating_mul(2).min(self.cap);
    }
}

struct RestartBackoff {
    base: Duration,
    cap: Duration,
    next: Duration,
}

impl RestartBackoff {
    fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            next: base,
        }
    }

    fn after_failure(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.cap);
        delay
    }

    fn reset(&mut self) {
        self.next = self.base;
    }
}

fn duration_until_store_action(
    action: meerkat_schedule::ScheduleStoreActionTime,
) -> Option<Duration> {
    action.next_action_at_utc.map(|next_action_at_utc| {
        next_action_at_utc
            .signed_duration_since(action.store_now_utc)
            .to_std()
            .unwrap_or(Duration::ZERO)
    })
}

fn idle_delay_for_store_action(
    action: meerkat_schedule::ScheduleStoreActionTime,
    wake_mode: ScheduleStoreWakeMode,
    backoff_delay: Duration,
    base_interval: Duration,
) -> Option<Duration> {
    let until_action = duration_until_store_action(action);
    match wake_mode {
        ScheduleStoreWakeMode::ProcessLocal | ScheduleStoreWakeMode::Push => match until_action {
            Some(delay) if delay.is_zero() => Some(backoff_delay),
            Some(delay) => Some(delay),
            None => None,
        },
        ScheduleStoreWakeMode::BoundedPoll { max_interval } => {
            // A zero custom interval cannot create a hot loop. A non-zero
            // declaration is the store's exact maximum convergence interval,
            // including intervals tighter than the host's ordinary base
            // cadence, and must never be silently widened.
            let poll_interval = if max_interval.is_zero() {
                base_interval
            } else {
                max_interval
            };
            match until_action {
                Some(delay) if delay.is_zero() => Some(backoff_delay),
                Some(delay) => Some(delay.min(poll_interval)),
                None => Some(poll_interval),
            }
        }
    }
}

fn incident_from_tick_report(
    report: &meerkat_schedule::ScheduleTickReport,
) -> Option<ScheduleHostIncident> {
    (report.fault_count() > 0).then(|| {
        ScheduleHostIncident::new(
            ScheduleHostIncidentClass::DriverRowFaults,
            format!(
                "tick degraded with {} fault(s):\n{}",
                report.fault_count(),
                report.bounded_fault_summary(MAX_FAULT_LOG_SAMPLES, MAX_FAULT_LOG_SAMPLE_CHARS)
            ),
        )
    })
}

async fn optional_schedule_host_sleep(delay: Option<Duration>) {
    match delay {
        Some(delay) => schedule_host_tokio::time::sleep(delay).await,
        None => std::future::pending::<()>().await,
    }
}

async fn wait_for_durable_store_wake(
    store: Arc<dyn meerkat_schedule::ScheduleStore>,
    wake_mode: ScheduleStoreWakeMode,
) -> Result<(), meerkat_schedule::ScheduleStoreError> {
    match wake_mode {
        ScheduleStoreWakeMode::Push => store.wait_for_durable_wake().await,
        ScheduleStoreWakeMode::ProcessLocal | ScheduleStoreWakeMode::BoundedPoll { .. } => {
            std::future::pending::<Result<(), meerkat_schedule::ScheduleStoreError>>().await
        }
    }
}

#[derive(Debug)]
enum ScheduleHostWorkerExit {
    DurableWakeFailed(meerkat_schedule::ScheduleStoreError),
    MutationSignalClosed,
}

impl ScheduleHostWorkerExit {
    fn into_incident(self) -> ScheduleHostIncident {
        match self {
            Self::DurableWakeFailed(error) => ScheduleHostIncident::new(
                ScheduleHostIncidentClass::DurableWakeFailed,
                format!("durable schedule wake failed: {error}"),
            ),
            Self::MutationSignalClosed => ScheduleHostIncident::new(
                ScheduleHostIncidentClass::WorkerExited,
                "process-local schedule mutation signal closed",
            ),
        }
    }
}

async fn run_schedule_host_worker(
    schedule_service: ScheduleService,
    driver: Arc<ScheduleDriver>,
    wake_mode: ScheduleStoreWakeMode,
    base_interval: Duration,
    backoff_cap: Duration,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), ScheduleHostWorkerExit> {
    let mut health = ScheduleHostIncidentTracker::new(std::time::Duration::from_secs(60));

    let mut backoff = TickBackoff::new(base_interval, backoff_cap, IDLE_TICKS_BEFORE_BACKOFF);
    let mut delay = Some(base_interval);
    let mut mutation_rx = schedule_service.subscribe_mutations();
    let store = schedule_service.store();

    loop {
        schedule_host_tokio::select! {
            _ = &mut shutdown_rx => return Ok(()),
            mutation = mutation_rx.changed() => {
                if mutation.is_err() {
                    return Err(ScheduleHostWorkerExit::MutationSignalClosed);
                }
            }
            durable_wake = wait_for_durable_store_wake(Arc::clone(&store), wake_mode) => {
                if let Err(error) = durable_wake {
                    return Err(ScheduleHostWorkerExit::DurableWakeFailed(error));
                }
            }
            () = optional_schedule_host_sleep(delay) => {}
        }

        let outcome = driver.tick_once().await;
        let (next_delay, incident) = match outcome {
            Err(error) => {
                backoff.observe_failure();
                (
                    Some(backoff.current),
                    Some(ScheduleHostIncident::new(
                        ScheduleHostIncidentClass::DriverTickFailed,
                        format!("schedule driver tick failed: {error}"),
                    )),
                )
            }
            Ok(report) if report.made_progress() => {
                let incident = incident_from_tick_report(&report);
                if incident.is_some() {
                    backoff.observe_failure();
                } else {
                    backoff.observe_report(&report);
                }
                (Some(backoff.current), incident)
            }
            Ok(report) => match store.next_action_time_utc().await {
                Err(error) => {
                    backoff.observe_failure();
                    (
                        Some(backoff.current),
                        Some(ScheduleHostIncident::new(
                            ScheduleHostIncidentClass::NextActionReadFailed,
                            format!("could not read next durable action time: {error}"),
                        )),
                    )
                }
                Ok(action) => {
                    let incident = incident_from_tick_report(&report);
                    if incident.is_some() {
                        backoff.observe_failure();
                    } else {
                        backoff.observe_report(&report);
                    }
                    (
                        idle_delay_for_store_action(
                            action,
                            wake_mode,
                            backoff.current,
                            base_interval,
                        ),
                        incident,
                    )
                }
            },
        };
        log_schedule_host_health(
            health.observe(incident, meerkat_core::time_compat::Instant::now()),
        );
        delay = next_delay;
    }
}

async fn supervise_schedule_host(
    schedule_service: ScheduleService,
    driver: Arc<ScheduleDriver>,
    wake_mode: ScheduleStoreWakeMode,
    base_interval: Duration,
    backoff_cap: Duration,
    stable_window: Duration,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let mut health = ScheduleHostIncidentTracker::new(std::time::Duration::from_secs(60));
    let mut restart_backoff = RestartBackoff::new(base_interval, backoff_cap);

    loop {
        let (worker_shutdown_tx, worker_shutdown_rx) = oneshot::channel();
        let mut worker = schedule_host_tokio::task::spawn(run_schedule_host_worker(
            schedule_service.clone(),
            Arc::clone(&driver),
            wake_mode,
            base_interval,
            backoff_cap,
            worker_shutdown_rx,
        ));
        let mut worker_shutdown_tx = Some(worker_shutdown_tx);
        let mut stable_timer = Box::pin(schedule_host_tokio::time::sleep(stable_window));
        let mut stable = false;

        let worker_result = loop {
            schedule_host_tokio::select! {
                _ = &mut shutdown_rx => {
                    if let Some(shutdown_tx) = worker_shutdown_tx.take() {
                        let _ = shutdown_tx.send(());
                    }
                    let _ = worker.await;
                    return;
                }
                result = &mut worker => break result,
                () = &mut stable_timer, if !stable => {
                    stable = true;
                    restart_backoff.reset();
                    log_schedule_host_health(health.observe(
                        None,
                        meerkat_core::time_compat::Instant::now(),
                    ));
                }
            }
        };
        let incident = match worker_result {
            Ok(Ok(())) => ScheduleHostIncident::new(
                ScheduleHostIncidentClass::WorkerExited,
                "schedule host worker exited without supervisor shutdown",
            ),
            Ok(Err(exit)) => exit.into_incident(),
            Err(error) => ScheduleHostIncident::new(
                ScheduleHostIncidentClass::WorkerPanicked,
                format!("schedule host worker task terminated: {error}"),
            ),
        };
        log_schedule_host_health(
            health.observe(Some(incident), meerkat_core::time_compat::Instant::now()),
        );

        let restart_delay = restart_backoff.after_failure();
        schedule_host_tokio::select! {
            _ = &mut shutdown_rx => return,
            () = schedule_host_tokio::time::sleep(restart_delay) => {}
        }
    }
}

pub fn spawn_schedule_host(
    schedule_service: ScheduleService,
    adapter: Arc<SharedScheduleTargetAdapter>,
    owner_id: impl Into<String>,
) -> ScheduleHostHandle {
    let driver = Arc::new(ScheduleDriver::new(
        schedule_service.clone(),
        schedule_service.store(),
        adapter.clone(),
        adapter,
        owner_id,
        ScheduleDriverConfig {
            claim_limit: 32,
            lease_duration: ChronoDuration::seconds(60),
        },
    ));
    let wake_mode = schedule_service.store().wake_mode();
    // Tick pacing: base interval while work flows, exponential backoff to
    // the cap while ticks fail or make no progress (see `TickBackoff`). The
    // in-crate test profile keeps both bounds small so host tests never wait
    // on an escalated interval.
    let (base_interval, backoff_cap, stable_window) = if cfg!(test) {
        (
            Duration::from_millis(50),
            Duration::from_millis(400),
            Duration::from_millis(200),
        )
    } else {
        (
            Duration::from_millis(250),
            Duration::from_secs(30),
            Duration::from_secs(60),
        )
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let join = schedule_host_tokio::task::spawn(supervise_schedule_host(
        schedule_service,
        driver,
        wake_mode,
        base_interval,
        backoff_cap,
        stable_window,
        shutdown_rx,
    ));

    ScheduleHostHandle {
        shutdown_tx: Some(shutdown_tx),
        join,
    }
}

pub fn build_dispatch_from_accepted(
    occurrence: &Occurrence,
    accepted: AcceptedScheduledInput,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = accepted.correlation_id.clone();
    receipt.runtime_outcome = Some(accepted.admission_outcome.runtime_outcome());
    receipt.materialized_session_id = materialized_session_id.clone();

    let completion = schedule_completion_from_runtime_completion(
        accepted.completion,
        materialized_session_id.clone(),
    );

    DeliveryDispatch {
        receipt,
        correlation_id: accepted.correlation_id,
        materialized_session_id,
        completion,
    }
}

fn schedule_completion_from_runtime_completion(
    completion: AcceptedScheduledInputCompletion,
    materialized_session_id: Option<SessionId>,
) -> DeliveryCompletion {
    Box::pin(async move {
        let outcome = match completion {
            AcceptedScheduledInputCompletion::Handle(handle) => match handle.try_wait().await {
                Ok(outcome) => outcome,
                Err(error) => {
                    return Err(ScheduleDomainError::DeliveryCompletionFailed {
                        reason: completion_wait_failure_reason(&error),
                        detail: format!("runtime completion authority unavailable: {error}"),
                    });
                }
            },
            AcceptedScheduledInputCompletion::Terminal(terminal) => {
                return Ok(delivery_terminal_from_completion_outcome(
                    terminal,
                    materialized_session_id,
                ));
            }
            AcceptedScheduledInputCompletion::AuthorityUnavailable { detail } => {
                return Err(ScheduleDomainError::DeliveryCompletionFailed {
                    reason: DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable,
                    detail,
                });
            }
        };
        Ok(delivery_terminal_from_completion_outcome(
            outcome,
            materialized_session_id,
        ))
    })
}

fn completion_wait_failure_reason(error: &CompletionWaitError) -> DeliveryCompletionFailureReason {
    match error {
        CompletionWaitError::ChannelClosed => {
            DeliveryCompletionFailureReason::RuntimeCompletionChannelClosed
        }
        CompletionWaitError::AttachmentReplaced | CompletionWaitError::AuthorityUnavailable(_) => {
            DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
        }
    }
}

fn delivery_terminal_from_completion_outcome(
    outcome: CompletionOutcome,
    _materialized_session_id: Option<SessionId>,
) -> DeliveryTerminal {
    match outcome {
        CompletionOutcome::Completed(_) | CompletionOutcome::CompletedWithoutResult => {
            DeliveryTerminal::runtime_completion(
                meerkat_schedule::RuntimeCompletionOutcome::Completed,
                None,
                None,
            )
        }
        CompletionOutcome::CallbackPending {
            tool_name, args, ..
        } => {
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionCallbackPending {
                    tool_name,
                    payload: args,
                };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::CallbackPending,
                runtime_outcome,
            )
        }
        CompletionOutcome::CallbackBatchPending { pending_tool_calls } => {
            let first = pending_tool_calls.first();
            let tool_name = first
                .map(|call| call.tool_name.clone())
                .unwrap_or_else(|| "callback_batch".to_string());
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionCallbackPending {
                    tool_name,
                    payload: serde_json::json!({
                        "pending_tool_calls": pending_tool_calls,
                    }),
                };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::CallbackPending,
                runtime_outcome,
            )
        }
        CompletionOutcome::Cancelled => {
            let runtime_outcome = meerkat_schedule::RuntimeDeliveryOutcome::CompletionAbandoned {
                detail: "request cancelled".to_string(),
            };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::Cancelled,
                runtime_outcome,
            )
        }
        CompletionOutcome::Abandoned { reason, error }
        | CompletionOutcome::AbandonedWithError { reason, error } => {
            let error_detail =
                serde_json::to_string(&error).unwrap_or_else(|_| "<unserializable>".to_string());
            let runtime_outcome = meerkat_schedule::RuntimeDeliveryOutcome::CompletionAbandoned {
                detail: format!("{reason}; error={error_detail}"),
            };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::Abandoned,
                runtime_outcome,
            )
        }
        CompletionOutcome::CompletedWithFinalizationFailure { error, .. } => {
            let detail = serde_json::to_string(&error)
                .unwrap_or_else(|_| "turn finalization failed".to_string());
            DeliveryTerminal::runtime_completion(
                meerkat_schedule::RuntimeCompletionOutcome::FinalizationFailed,
                Some(detail),
                None,
            )
        }
        CompletionOutcome::RuntimeTerminated { reason, error } => {
            let error_detail =
                serde_json::to_string(&error).unwrap_or_else(|_| "<unserializable>".to_string());
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionRuntimeTerminated {
                    detail: format!("{reason}; error={error_detail}"),
                };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::RuntimeTerminated,
                runtime_outcome,
            )
        }
    }
}

fn terminal_from_runtime_completion_outcome(
    outcome: meerkat_schedule::RuntimeCompletionOutcome,
    runtime_outcome: meerkat_schedule::RuntimeDeliveryOutcome,
) -> DeliveryTerminal {
    let detail = runtime_outcome.detail();
    DeliveryTerminal::runtime_completion(outcome, Some(detail), Some(runtime_outcome))
}

pub fn immediate_completed_dispatch(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
) -> DeliveryDispatch {
    immediate_completed_dispatch_with_admission_outcome(
        occurrence,
        correlation_id,
        ScheduleAdmissionOutcome::Accepted,
    )
}

pub fn immediate_completed_dispatch_with_admission_outcome(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
    admission_outcome: ScheduleAdmissionOutcome,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
    receipt.runtime_outcome = Some(admission_outcome.runtime_outcome());
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: None,
        completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
    }
}

pub fn async_completion_dispatch(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
    completion: DeliveryCompletion,
) -> DeliveryDispatch {
    async_completion_dispatch_with_admission_outcome(
        occurrence,
        correlation_id,
        ScheduleAdmissionOutcome::Accepted,
        completion,
    )
}

pub fn async_completion_dispatch_with_admission_outcome(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
    admission_outcome: ScheduleAdmissionOutcome,
    completion: DeliveryCompletion,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
    receipt.runtime_outcome = Some(admission_outcome.runtime_outcome());
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: None,
        completion,
    }
}

pub fn immediate_delivery_failure(
    occurrence: &Occurrence,
    detail: String,
    failure_reason: DeliveryFailureReason,
    correlation_id: Option<String>,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchStarted,
    );
    receipt.correlation_id = correlation_id.clone();
    receipt.materialized_session_id = materialized_session_id.clone();
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id,
        completion: Box::pin(async move {
            Ok(DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(detail),
                delivery_failure_reason: Some(failure_reason),
                runtime_completion_outcome: None,
                runtime_outcome: None,
            })
        }),
    }
}

/// Project one runtime admission into the schedule driver's typed delivery
/// contract.
///
/// Runtime intentionally returns no live completion handle when an accepted
/// or deduplicated input is already terminal. That is success with a durable
/// terminal witness, not missing authority. This single adapter path asks the
/// runtime for the machine-authorized exact rich completion receipt and
/// replays it so every schedule surface preserves the same result class and
/// payload. A coarse input-terminal class is never treated as completion
/// authority.
pub async fn runtime_delivery_dispatch_from_admission(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    outcome: meerkat_runtime::accept::AcceptOutcome,
    handle: Option<CompletionHandle>,
    materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    let (input_id, admission_outcome) = match outcome {
        meerkat_runtime::accept::AcceptOutcome::Accepted { input_id, .. } => {
            (input_id, ScheduleAdmissionOutcome::Accepted)
        }
        meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
            (existing_id, ScheduleAdmissionOutcome::Deduplicated)
        }
        meerkat_runtime::accept::AcceptOutcome::Rejected { reason } => {
            return Ok(immediate_delivery_failure(
                occurrence,
                reason.to_string(),
                DeliveryFailureReason::RuntimeRejected,
                Some(identity.correlation_id.clone()),
                materialized_session_id,
            ));
        }
        _ => {
            return Ok(immediate_delivery_failure(
                occurrence,
                "runtime returned an unknown admission outcome".to_string(),
                DeliveryFailureReason::RuntimeRejected,
                Some(identity.correlation_id.clone()),
                materialized_session_id,
            ));
        }
    };
    let correlation_id = Some(identity.correlation_id.clone());

    let accepted = match handle {
        Some(handle) => AcceptedScheduledInput::with_runtime_handle(correlation_id.clone(), handle),
        None => {
            match runtime_adapter
                .input_terminal_completion(session_id, &input_id)
                .await
            {
                Ok(Some(terminal)) => {
                    AcceptedScheduledInput::with_runtime_terminal(correlation_id.clone(), terminal)
                }
                Ok(None) => AcceptedScheduledInput::with_authority_unavailable(
                    correlation_id.clone(),
                    format!(
                        "runtime returned no completion handle and no exact terminal completion receipt for input {input_id}"
                    ),
                ),
                Err(error) => AcceptedScheduledInput::with_authority_unavailable(
                    correlation_id.clone(),
                    format!(
                        "runtime could not provide the exact terminal completion receipt for input {input_id}: {error}"
                    ),
                ),
            }
        }
    }
    .with_admission_outcome(admission_outcome);

    Ok(build_dispatch_from_accepted(
        occurrence,
        accepted,
        materialized_session_id,
    ))
}

/// Runtime-facing delivery identity for a scheduled occurrence: schedule +
/// occurrence ONLY.
///
/// `attempt_count` is deliberately excluded (2026-07 P0, renamed from
/// `schedule_attempt_idempotency_key`): the runtime dedupes admissions on
/// this exact string, so an attempt-varying key admitted every lease-expiry
/// reclaim as a brand-new input while the previous attempt's turn was still
/// running — a duplicate turn per reclaim. With the occurrence-level key, a
/// retry of a live or already-ran delivery deduplicates at admission and
/// attaches to the existing input's completion instead. Attempt counts and
/// claim tokens remain store-side claim FENCING only (stale-completion
/// screening is unchanged).
pub fn schedule_delivery_idempotency_key(occurrence: &Occurrence) -> String {
    ScheduleDeliveryIdentity::for_occurrence(occurrence).idempotency_key
}

fn schedule_v0810_predecessor_delivery_keys(
    occurrence: &Occurrence,
) -> impl Iterator<Item = String> + '_ {
    (1..occurrence.attempt_count)
        .rev()
        .map(|predecessor_attempt| {
            format!(
                "schedule:{}:occurrence:{}:attempt:{predecessor_attempt}",
                occurrence.schedule_id, occurrence.occurrence_id
            )
        })
}

/// Resolve the one supported schedule-key upgrade boundary.
///
/// Meerkat 0.8.10 keyed a delivery by claim attempt. A redrive under 0.8.11
/// increments the durable attempt before delivery, so any earlier durable
/// attempt key can name work originally admitted by 0.8.10. The resolver
/// walks that finite key family newest-first: this remains correct after
/// repeated 0.8.11 reclaims that reused the same old binding. New occurrences
/// use the driver's 0.8.11 occurrence-stable key. This is intentionally not a
/// general legacy alias mechanism.
pub async fn schedule_runtime_delivery_idempotency_key(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    occurrence: &Occurrence,
    canonical_idempotency_key: &str,
) -> Result<String, ScheduleDomainError> {
    let canonical_key = canonical_idempotency_key.to_owned();
    if occurrence.attempt_count <= 1 {
        return Ok(canonical_key);
    }

    // Prefer the canonical 0.8.11 binding once it exists. Otherwise walk the
    // finite set of prior durable claim attempts newest-first. This remains
    // correct across more than one 0.8.11 crash: attempt N may have reused an
    // attempt-1 key admitted by 0.8.10, so checking only N-1 would lose the
    // bridge on the next reclaim.
    let canonical = runtime_adapter
        .input_state_by_idempotency_key(session_id, &canonical_key)
        .await
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    if canonical.is_some() {
        return Ok(canonical_key);
    }

    for candidate_key in schedule_v0810_predecessor_delivery_keys(occurrence) {
        let predecessor = runtime_adapter
            .input_state_by_idempotency_key(session_id, &candidate_key)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        if predecessor.is_some() {
            if candidate_key != canonical_key {
                tracing::info!(
                    schedule_id = %occurrence.schedule_id,
                    occurrence_id = %occurrence.occurrence_id,
                    predecessor_key = %candidate_key,
                    "reusing Meerkat 0.8.10 schedule delivery identity"
                );
            }
            return Ok(candidate_key);
        }
    }
    Ok(canonical_key)
}

/// Project the driver's canonical string correlation into the runtime's UUID
/// carrier. Parsing here makes the schedule/runtime seam fail closed if those
/// two typed representations ever drift.
pub fn schedule_runtime_correlation_id(
    identity: &ScheduleDeliveryIdentity,
) -> Result<meerkat_runtime::CorrelationId, ScheduleDomainError> {
    let uuid = identity.correlation_id.parse().map_err(|error| {
        ScheduleDomainError::Internal(format!("invalid schedule correlation id: {error}"))
    })?;
    Ok(meerkat_runtime::CorrelationId::from_uuid(uuid))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Detail churn inside one stable class cannot punch through the incident
    /// rate limit. The latest detail is retained for the next heartbeat.
    #[test]
    fn incident_tracker_rate_limits_by_stable_class() {
        let heartbeat = std::time::Duration::from_secs(60);
        let mut tracker = ScheduleHostIncidentTracker::new(heartbeat);
        let start = meerkat_core::time_compat::Instant::now();

        assert!(matches!(
            tracker.observe(None, start),
            ScheduleHostHealthLog::Quiet
        ));

        assert!(matches!(
            tracker.observe(
                Some(ScheduleHostIncident::new(
                    ScheduleHostIncidentClass::DriverTickFailed,
                    "page 1 failed"
                )),
                start
            ),
            ScheduleHostHealthLog::Incident {
                class: ScheduleHostIncidentClass::DriverTickFailed,
                ..
            }
        ));

        // Different volatile detail, same class: still rate limited.
        assert!(matches!(
            tracker.observe(
                Some(ScheduleHostIncident::new(
                    ScheduleHostIncidentClass::DriverTickFailed,
                    "page 2 failed"
                )),
                start + std::time::Duration::from_millis(250)
            ),
            ScheduleHostHealthLog::Quiet
        ));

        match tracker.observe(
            Some(ScheduleHostIncident::new(
                ScheduleHostIncidentClass::DriverTickFailed,
                "page 3 failed",
            )),
            start + heartbeat + std::time::Duration::from_secs(1),
        ) {
            ScheduleHostHealthLog::Heartbeat {
                detail,
                consecutive,
                ..
            } => {
                assert_eq!(consecutive, 3);
                assert_eq!(detail, "page 3 failed");
            }
            other => panic!("expected heartbeat, got {other:?}"),
        }

        // A mechanism/class change is a distinct incident.
        assert!(matches!(
            tracker.observe(
                Some(ScheduleHostIncident::new(
                    ScheduleHostIncidentClass::NextActionReadFailed,
                    "index unavailable"
                )),
                start + heartbeat + std::time::Duration::from_secs(2)
            ),
            ScheduleHostHealthLog::Incident {
                class: ScheduleHostIncidentClass::NextActionReadFailed,
                ..
            }
        ));

        match tracker.observe(None, start + heartbeat + std::time::Duration::from_secs(3)) {
            ScheduleHostHealthLog::Recovered { after, .. } => assert_eq!(after, 4),
            other => panic!("expected recovery, got {other:?}"),
        }
    }

    /// A successful tick with row faults remains degraded, and its operator
    /// detail is bounded before entering the tracker.
    #[test]
    fn row_fault_incident_is_bounded_and_attributable() {
        let heartbeat = std::time::Duration::from_secs(60);
        let mut tracker = ScheduleHostIncidentTracker::new(heartbeat);
        let start = meerkat_core::time_compat::Instant::now();
        let mut report = meerkat_schedule::ScheduleTickReport::default();
        for index in 0..8 {
            report
                .occurrence_row_faults
                .push(meerkat_schedule::ScheduleStoreRowFault {
                    schedule_id: Some("sched-1".to_string()),
                    occurrence_id: Some(format!("occ-{index}")),
                    kind: meerkat_schedule::ScheduleStoreRowFaultKind::Deserialization,
                    detail: "poisoned row".repeat(200),
                });
        }
        let incident = incident_from_tick_report(&report).expect("row faults are unhealthy");

        match tracker.observe(Some(incident), start) {
            ScheduleHostHealthLog::Incident { class, detail } => {
                assert_eq!(class, ScheduleHostIncidentClass::DriverRowFaults);
                assert!(detail.contains("occ-0"), "{detail}");
                assert!(detail.contains("additional fault(s) omitted"), "{detail}");
                assert!(!detail.contains("occ-7"), "{detail}");
                assert!(
                    detail.len() < 2_500,
                    "bounded detail grew to {}",
                    detail.len()
                );
            }
            other => panic!("expected incident, got {other:?}"),
        }
    }

    /// Rotating durable pages remain one row-fault incident class rather than
    /// producing one ERROR per page.
    #[test]
    fn rotating_row_fault_pages_do_not_flood_incidents() {
        let heartbeat = std::time::Duration::from_secs(60);
        let mut tracker = ScheduleHostIncidentTracker::new(heartbeat);
        let start = meerkat_core::time_compat::Instant::now();

        for page in 0..4 {
            let action = tracker.observe(
                Some(ScheduleHostIncident::new(
                    ScheduleHostIncidentClass::DriverRowFaults,
                    format!("fault page {page}"),
                )),
                start + std::time::Duration::from_millis(250 * page),
            );
            if page == 0 {
                assert!(matches!(action, ScheduleHostHealthLog::Incident { .. }));
            } else {
                assert!(matches!(action, ScheduleHostHealthLog::Quiet));
            }
        }
    }

    /// 2026-07-29 incident: a failing tick retried at the fixed 4Hz interval
    /// forever (query spam priced in currency on remote stores). The backoff
    /// must escalate immediately on every failing tick, cap, and snap back
    /// to the base interval on the first progressing tick.
    #[test]
    fn tick_backoff_escalates_on_failing_ticks_and_resets_on_progress() {
        let base = Duration::from_millis(250);
        let cap = Duration::from_secs(30);
        let mut backoff = TickBackoff::new(base, cap, 8);
        let failure: Result<meerkat_schedule::ScheduleTickReport, _> = Err(
            meerkat_schedule::ScheduleDomainError::Internal("store unavailable".to_string()),
        );

        // Each failing tick doubles: 500ms, 1s, 2s, ...
        assert_eq!(backoff.observe(&failure), Duration::from_millis(500));
        assert_eq!(backoff.observe(&failure), Duration::from_millis(1000));
        assert_eq!(backoff.observe(&failure), Duration::from_millis(2000));
        // ... until the cap holds.
        for _ in 0..16 {
            assert!(backoff.observe(&failure) <= cap);
        }
        assert_eq!(backoff.observe(&failure), cap);

        // A progressing tick resets to the base interval at once.
        let progressing: Result<_, meerkat_schedule::ScheduleDomainError> =
            Ok(meerkat_schedule::ScheduleTickReport {
                claimed_occurrences: 1,
                ..meerkat_schedule::ScheduleTickReport::default()
            });
        assert_eq!(backoff.observe(&progressing), base);
        // And the next failure starts escalating from the base again.
        assert_eq!(backoff.observe(&failure), Duration::from_millis(500));
    }

    /// Idle (successful but zero-progress) ticks keep the fast cadence
    /// through the grace run — freshly created work must be picked up
    /// promptly — then escalate, and any progressing tick resets both the
    /// interval and the grace counter.
    #[test]
    fn tick_backoff_tolerates_idle_grace_then_escalates_and_resets() {
        let base = Duration::from_millis(250);
        let cap = Duration::from_secs(30);
        let grace = 3;
        let mut backoff = TickBackoff::new(base, cap, grace);
        let idle: Result<_, meerkat_schedule::ScheduleDomainError> =
            Ok(meerkat_schedule::ScheduleTickReport::default());
        let progressing: Result<_, meerkat_schedule::ScheduleDomainError> =
            Ok(meerkat_schedule::ScheduleTickReport {
                planned_occurrences: 1,
                ..meerkat_schedule::ScheduleTickReport::default()
            });

        // Grace run: the base interval holds for `grace` idle ticks.
        for _ in 0..grace {
            assert_eq!(backoff.observe(&idle), base);
        }
        // Past the grace run, idle ticks escalate.
        assert_eq!(backoff.observe(&idle), Duration::from_millis(500));
        assert_eq!(backoff.observe(&idle), Duration::from_millis(1000));

        // Progress resets the interval AND re-arms the grace run.
        assert_eq!(backoff.observe(&progressing), base);
        for _ in 0..grace {
            assert_eq!(backoff.observe(&idle), base);
        }
        assert_eq!(backoff.observe(&idle), Duration::from_millis(500));
    }

    /// Ask 16 regression, loop-level: the spawned schedule host loop must
    /// FEED tick outcomes into the health tracker and log them. Reverting
    /// the loop body to `let _ = driver.tick_once().await;` (the exact
    /// silent-discard shape the field incident hit) fails this test.
    #[tokio::test]
    async fn spawn_schedule_host_logs_failing_ticks() {
        use uuid::Uuid;

        struct FailingScheduleStore {
            wake_mode: meerkat_schedule::ScheduleStoreWakeMode,
            durable_wake_attempts: Arc<AtomicUsize>,
            list_schedules_attempts: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl ScheduleStore for FailingScheduleStore {
            fn kind(&self) -> meerkat_schedule::ScheduleStoreKind {
                meerkat_schedule::ScheduleStoreKind::Custom
            }

            fn wake_mode(&self) -> meerkat_schedule::ScheduleStoreWakeMode {
                self.wake_mode
            }

            async fn wait_for_durable_wake(
                &self,
            ) -> Result<(), meerkat_schedule::ScheduleStoreError> {
                self.durable_wake_attempts
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic durable wake outage".to_string(),
                ))
            }

            async fn get_store_time_utc(
                &self,
            ) -> Result<chrono::DateTime<chrono::Utc>, meerkat_schedule::ScheduleStoreError>
            {
                Ok(chrono::Utc::now())
            }

            async fn next_action_time_utc(
                &self,
            ) -> Result<
                meerkat_schedule::ScheduleStoreActionTime,
                meerkat_schedule::ScheduleStoreError,
            > {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn read_due_refill_candidates(
                &self,
                _limit: usize,
            ) -> Result<meerkat_schedule::ScheduleRefillBatch, meerkat_schedule::ScheduleStoreError>
            {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_schedule_write(
                &self,
                _write: meerkat_schedule::AuthorizedScheduleWrite,
            ) -> Result<(), meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn get_schedule(
                &self,
                _schedule_id: &meerkat_schedule::ScheduleId,
            ) -> Result<Option<meerkat_schedule::Schedule>, meerkat_schedule::ScheduleStoreError>
            {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn list_schedules(
                &self,
                _filter: meerkat_schedule::ScheduleFilter,
            ) -> Result<Vec<meerkat_schedule::Schedule>, meerkat_schedule::ScheduleStoreError>
            {
                self.list_schedules_attempts
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_occurrence_write(
                &self,
                _write: meerkat_schedule::AuthorizedOccurrenceWrite,
            ) -> Result<(), meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_occurrence_writes(
                &self,
                _writes: Vec<meerkat_schedule::AuthorizedOccurrenceWrite>,
            ) -> Result<(), meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_schedule_mutation(
                &self,
                _schedule: meerkat_schedule::AuthorizedScheduleWrite,
                _occurrences: Vec<meerkat_schedule::AuthorizedOccurrenceWrite>,
            ) -> Result<meerkat_schedule::Schedule, meerkat_schedule::ScheduleStoreError>
            {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_schedule_refill(
                &self,
                _schedule: meerkat_schedule::AuthorizedScheduleWrite,
                _occurrences: Vec<meerkat_schedule::AuthorizedOccurrenceWrite>,
                _next_refill_at_utc: Option<chrono::DateTime<chrono::Utc>>,
            ) -> Result<meerkat_schedule::Schedule, meerkat_schedule::ScheduleStoreError>
            {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn record_refill_deadline_if_current(
                &self,
                _schedule_id: &meerkat_schedule::ScheduleId,
                _expected_revision: meerkat_schedule::ScheduleRevision,
                _expected_refill_at_utc: chrono::DateTime<chrono::Utc>,
                _next_refill_at_utc: Option<chrono::DateTime<chrono::Utc>>,
            ) -> Result<(), meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn get_occurrence(
                &self,
                _occurrence_id: &meerkat_schedule::OccurrenceId,
            ) -> Result<Option<Occurrence>, meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn list_occurrences(
                &self,
                _filter: meerkat_schedule::OccurrenceFilter,
            ) -> Result<Vec<Occurrence>, meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn append_receipt(
                &self,
                _receipt: meerkat_schedule::DeliveryReceipt,
            ) -> Result<(), meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn list_receipts(
                &self,
                _occurrence_id: &meerkat_schedule::OccurrenceId,
            ) -> Result<Vec<meerkat_schedule::DeliveryReceipt>, meerkat_schedule::ScheduleStoreError>
            {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn claim_due_occurrences(
                &self,
                _request: meerkat_schedule::ClaimDueRequest,
            ) -> Result<meerkat_schedule::ClaimDueResult, meerkat_schedule::ScheduleStoreError>
            {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn renew_occurrence_lease_if_current(
                &self,
                _request: meerkat_schedule::RenewOccurrenceLeaseRequest,
            ) -> Result<
                meerkat_schedule::RenewOccurrenceLeaseResult,
                meerkat_schedule::ScheduleStoreError,
            > {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn transition_occurrence_if_current(
                &self,
                _occurrence_id: &meerkat_schedule::OccurrenceId,
                _expected_attempt: u32,
                _expected_claim_token: Option<Uuid>,
                _transition: meerkat_schedule::OccurrenceLifecycleInput,
            ) -> Result<
                Option<(Occurrence, Vec<meerkat_schedule::OccurrenceLifecycleEffect>)>,
                meerkat_schedule::ScheduleStoreError,
            > {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn transition_occurrence_with_receipt_if_current(
                &self,
                _occurrence_id: &meerkat_schedule::OccurrenceId,
                _expected_attempt: u32,
                _expected_claim_token: Option<Uuid>,
                _transition: meerkat_schedule::OccurrenceLifecycleInput,
                _runtime_outcome: Option<meerkat_schedule::RuntimeDeliveryOutcome>,
            ) -> Result<Option<Occurrence>, meerkat_schedule::ScheduleStoreError> {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }
        }

        #[derive(Clone)]
        struct SharedBuf(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("log buffer lock")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer_buf = SharedBuf(Arc::clone(&buf));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_writer(move || writer_buf.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let local_list_schedules_attempts = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(FailingScheduleStore {
            wake_mode: ScheduleStoreWakeMode::ProcessLocal,
            durable_wake_attempts: Arc::new(AtomicUsize::new(0)),
            list_schedules_attempts: Arc::clone(&local_list_schedules_attempts),
        }) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(PanicOnMaterializeHost {
            materialize_calls: Arc::new(AtomicUsize::new(0)),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = Arc::new(SharedScheduleTargetAdapter::new(
            service.clone(),
            session_host,
            mob_host,
        ));

        let handle = spawn_schedule_host(service, adapter, "tick-health-test");
        // cfg!(test) poll interval is 50ms; give the loop a few ticks.
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.shutdown().await;
        assert_eq!(
            local_list_schedules_attempts.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "the long-lived worker must never run the catalog-wide compatibility migration"
        );

        // A custom push store whose wait primitive fails terminates only the
        // worker. The supervisor reports the stable incident class, backs
        // off, and starts another worker while its public handle stays live.
        let durable_wake_attempts = Arc::new(AtomicUsize::new(0));
        let push_list_schedules_attempts = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(FailingScheduleStore {
            wake_mode: ScheduleStoreWakeMode::Push,
            durable_wake_attempts: Arc::clone(&durable_wake_attempts),
            list_schedules_attempts: Arc::clone(&push_list_schedules_attempts),
        }) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(PanicOnMaterializeHost {
            materialize_calls: Arc::new(AtomicUsize::new(0)),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = Arc::new(SharedScheduleTargetAdapter::new(
            service.clone(),
            session_host,
            mob_host,
        ));
        let handle = spawn_schedule_host(service, adapter, "push-wake-supervision-test");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(handle.is_running(), "supervisor must survive worker exits");
        handle.shutdown().await;
        assert!(
            durable_wake_attempts.load(std::sync::atomic::Ordering::Relaxed) >= 2,
            "bounded supervision must retry a failed custom push wait"
        );
        assert_eq!(
            push_list_schedules_attempts.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "worker restarts must not repeat catalog-wide compatibility migration"
        );

        let logs = String::from_utf8(buf.lock().expect("log buffer lock").clone())
            .expect("captured logs should be utf8");
        assert!(
            logs.contains("schedule host entered an unhealthy state"),
            "the host loop must log failing tick outcomes, got: {logs}"
        );
        assert!(
            logs.contains("synthetic store outage"),
            "the incident log must carry the tick failure detail, got: {logs}"
        );
        assert!(
            logs.contains("durable_wake_failed"),
            "worker push-wait termination must be visible, got: {logs}"
        );
    }

    use async_trait::async_trait;
    use meerkat_schedule::ScheduleStore;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn sample_occurrence() -> Occurrence {
        let schedule = meerkat_schedule::Schedule::new(meerkat_schedule::CreateScheduleRequest {
            name: Some("schedule-host-test".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Interval(
                meerkat_schedule::IntervalTriggerSpec {
                    start_at_utc: chrono::Utc::now(),
                    every_seconds: 60,
                    end_at_utc: None,
                },
            ),
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text("hello".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
            overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("sample schedule creation should pass generated authority");
        let mut occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat_schedule::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("sample occurrence planning should pass generated authority");
        occurrence.attempt_count = 1;
        occurrence
    }

    #[tokio::test]
    async fn durable_terminal_dedup_replays_completed_instead_of_failing_authority() {
        let occurrence = sample_occurrence();
        let accepted = AcceptedScheduledInput::with_runtime_terminal(
            Some("existing-input".to_string()),
            CompletionOutcome::CompletedWithoutResult,
        )
        .with_admission_outcome(ScheduleAdmissionOutcome::Deduplicated);

        let dispatch = build_dispatch_from_accepted(&occurrence, accepted, None);
        assert_eq!(
            dispatch.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        let terminal = dispatch
            .completion
            .await
            .expect("durable terminal witness should be replayable");

        assert_eq!(terminal.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(
            terminal.runtime_completion_outcome,
            Some(meerkat_schedule::RuntimeCompletionOutcome::Completed)
        );
        assert_eq!(terminal.delivery_failure_reason, None);
    }

    #[tokio::test]
    async fn durable_terminal_replay_preserves_rich_failure_metadata() {
        let occurrence = sample_occurrence();
        let accepted = AcceptedScheduledInput::with_runtime_terminal(
            Some("existing-input".to_string()),
            CompletionOutcome::CompletedWithFinalizationFailure {
                error: meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                    "checkpoint commit failed",
                ),
            },
        )
        .with_admission_outcome(ScheduleAdmissionOutcome::Deduplicated);

        let terminal = build_dispatch_from_accepted(&occurrence, accepted, None)
            .completion
            .await
            .expect("exact durable completion receipt should be replayable");

        assert_eq!(
            terminal.runtime_completion_outcome,
            Some(meerkat_schedule::RuntimeCompletionOutcome::FinalizationFailed)
        );
        let detail = terminal.detail.expect("rich failure metadata");
        assert!(detail.contains("runtime_apply_failure"));
        assert!(detail.contains("checkpoint commit failed"));
    }

    #[test]
    fn v0810_predecessor_keys_survive_more_than_one_reclaim() {
        let mut occurrence = sample_occurrence();
        assert_eq!(
            schedule_v0810_predecessor_delivery_keys(&occurrence).collect::<Vec<_>>(),
            Vec::<String>::new(),
            "a first 0.8.11 attempt cannot have a 0.8.10 predecessor"
        );

        occurrence.attempt_count = 3;
        assert_eq!(
            schedule_v0810_predecessor_delivery_keys(&occurrence).collect::<Vec<_>>(),
            vec![
                format!(
                    "schedule:{}:occurrence:{}:attempt:2",
                    occurrence.schedule_id, occurrence.occurrence_id
                ),
                format!(
                    "schedule:{}:occurrence:{}:attempt:1",
                    occurrence.schedule_id, occurrence.occurrence_id
                ),
            ],
            "attempt 3 must still discover an attempt-1 binding after attempt 2 also crashed"
        );
    }

    #[test]
    fn mob_member_identity_rejects_pre_v0810_schema() {
        let v1 = r#"mob_member:{"schema":"meerkat.schedule.mob_member_identity.v1","mob_id":"ops","member":"watcher"}"#;
        let v2 = r#"mob_member:{"schema":"meerkat.schedule.mob_member_identity.v2","mob_id":"ops","member":"watcher"}"#;

        assert!(parse_mob_member_schedule_identity(v1).is_none());
        assert_eq!(
            parse_mob_member_schedule_identity(v2),
            Some(MobMemberScheduleIdentity {
                mob_id: "ops".to_string(),
                member: "watcher".to_string(),
            })
        );
    }

    #[test]
    fn store_action_delay_uses_store_clock_and_overdue_work_preserves_backoff() {
        let store_now_utc = chrono::Utc::now();
        let future_action = meerkat_schedule::ScheduleStoreActionTime {
            store_now_utc,
            next_action_at_utc: Some(store_now_utc + ChronoDuration::seconds(3)),
        };
        assert_eq!(
            duration_until_store_action(future_action),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            idle_delay_for_store_action(
                future_action,
                ScheduleStoreWakeMode::BoundedPoll {
                    max_interval: Duration::from_secs(5)
                },
                Duration::from_secs(10),
                Duration::from_millis(250),
            ),
            Some(Duration::from_secs(3))
        );

        let overdue_action = meerkat_schedule::ScheduleStoreActionTime {
            store_now_utc,
            next_action_at_utc: Some(store_now_utc - ChronoDuration::milliseconds(1)),
        };
        assert_eq!(
            duration_until_store_action(overdue_action),
            Some(Duration::ZERO)
        );
        assert_eq!(
            idle_delay_for_store_action(
                overdue_action,
                ScheduleStoreWakeMode::Push,
                Duration::from_secs(5),
                Duration::from_millis(250),
            ),
            Some(Duration::from_secs(5))
        );

        let no_action = meerkat_schedule::ScheduleStoreActionTime {
            store_now_utc,
            next_action_at_utc: None,
        };
        assert_eq!(
            idle_delay_for_store_action(
                no_action,
                ScheduleStoreWakeMode::ProcessLocal,
                Duration::from_secs(5),
                Duration::from_millis(250),
            ),
            None
        );
        assert_eq!(
            idle_delay_for_store_action(
                no_action,
                ScheduleStoreWakeMode::BoundedPoll {
                    max_interval: Duration::from_secs(5)
                },
                Duration::from_secs(10),
                Duration::from_millis(250),
            ),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            idle_delay_for_store_action(
                no_action,
                ScheduleStoreWakeMode::BoundedPoll {
                    max_interval: Duration::from_millis(100)
                },
                Duration::from_secs(10),
                Duration::from_millis(250),
            ),
            Some(Duration::from_millis(100)),
            "a non-zero store convergence bound must not be widened to the host base cadence"
        );
        assert_eq!(
            idle_delay_for_store_action(
                no_action,
                ScheduleStoreWakeMode::BoundedPoll {
                    max_interval: Duration::ZERO
                },
                Duration::from_secs(10),
                Duration::from_millis(250),
            ),
            Some(Duration::from_millis(250)),
            "zero cannot authorize a hot poll loop"
        );
    }

    #[tokio::test]
    async fn noop_mob_host_reports_clear_feature_required_failure() {
        let host = NoopScheduleMobHost::new(
            "scheduled mob targets require the mob feature on the CLI host",
        );
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: meerkat_schedule::ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };

        let probe = host
            .probe_mob_target(&binding)
            .await
            .expect("probe should succeed");
        let TargetProbeOutcome::Missing { detail } = probe else {
            panic!("expected no-op mob host to report missing, got {probe:?}");
        };
        assert_eq!(
            detail.as_deref(),
            Some("scheduled mob targets require the mob feature on the CLI host")
        );

        let occurrence = sample_occurrence();
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = host
            .deliver_mob_target(&occurrence, &identity, &binding)
            .await
            .expect("delivery dispatch");
        let terminal = dispatch.completion.await.expect("delivery terminal");

        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.detail.as_deref(),
            Some("scheduled mob targets require the mob feature on the CLI host")
        );
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::MobRejected)
        );
    }

    #[tokio::test]
    async fn accepted_schedule_dispatch_waits_for_runtime_completion_failure() {
        let terminal = delivery_terminal_from_completion_outcome(
            CompletionOutcome::CallbackPending {
                tool_use_id: "call-1".to_string(),
                tool_name: "external_approval".to_string(),
                args: serde_json::json!({"ticket": "INC-1"}),
            },
            None,
        );

        assert_eq!(terminal.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(
            terminal.runtime_completion_outcome,
            Some(meerkat_schedule::RuntimeCompletionOutcome::CallbackPending)
        );
        assert!(
            terminal
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("external_approval")
        );
        assert!(terminal.runtime_outcome.is_some());
    }

    #[tokio::test]
    async fn accepted_schedule_dispatch_without_runtime_authority_reports_typed_completion_failure()
    {
        let occurrence = sample_occurrence();
        let dispatch = build_dispatch_from_accepted(
            &occurrence,
            AcceptedScheduledInput::with_authority_unavailable(
                Some("corr-1".to_string()),
                "runtime completion authority unavailable for terminal input",
            ),
            None,
        );

        let error = dispatch.completion.await.expect_err("completion failure");
        match error {
            ScheduleDomainError::DeliveryCompletionFailed { reason, detail } => {
                assert_eq!(
                    reason,
                    DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
                );
                assert_eq!(
                    detail,
                    "runtime completion authority unavailable for terminal input"
                );
            }
            other => panic!("unexpected completion error: {other}"),
        }
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A session host that must never be asked to materialize. Any
    /// `materialize_session` call records a hit and returns an error so the
    /// Layer B reuse guard regression is caught as a test failure rather than
    /// a silent duplicate session.
    struct PanicOnMaterializeHost {
        materialize_calls: Arc<AtomicUsize>,
    }

    struct IdentityResolvingHost {
        current_session_id: Arc<Mutex<SessionId>>,
        delivered_session_id: Arc<Mutex<Option<SessionId>>>,
        legacy_session_id: Option<SessionId>,
    }

    struct RecordingMaterializeHost {
        materialized_config_prompt: Arc<Mutex<Option<String>>>,
        dispatched_turn_prompt: Arc<Mutex<Option<String>>>,
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for PanicOnMaterializeHost {
        async fn probe_session_target(
            &self,
            _binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }

        async fn materialize_session(
            &self,
            _occurrence: &Occurrence,
            _create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            self.materialize_calls.fetch_add(1, Ordering::SeqCst);
            Err(ScheduleDomainError::Internal(
                "Layer B reuse guard must reuse the bound session, never materialize".to_string(),
            ))
        }

        async fn deliver_prompt(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            _dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            _event_type: String,
            _payload: serde_json::Value,
            _render_metadata: Option<RenderMetadata>,
            _materialized_session_id: Option<SessionId>,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for IdentityResolvingHost {
        async fn probe_session_target(
            &self,
            _binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }

        async fn probe_identity_target(
            &self,
            binding: &IdentityTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            assert_eq!(binding.identity(), "domain:security");
            Ok(TargetProbeOutcome::Ready)
        }

        async fn resolve_identity_target(
            &self,
            binding: &IdentityTargetBinding,
        ) -> Result<Option<SessionId>, ScheduleDomainError> {
            assert_eq!(binding.identity(), "domain:security");
            Ok(Some(
                self.current_session_id
                    .lock()
                    .expect("current session lock")
                    .clone(),
            ))
        }

        async fn recover_session_target_identity(
            &self,
            binding: &SessionTargetBinding,
        ) -> Result<Option<IdentityTargetBinding>, ScheduleDomainError> {
            let Some(legacy_session_id) = &self.legacy_session_id else {
                return Ok(None);
            };
            if binding.resolved_session_id() != Some(legacy_session_id) {
                return Ok(None);
            }
            Ok(Some(IdentityTargetBinding::resumable(
                "domain:security",
                binding.action().clone(),
            )))
        }

        async fn materialize_session(
            &self,
            _occurrence: &Occurrence,
            _create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            panic!("identity targets must resolve existing materialized sessions")
        }

        async fn deliver_prompt(
            &self,
            session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            assert_eq!(dispatch.materialized_session_id, None);
            *self
                .delivered_session_id
                .lock()
                .expect("delivered session lock") = Some(session_id.clone());
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            _event_type: String,
            _payload: serde_json::Value,
            _render_metadata: Option<RenderMetadata>,
            _materialized_session_id: Option<SessionId>,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for RecordingMaterializeHost {
        async fn probe_session_target(
            &self,
            _binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }

        async fn materialize_session(
            &self,
            occurrence: &Occurrence,
            create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            *self
                .materialized_config_prompt
                .lock()
                .expect("materialized prompt lock") = create.system_prompt.clone();
            Ok(occurrence.materialized_session_id())
        }

        async fn deliver_prompt(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            *self
                .dispatched_turn_prompt
                .lock()
                .expect("dispatch prompt lock") = dispatch.system_prompt;
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            _event_type: String,
            _payload: serde_json::Value,
            _render_metadata: Option<RenderMetadata>,
            _materialized_session_id: Option<SessionId>,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }
    }

    fn materialize_on_demand_target() -> TargetBinding {
        TargetBinding::session(SessionTargetBinding::materialize_on_demand(
            SessionMaterializationSpec {
                model: "claude-sonnet-4-6".to_string(),
                system_prompt: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                provider_params: None,
                comms_name: None,
                peer_meta: None,
                labels: BTreeMap::new(),
                preload_skills: Vec::new(),
                additional_instructions: Vec::new(),
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                keep_alive: false,
                app_context: None,
            },
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::Text("scheduled prompt".to_string()),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ))
    }

    #[tokio::test]
    async fn resolve_session_reuses_authoritative_bound_id_without_materializing_on_stale_snapshot()
    {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("layer-b-reuse".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() - ChronoDuration::seconds(1),
                },
                target: materialize_on_demand_target(),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::AllowConcurrent,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("schedule create should plan one occurrence");

        let occurrence = service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("list occurrences")
            .into_iter()
            .next()
            .expect("schedule should have planned one occurrence");

        // A prior attempt materialized and committed the bound id to the
        // authoritative schedule target (and pending occurrences).
        let bound_id = SessionId::new();
        service
            .bind_materialized_session_for_occurrence(&occurrence, &bound_id)
            .await
            .expect("bind should commit the materialized session id");

        // The in-flight occurrence snapshot is STALE: it still reports
        // `bound_session_id: None`, exactly the window the residual described.
        let mut stale = occurrence.clone();
        stale.target_snapshot = materialize_on_demand_target();
        let TargetBinding::Session(binding) = &stale.target_snapshot else {
            panic!("expected a session target binding");
        };
        assert!(
            binding.resolved_session_id().is_none(),
            "stale snapshot must start unbound to exercise the reuse guard"
        );

        let materialize_calls = Arc::new(AtomicUsize::new(0));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(PanicOnMaterializeHost {
            materialize_calls: Arc::clone(&materialize_calls),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = SharedScheduleTargetAdapter::new(service, session_host, mob_host);

        let TargetBinding::Session(stale_binding) = &stale.target_snapshot else {
            panic!("expected a session target binding");
        };
        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&stale);
        let resolved = adapter
            .resolve_session(&stale, &delivery_identity, stale_binding)
            .await
            .expect("reuse guard should resolve without a delivery failure");

        assert_eq!(resolved.session_id, bound_id);
        assert_eq!(resolved.materialized_session_id, Some(bound_id));
        assert_eq!(
            materialize_calls.load(Ordering::SeqCst),
            0,
            "Layer B guard must reuse the bound id, never call materialize_session"
        );
    }

    #[tokio::test]
    async fn materialized_prompt_system_is_dispatched_at_turn_boundary_only() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let mut target = materialize_on_demand_target();
        let TargetBinding::Session(binding) = &mut target else {
            panic!("expected materialized session target");
        };
        let SessionTargetBinding::MaterializeOnDemandSession { create, action, .. } =
            binding.as_mut()
        else {
            panic!("expected materialized session target");
        };
        create.system_prompt = Some("materialization config".to_string());
        let ScheduledSessionAction::Prompt { system_prompt, .. } = action else {
            panic!("expected prompt action");
        };
        *system_prompt = Some("ordinary scheduled system".to_string());

        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("scheduled-system-boundary".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() - ChronoDuration::seconds(1),
                },
                target,
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::AllowConcurrent,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("schedule create");
        let occurrence = service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("list occurrences")
            .into_iter()
            .next()
            .expect("planned occurrence");
        let materialized_config_prompt = Arc::new(Mutex::new(None));
        let dispatched_turn_prompt = Arc::new(Mutex::new(None));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> =
            Arc::new(RecordingMaterializeHost {
                materialized_config_prompt: Arc::clone(&materialized_config_prompt),
                dispatched_turn_prompt: Arc::clone(&dispatched_turn_prompt),
            });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = SharedScheduleTargetAdapter::new(service, session_host, mob_host);
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);

        let dispatch = adapter
            .deliver_occurrence(&occurrence, &identity)
            .await
            .expect("delivery dispatch");
        dispatch.completion.await.expect("delivery completion");

        assert_eq!(
            materialized_config_prompt
                .lock()
                .expect("materialized prompt lock")
                .as_deref(),
            Some("materialization config")
        );
        assert_eq!(
            dispatched_turn_prompt
                .lock()
                .expect("dispatch prompt lock")
                .as_deref(),
            Some("ordinary scheduled system")
        );
    }

    #[tokio::test]
    async fn identity_target_resolves_current_session_at_delivery_time() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let current_session_id = Arc::new(Mutex::new(SessionId::new()));
        let delivered_session_id = Arc::new(Mutex::new(None));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(IdentityResolvingHost {
            current_session_id: Arc::clone(&current_session_id),
            delivered_session_id: Arc::clone(&delivered_session_id),
            legacy_session_id: None,
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = SharedScheduleTargetAdapter::new(service, session_host, mob_host);

        let mut occurrence = sample_occurrence();
        occurrence.target_snapshot = TargetBinding::identity(IdentityTargetBinding::resumable(
            "domain:security",
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::Text("identity check".to_string()),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ));

        let session_after_restart = SessionId::new();
        *current_session_id.lock().expect("current session lock") = session_after_restart.clone();

        let probe = adapter
            .probe_target(&occurrence)
            .await
            .expect("identity probe should resolve through host");
        assert!(matches!(probe, TargetProbeOutcome::Ready));

        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = adapter
            .deliver_occurrence(&occurrence, &delivery_identity)
            .await
            .expect("identity delivery should dispatch");
        let terminal = dispatch.completion.await.expect("delivery completion");
        assert_eq!(terminal.phase, OccurrencePhase::Completed);
        assert_eq!(
            delivered_session_id
                .lock()
                .expect("delivered session lock")
                .as_ref(),
            Some(&session_after_restart)
        );
    }

    #[tokio::test]
    async fn migrate_recoverable_session_target_persists_identity_target() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let legacy_session_id = SessionId::new();
        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("legacy-owned-session".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Interval(
                    meerkat_schedule::IntervalTriggerSpec {
                        start_at_utc: chrono::Utc::now(),
                        every_seconds: 60,
                        end_at_utc: None,
                    },
                ),
                target: TargetBinding::session(SessionTargetBinding::ResumableSession {
                    session_id: legacy_session_id.clone(),
                    action: ScheduledSessionAction::Prompt {
                        prompt: ContentInput::Text("legacy identity check".to_string()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                }),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("schedule create should succeed");
        let current_session_id = Arc::new(Mutex::new(SessionId::new()));
        let delivered_session_id = Arc::new(Mutex::new(None));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(IdentityResolvingHost {
            current_session_id,
            delivered_session_id,
            legacy_session_id: Some(legacy_session_id),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = SharedScheduleTargetAdapter::new(service.clone(), session_host, mob_host);

        let migrated = adapter
            .migrate_recoverable_session_targets()
            .await
            .expect("migration should succeed");
        assert_eq!(migrated, 1);

        let updated = store
            .get_schedule(&schedule.schedule_id)
            .await
            .expect("store read")
            .expect("schedule still exists");
        let TargetBinding::Identity(binding) = updated.target else {
            panic!("legacy session target should migrate to identity target");
        };
        assert_eq!(binding.identity(), "domain:security");
    }

    // -----------------------------------------------------------------------
    // HostRunnable targets
    // -----------------------------------------------------------------------

    use meerkat_schedule::{
        HostRunnable, HostRunnableError, HostRunnableName, HostRunnableOutcome,
        HostRunnableRegistry, OccurrenceFailureClass,
    };

    struct RecordingHostRunnable {
        invocations: Arc<Mutex<Vec<HostRunnableInvocation>>>,
        failure_detail: Option<String>,
    }

    #[async_trait]
    impl HostRunnable for RecordingHostRunnable {
        async fn run(
            &self,
            invocation: HostRunnableInvocation,
        ) -> Result<HostRunnableOutcome, HostRunnableError> {
            self.invocations
                .lock()
                .expect("invocation lock")
                .push(invocation);
            match &self.failure_detail {
                Some(detail) => Err(HostRunnableError::Failed {
                    detail: detail.clone(),
                }),
                None => Ok(HostRunnableOutcome::completed()),
            }
        }
    }

    fn runnable_name(value: &str) -> HostRunnableName {
        HostRunnableName::parse(value).expect("valid runnable name")
    }

    fn host_runnable_target(name: &str, params: Option<&str>) -> TargetBinding {
        TargetBinding::host_runnable(HostRunnableTargetBinding {
            runnable: runnable_name(name),
            params: params.map(|raw| HostRunnableParams::parse(raw).expect("valid raw params")),
        })
    }

    fn registry_with(name: &str, runnable: Arc<dyn HostRunnable>) -> Arc<dyn ScheduleRunnableHost> {
        let mut registry = HostRunnableRegistry::new();
        registry
            .register(runnable_name(name), runnable)
            .expect("runnable registration");
        Arc::new(registry)
    }

    fn host_runnable_adapter(
        service: ScheduleService,
        runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    ) -> SharedScheduleTargetAdapter {
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(PanicOnMaterializeHost {
            materialize_calls: Arc::new(AtomicUsize::new(0)),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = SharedScheduleTargetAdapter::new(service, session_host, mob_host);
        match runnable_host {
            Some(runnable_host) => adapter.with_runnable_host(runnable_host),
            None => adapter,
        }
    }

    fn recording_runnable(
        failure_detail: Option<&str>,
    ) -> (
        Arc<RecordingHostRunnable>,
        Arc<Mutex<Vec<HostRunnableInvocation>>>,
    ) {
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let runnable = Arc::new(RecordingHostRunnable {
            invocations: Arc::clone(&invocations),
            failure_detail: failure_detail.map(str::to_string),
        });
        (runnable, invocations)
    }

    #[tokio::test]
    async fn host_runnable_probe_matrix_reports_ready_only_when_registered() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let mut occurrence = sample_occurrence();
        occurrence.target_snapshot = host_runnable_target("nightly-report", None);

        // No runnable host configured on the surface.
        let adapter = host_runnable_adapter(service.clone(), None);
        let probe = adapter.probe_target(&occurrence).await.expect("probe");
        let TargetProbeOutcome::Missing { detail } = probe else {
            panic!("expected missing probe without a runnable host, got {probe:?}");
        };
        assert!(
            detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no runnable registry")),
            "missing detail should explain the absent registry: {detail:?}"
        );

        // A registry is configured but the named runnable is not registered.
        let (runnable, _invocations) = recording_runnable(None);
        let adapter = host_runnable_adapter(
            service.clone(),
            Some(registry_with("other-runnable", runnable)),
        );
        let probe = adapter.probe_target(&occurrence).await.expect("probe");
        let TargetProbeOutcome::Missing { detail } = probe else {
            panic!("expected missing probe for unregistered runnable, got {probe:?}");
        };
        assert!(
            detail
                .as_deref()
                .is_some_and(|detail| detail.contains("not registered")),
            "missing detail should name the unregistered runnable: {detail:?}"
        );

        // The named runnable is registered.
        let (runnable, _invocations) = recording_runnable(None);
        let adapter =
            host_runnable_adapter(service, Some(registry_with("nightly-report", runnable)));
        let probe = adapter.probe_target(&occurrence).await.expect("probe");
        assert!(matches!(probe, TargetProbeOutcome::Ready));
    }

    #[tokio::test]
    async fn host_runnable_delivery_without_registry_fails_target_missing() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let adapter = host_runnable_adapter(ScheduleService::new(store), None);
        let mut occurrence = sample_occurrence();
        occurrence.target_snapshot = host_runnable_target("nightly-report", None);

        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = adapter
            .deliver_occurrence(&occurrence, &delivery_identity)
            .await
            .expect("delivery dispatch");
        let terminal = dispatch.completion.await.expect("delivery terminal");

        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::TargetMissing)
        );
        assert!(
            terminal
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no runnable registry")),
            "failure detail should explain the absent registry: {:?}",
            terminal.detail
        );
    }

    #[tokio::test]
    async fn host_runnable_delivery_unregistered_fails_target_missing() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let (runnable, invocations) = recording_runnable(None);
        let adapter = host_runnable_adapter(
            ScheduleService::new(store),
            Some(registry_with("other-runnable", runnable)),
        );
        let mut occurrence = sample_occurrence();
        occurrence.target_snapshot = host_runnable_target("nightly-report", None);

        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = adapter
            .deliver_occurrence(&occurrence, &delivery_identity)
            .await
            .expect("delivery dispatch");
        let terminal = dispatch.completion.await.expect("delivery terminal");

        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::TargetMissing)
        );
        assert!(
            invocations.lock().expect("invocation lock").is_empty(),
            "an unregistered runnable must never be invoked"
        );
    }

    #[tokio::test]
    async fn host_runnable_delivery_success_completes_with_typed_invocation() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let (runnable, invocations) = recording_runnable(None);
        let adapter = host_runnable_adapter(
            ScheduleService::new(store),
            Some(registry_with("nightly-report", runnable)),
        );
        let mut occurrence = sample_occurrence();
        occurrence.target_snapshot = host_runnable_target("nightly-report", Some(r#"{"depth":3}"#));

        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = adapter
            .deliver_occurrence(&occurrence, &delivery_identity)
            .await
            .expect("delivery dispatch");
        assert_eq!(
            dispatch.receipt.stage,
            DeliveryReceiptStage::DispatchAccepted
        );
        assert_eq!(
            dispatch.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionAccepted)
        );
        assert_eq!(
            dispatch.correlation_id.as_deref(),
            Some(delivery_identity.correlation_id.as_str())
        );
        let terminal = dispatch.completion.await.expect("delivery terminal");

        assert_eq!(terminal.phase, OccurrencePhase::Completed);
        assert_eq!(terminal.delivery_failure_reason, None);

        let recorded = invocations.lock().expect("invocation lock");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].occurrence_id, occurrence.occurrence_id);
        assert_eq!(recorded[0].schedule_id, occurrence.schedule_id);
        assert_eq!(
            recorded[0].delivery_idempotency_key,
            delivery_identity.idempotency_key
        );
        assert_eq!(recorded[0].runnable.as_str(), "nightly-report");
        assert_eq!(recorded[0].trigger_time, occurrence.due_at_utc);
        assert_eq!(
            recorded[0]
                .params
                .as_deref()
                .map(serde_json::value::RawValue::get),
            Some(r#"{"depth":3}"#)
        );
    }

    #[tokio::test]
    async fn host_runnable_delivery_callback_error_maps_to_runtime_rejected() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let (runnable, _invocations) = recording_runnable(Some("downstream export failed"));
        let adapter = host_runnable_adapter(
            ScheduleService::new(store),
            Some(registry_with("nightly-report", runnable)),
        );
        let mut occurrence = sample_occurrence();
        occurrence.target_snapshot = host_runnable_target("nightly-report", None);

        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = adapter
            .deliver_occurrence(&occurrence, &delivery_identity)
            .await
            .expect("delivery dispatch");
        let terminal = dispatch.completion.await.expect("delivery terminal");

        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.delivery_failure_reason,
            Some(DeliveryFailureReason::RuntimeRejected)
        );
        assert!(
            terminal
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("downstream export failed")),
            "failure detail should carry the callback error: {:?}",
            terminal.detail
        );
    }

    async fn wait_for_occurrence_phase(
        service: &ScheduleService,
        schedule_id: &meerkat_schedule::ScheduleId,
        expected_phase: OccurrencePhase,
    ) -> Occurrence {
        for _ in 0..50 {
            let occurrences = service
                .list_occurrences(schedule_id)
                .await
                .expect("list occurrences");
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.phase == expected_phase)
            {
                return occurrence;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for occurrence phase {expected_phase:?}");
    }

    async fn create_host_runnable_schedule(
        service: &ScheduleService,
        name: &str,
        params: Option<&str>,
    ) -> meerkat_schedule::Schedule {
        service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some(format!("host-runnable-{name}")),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() - ChronoDuration::seconds(1),
                },
                target: host_runnable_target(name, params),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::AllowConcurrent,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("host runnable schedule create should pass public api validation")
    }

    fn host_runnable_driver(
        service: ScheduleService,
        store: Arc<dyn ScheduleStore>,
        adapter: Arc<SharedScheduleTargetAdapter>,
    ) -> ScheduleDriver {
        ScheduleDriver::new(
            service,
            store,
            adapter.clone(),
            adapter,
            "host-runnable-driver",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: ChronoDuration::seconds(30),
            },
        )
    }

    #[tokio::test]
    async fn host_runnable_schedule_completes_through_real_driver_tick() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule =
            create_host_runnable_schedule(&service, "nightly-report", Some(r#"{"depth":3}"#)).await;

        let (runnable, invocations) = recording_runnable(None);
        let adapter = Arc::new(host_runnable_adapter(
            service.clone(),
            Some(registry_with("nightly-report", runnable)),
        ));
        let driver = host_runnable_driver(service.clone(), store.clone(), adapter);

        let report = driver.tick_once().await.expect("driver tick");
        assert_eq!(report.claimed_occurrences, 1);

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await;
        assert_eq!(occurrence.failure_class, None);

        // Occurrence lifecycle parity with session/mob targets: the driver
        // records the dispatch receipt and the terminal completion receipt
        // through the occurrence authority.
        let receipts = store
            .list_receipts(&occurrence.occurrence_id)
            .await
            .expect("receipts");
        assert!(
            receipts
                .iter()
                .any(|receipt| receipt.stage == DeliveryReceiptStage::DispatchStarted),
            "dispatch receipt should be recorded"
        );
        assert_eq!(
            receipts.last().map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::Completed),
            "terminal receipt should record completion"
        );

        let recorded = invocations.lock().expect("invocation lock");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].occurrence_id, occurrence.occurrence_id);
        assert_eq!(recorded[0].schedule_id, schedule.schedule_id);
    }

    #[tokio::test]
    async fn host_runnable_schedule_failure_records_runtime_rejected_through_driver() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = create_host_runnable_schedule(&service, "nightly-report", None).await;

        let (runnable, _invocations) = recording_runnable(Some("downstream export failed"));
        let adapter = Arc::new(host_runnable_adapter(
            service.clone(),
            Some(registry_with("nightly-report", runnable)),
        ));
        let driver = host_runnable_driver(service.clone(), store.clone(), adapter);

        driver.tick_once().await.expect("driver tick");

        let occurrence = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::DeliveryFailed,
        )
        .await;
        assert_eq!(
            occurrence.failure_class,
            Some(OccurrenceFailureClass::RuntimeRejected)
        );

        let receipts = store
            .list_receipts(&occurrence.occurrence_id)
            .await
            .expect("receipts");
        let last_receipt = receipts.last().expect("terminal receipt");
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DeliveryFailed);
        assert_eq!(
            last_receipt.failure_class,
            Some(OccurrenceFailureClass::RuntimeRejected)
        );
        assert!(
            last_receipt
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("downstream export failed")),
            "terminal receipt should carry the callback failure detail: {:?}",
            last_receipt.detail
        );
    }

    #[tokio::test]
    async fn host_runnable_schedule_without_registry_misfires_through_driver() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = create_host_runnable_schedule(&service, "nightly-report", None).await;

        let adapter = Arc::new(host_runnable_adapter(service.clone(), None));
        let driver = host_runnable_driver(service.clone(), store.clone(), adapter);

        driver.tick_once().await.expect("driver tick");

        // MissingTargetPolicy::MarkMisfired: the probe reports Missing, the
        // occurrence authority classifies the misfire — same machine path as
        // missing session/mob targets.
        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Misfired)
                .await;
        assert_eq!(
            occurrence.failure_class,
            Some(OccurrenceFailureClass::TargetMissing)
        );
    }
}
