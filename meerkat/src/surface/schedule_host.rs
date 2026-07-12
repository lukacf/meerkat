use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat_core::{ContentInput, Session, SessionId, skills::SkillRef, types::RenderMetadata};
use meerkat_runtime::{
    CompletionHandle,
    completion::{CompletionOutcome, CompletionWaitError},
};
use meerkat_schedule::{
    DeliveryCompletion, DeliveryCompletionFailureReason, DeliveryDispatch, DeliveryFailureReason,
    DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal, HostRunnableInvocation,
    HostRunnableParams, HostRunnableTargetBinding, IdentityTargetBinding, MobTargetBinding,
    Occurrence, OccurrencePhase, RunnableProbe, ScheduleDomainError, ScheduleDriver,
    ScheduleDriverConfig, ScheduleFilter, ScheduleRunnableHost, ScheduleService, ScheduleStoreKind,
    ScheduleTargetDelivery, ScheduleTargetProbe, ScheduledSessionAction,
    SessionMaterializationSpec, SessionTargetBinding, TargetBinding, TargetProbeOutcome,
    UpdateScheduleRequest,
};
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::oneshot;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::oneshot;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::task::JoinHandle;

pub struct ScheduleHostHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl ScheduleHostHandle {
    pub async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        let _ = self.join.await;
    }
}

#[derive(Debug, Clone)]
struct ResolvedScheduledSession {
    session_id: SessionId,
    materialized_session_id: Option<SessionId>,
    allow_system_prompt_override: bool,
}

pub enum AcceptedScheduledInputCompletion {
    RuntimeHandle(CompletionHandle),
    RuntimeCompletionAuthorityUnavailable { detail: String },
}

pub struct AcceptedScheduledInput {
    pub correlation_id: Option<String>,
    pub completion: AcceptedScheduledInputCompletion,
}

impl AcceptedScheduledInput {
    pub fn with_runtime_handle(correlation_id: Option<String>, handle: CompletionHandle) -> Self {
        Self {
            correlation_id,
            completion: AcceptedScheduledInputCompletion::RuntimeHandle(handle),
        }
    }

    pub fn with_authority_unavailable(
        correlation_id: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            correlation_id,
            completion: AcceptedScheduledInputCompletion::RuntimeCompletionAuthorityUnavailable {
                detail: detail.into(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledPromptDispatch {
    pub prompt: ContentInput,
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
        "meerkat.schedule.mob_member_identity.v1" | "meerkat.schedule.mob_member_identity.v2" => {
            Some(MobMemberScheduleIdentity {
                mob_id: key.mob_id,
                member: key.member,
            })
        }
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
    async fn materialize_session(
        &self,
        occurrence: &Occurrence,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError>;

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
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
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;

    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
        let _ = binding;
        Ok(None)
    }

    async fn deliver_identity_target(
        &self,
        occurrence: &Occurrence,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        let _ = (occurrence, binding);
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
        _binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        Ok(immediate_delivery_failure(
            occurrence,
            self.detail.clone(),
            DeliveryFailureReason::MobRejected,
            None,
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
                                None,
                                None,
                            )
                        })?;
                    if let Some(identity) = recovered {
                        if let Some(dispatch) = self
                            .mob_host
                            .deliver_identity_target(occurrence, &identity)
                            .await
                            .map_err(|error| {
                                immediate_delivery_failure(
                                    occurrence,
                                    error.to_string(),
                                    DeliveryFailureReason::TargetMaterializationFailed,
                                    None,
                                    None,
                                )
                            })?
                        {
                            return Err(dispatch);
                        }
                        return self.resolve_identity(occurrence, &identity).await;
                    }
                }
                Ok(ResolvedScheduledSession {
                    session_id: session_id.clone(),
                    materialized_session_id: None,
                    allow_system_prompt_override: false,
                })
            }
            SessionTargetBinding::MaterializeOnDemandSession {
                bound_session_id: Some(session_id),
                ..
            } => Ok(ResolvedScheduledSession {
                session_id: session_id.clone(),
                materialized_session_id: Some(session_id.clone()),
                allow_system_prompt_override: false,
            }),
            SessionTargetBinding::MaterializeOnDemandSession {
                create,
                action,
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
                        allow_system_prompt_override: false,
                    });
                }
                let prompt_system_prompt = match action {
                    ScheduledSessionAction::Prompt { system_prompt, .. } => {
                        system_prompt.as_deref()
                    }
                    ScheduledSessionAction::Event { .. } => None,
                };
                match self
                    .session_host
                    .materialize_session(occurrence, create, prompt_system_prompt)
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
                                None,
                                Some(session_id),
                            ));
                        }
                        Ok(ResolvedScheduledSession {
                            session_id: session_id.clone(),
                            materialized_session_id: Some(session_id),
                            allow_system_prompt_override: true,
                        })
                    }
                    Err(error) => Err(immediate_delivery_failure(
                        occurrence,
                        error.to_string(),
                        DeliveryFailureReason::TargetMaterializationFailed,
                        None,
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
        binding: &IdentityTargetBinding,
    ) -> Result<ResolvedScheduledSession, DeliveryDispatch> {
        match self.session_host.resolve_identity_target(binding).await {
            Ok(Some(session_id)) => Ok(ResolvedScheduledSession {
                session_id,
                materialized_session_id: None,
                allow_system_prompt_override: false,
            }),
            Ok(None) => Err(immediate_delivery_failure(
                occurrence,
                format!(
                    "scheduled identity target not found: {}",
                    binding.identity()
                ),
                DeliveryFailureReason::TargetMaterializationFailed,
                None,
                None,
            )),
            Err(error) => Err(immediate_delivery_failure(
                occurrence,
                error.to_string(),
                DeliveryFailureReason::TargetMaterializationFailed,
                None,
                None,
            )),
        }
    }

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
                if system_prompt.is_some() && !resolved.allow_system_prompt_override {
                    return Ok(immediate_delivery_failure(
                        occurrence,
                        "scheduled system_prompt override is only supported when materializing a new session"
                            .to_string(),
                        DeliveryFailureReason::RuntimeRejected,
                        None,
                        resolved.materialized_session_id,
                    ));
                }
                self.session_host
                    .deliver_prompt(
                        &resolved.session_id,
                        occurrence,
                        ScheduledPromptDispatch {
                            prompt: prompt.clone(),
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
                None,
                None,
            );
        };
        if runnable_host.probe_runnable(&binding.runnable) == RunnableProbe::Unknown {
            return immediate_delivery_failure(
                occurrence,
                format!("host runnable '{}' is not registered", binding.runnable),
                DeliveryFailureReason::TargetMissing,
                None,
                None,
            );
        }

        let invocation = HostRunnableInvocation {
            occurrence_id: occurrence.occurrence_id.clone(),
            schedule_id: occurrence.schedule_id.clone(),
            runnable: binding.runnable.clone(),
            trigger_time: occurrence.due_at_utc,
            params: binding.params.clone().map(HostRunnableParams::into_raw),
        };
        let runnable_host = Arc::clone(runnable_host);
        async_completion_dispatch(
            occurrence,
            None,
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
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                let resolved = match self.resolve_session(occurrence, binding).await {
                    Ok(resolved) => resolved,
                    Err(dispatch) => return Ok(dispatch),
                };

                self.deliver_session_action(occurrence, resolved, binding.action())
                    .await
            }
            TargetBinding::Identity(binding) => {
                if let Some(dispatch) = self
                    .mob_host
                    .deliver_identity_target(occurrence, binding)
                    .await?
                {
                    return Ok(dispatch);
                }
                let resolved = match self.resolve_identity(occurrence, binding).await {
                    Ok(resolved) => resolved,
                    Err(dispatch) => return Ok(dispatch),
                };
                self.deliver_session_action(occurrence, resolved, binding.action())
                    .await
            }
            TargetBinding::Mob(binding) => {
                self.mob_host.deliver_mob_target(occurrence, binding).await
            }
            TargetBinding::HostRunnable(binding) => {
                Ok(self.deliver_host_runnable(occurrence, binding))
            }
        }
    }
}

pub fn schedule_host_supported(kind: ScheduleStoreKind) -> bool {
    !matches!(kind, ScheduleStoreKind::Disabled | ScheduleStoreKind::Jsonl)
}

/// Tick-health bookkeeping for the schedule host loop: a driver that cannot
/// claim is an incident, not a no-op, so tick errors and per-row faults are
/// logged on first occurrence and on change at ERROR, with a rate-limited
/// heartbeat (with the consecutive count) while the same condition persists,
/// and an INFO recovery line when the loop is healthy again.
struct TickHealthTracker {
    fingerprint: Option<String>,
    /// Ticks the CURRENT fingerprint has persisted (heartbeat counter).
    consecutive: u64,
    /// Total unhealthy ticks since the loop was last healthy, across
    /// fingerprint changes — the outage length reported on recovery.
    outage_ticks: u64,
    last_logged: Option<meerkat_core::time_compat::Instant>,
    heartbeat_every: std::time::Duration,
}

#[derive(Debug)]
enum TickHealthLog {
    Quiet,
    /// A new or changed failure condition: log at ERROR.
    Incident(String),
    /// The same condition persists: rate-limited WARN heartbeat.
    Heartbeat {
        fingerprint: String,
        consecutive: u64,
    },
    /// The loop recovered after `after` unhealthy ticks (counted across
    /// fingerprint changes within the same outage).
    Recovered {
        after: u64,
    },
}

impl TickHealthTracker {
    fn new(heartbeat_every: std::time::Duration) -> Self {
        Self {
            fingerprint: None,
            consecutive: 0,
            outage_ticks: 0,
            last_logged: None,
            heartbeat_every,
        }
    }

    fn observe(
        &mut self,
        outcome: &Result<
            meerkat_schedule::ScheduleTickReport,
            meerkat_schedule::ScheduleDomainError,
        >,
        now: meerkat_core::time_compat::Instant,
    ) -> TickHealthLog {
        let fingerprint = match outcome {
            Err(error) => Some(format!("tick failed: {error}")),
            Ok(report) if report.fault_count() > 0 => Some(format!(
                "tick degraded ({} row fault(s)):\n{}",
                report.fault_count(),
                report.fault_fingerprint()
            )),
            Ok(_) => None,
        };
        match fingerprint {
            None => {
                let after = self.outage_ticks;
                self.fingerprint = None;
                self.consecutive = 0;
                self.outage_ticks = 0;
                self.last_logged = None;
                if after > 0 {
                    TickHealthLog::Recovered { after }
                } else {
                    TickHealthLog::Quiet
                }
            }
            Some(fingerprint) => {
                self.outage_ticks += 1;
                self.consecutive += 1;
                if self.fingerprint.as_deref() != Some(fingerprint.as_str()) {
                    self.fingerprint = Some(fingerprint.clone());
                    self.consecutive = 1;
                    self.last_logged = Some(now);
                    return TickHealthLog::Incident(fingerprint);
                }
                if self
                    .last_logged
                    .is_none_or(|last| now.duration_since(last) >= self.heartbeat_every)
                {
                    self.last_logged = Some(now);
                    return TickHealthLog::Heartbeat {
                        fingerprint,
                        consecutive: self.consecutive,
                    };
                }
                TickHealthLog::Quiet
            }
        }
    }
}

fn log_tick_health(action: TickHealthLog) {
    match action {
        TickHealthLog::Quiet => {}
        TickHealthLog::Incident(fingerprint) => {
            tracing::error!(%fingerprint, "schedule driver tick is failing or degraded");
        }
        TickHealthLog::Heartbeat {
            fingerprint,
            consecutive,
        } => {
            tracing::warn!(
                %fingerprint,
                consecutive,
                "schedule driver tick still failing or degraded"
            );
        }
        TickHealthLog::Recovered { after } => {
            tracing::info!(after, "schedule driver tick recovered");
        }
    }
}

pub fn spawn_schedule_host(
    schedule_service: ScheduleService,
    adapter: Arc<SharedScheduleTargetAdapter>,
    owner_id: impl Into<String>,
) -> ScheduleHostHandle {
    let migration_adapter = Arc::clone(&adapter);
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
    let poll_interval = if cfg!(test) {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(250)
    };
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    #[cfg(not(target_arch = "wasm32"))]
    let join = tokio::spawn(async move {
        if let Err(error) = migration_adapter
            .migrate_recoverable_session_targets()
            .await
        {
            tracing::warn!(%error, "failed to migrate recoverable schedule session targets");
        }
        let mut interval = tokio::time::interval(poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut health = TickHealthTracker::new(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {
                    let outcome = driver.tick_once().await;
                    log_tick_health(health.observe(&outcome, meerkat_core::time_compat::Instant::now()));
                }
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    let join = tokio_with_wasm::alias::task::spawn(async move {
        if let Err(error) = migration_adapter
            .migrate_recoverable_session_targets()
            .await
        {
            tracing::warn!(%error, "failed to migrate recoverable schedule session targets");
        }
        let mut interval = tokio_with_wasm::alias::time::interval(poll_interval);
        let mut health = TickHealthTracker::new(std::time::Duration::from_secs(60));
        loop {
            tokio_with_wasm::alias::select! {
                _ = &mut shutdown_rx => break,
                () = interval.tick() => {
                    let outcome = driver.tick_once().await;
                    log_tick_health(health.observe(&outcome, meerkat_core::time_compat::Instant::now()));
                }
            }
        }
    });

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
            AcceptedScheduledInputCompletion::RuntimeHandle(handle) => {
                match handle.try_wait().await {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return Err(ScheduleDomainError::DeliveryCompletionFailed {
                            reason: completion_wait_failure_reason(&error),
                            detail: format!("runtime completion authority unavailable: {error}"),
                        });
                    }
                }
            }
            AcceptedScheduledInputCompletion::RuntimeCompletionAuthorityUnavailable { detail } => {
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
        CompletionOutcome::CallbackPending { tool_name, args } => {
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
        CompletionOutcome::Cancelled => {
            let runtime_outcome = meerkat_schedule::RuntimeDeliveryOutcome::CompletionAbandoned {
                detail: "request cancelled".to_string(),
            };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::Cancelled,
                runtime_outcome,
            )
        }
        CompletionOutcome::Abandoned { reason, .. } => {
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionAbandoned { detail: reason };
            terminal_from_runtime_completion_outcome(
                meerkat_schedule::RuntimeCompletionOutcome::Abandoned,
                runtime_outcome,
            )
        }
        CompletionOutcome::AbandonedWithError { reason, error } => {
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
            DeliveryTerminal::runtime_completion(
                meerkat_schedule::RuntimeCompletionOutcome::FinalizationFailed,
                Some(
                    error
                        .detail
                        .unwrap_or_else(|| "turn finalization failed".to_string()),
                ),
                None,
            )
        }
        CompletionOutcome::RuntimeTerminated { reason, .. } => {
            let runtime_outcome =
                meerkat_schedule::RuntimeDeliveryOutcome::CompletionRuntimeTerminated {
                    detail: reason,
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
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
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
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
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

pub fn schedule_attempt_idempotency_key(occurrence: &Occurrence) -> String {
    format!(
        "schedule:{}:occurrence:{}:attempt:{}",
        occurrence.schedule_id, occurrence.occurrence_id, occurrence.attempt_count
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Ask 16: a driver that cannot claim is an incident, not a no-op. The
    /// tracker logs a new/changed condition once at ERROR, heartbeats with
    /// the consecutive count while it persists, and reports recovery.
    #[test]
    fn tick_health_tracker_logs_incident_heartbeat_and_recovery() {
        let heartbeat = std::time::Duration::from_secs(60);
        let mut tracker = TickHealthTracker::new(heartbeat);
        let start = meerkat_core::time_compat::Instant::now();
        let failure: Result<meerkat_schedule::ScheduleTickReport, _> = Err(
            meerkat_schedule::ScheduleDomainError::Internal("store unavailable".to_string()),
        );
        let ok: Result<_, meerkat_schedule::ScheduleDomainError> =
            Ok(meerkat_schedule::ScheduleTickReport::default());

        // Healthy ticks stay quiet.
        assert!(matches!(tracker.observe(&ok, start), TickHealthLog::Quiet));

        // First failure: incident at ERROR.
        assert!(matches!(
            tracker.observe(&failure, start),
            TickHealthLog::Incident(_)
        ));
        // Same condition immediately after: quiet (rate limited).
        assert!(matches!(
            tracker.observe(&failure, start + std::time::Duration::from_millis(250)),
            TickHealthLog::Quiet
        ));
        // Same condition past the heartbeat window: WARN heartbeat with the
        // consecutive count.
        match tracker.observe(
            &failure,
            start + heartbeat + std::time::Duration::from_secs(1),
        ) {
            TickHealthLog::Heartbeat { consecutive, .. } => assert_eq!(consecutive, 3),
            other => panic!("expected heartbeat, got {other:?}"),
        }

        // A CHANGED condition logs immediately even inside the window.
        let changed: Result<meerkat_schedule::ScheduleTickReport, _> = Err(
            meerkat_schedule::ScheduleDomainError::Internal("different failure".to_string()),
        );
        assert!(matches!(
            tracker.observe(
                &changed,
                start + heartbeat + std::time::Duration::from_secs(2)
            ),
            TickHealthLog::Incident(_)
        ));

        // Recovery reports the FULL outage length (4 unhealthy ticks),
        // not just the ticks since the condition last changed.
        match tracker.observe(&ok, start + heartbeat + std::time::Duration::from_secs(3)) {
            TickHealthLog::Recovered { after } => assert_eq!(after, 4),
            other => panic!("expected recovery, got {other:?}"),
        }
        assert!(matches!(
            tracker.observe(&ok, start + heartbeat + std::time::Duration::from_secs(4)),
            TickHealthLog::Quiet
        ));
    }

    /// A tick that SUCCEEDS but skipped rows as typed faults is degraded,
    /// not healthy: the tracker treats the fault fingerprint like an error
    /// condition (log on change, heartbeat while it persists).
    #[test]
    fn tick_health_tracker_treats_row_faults_as_degraded() {
        let heartbeat = std::time::Duration::from_secs(60);
        let mut tracker = TickHealthTracker::new(heartbeat);
        let start = meerkat_core::time_compat::Instant::now();
        let mut report = meerkat_schedule::ScheduleTickReport::default();
        report
            .occurrence_row_faults
            .push(meerkat_schedule::ScheduleStoreRowFault {
                schedule_id: Some("sched-1".to_string()),
                occurrence_id: Some("occ-1".to_string()),
                kind: meerkat_schedule::ScheduleStoreRowFaultKind::Deserialization,
                detail: "poisoned row".to_string(),
            });
        let degraded: Result<_, meerkat_schedule::ScheduleDomainError> = Ok(report);

        match tracker.observe(&degraded, start) {
            TickHealthLog::Incident(fingerprint) => {
                assert!(fingerprint.contains("occ-1"), "{fingerprint}");
                assert!(fingerprint.contains("poisoned row"), "{fingerprint}");
            }
            other => panic!("expected incident, got {other:?}"),
        }
        assert!(matches!(
            tracker.observe(&degraded, start + std::time::Duration::from_millis(250)),
            TickHealthLog::Quiet
        ));
        let ok: Result<_, meerkat_schedule::ScheduleDomainError> =
            Ok(meerkat_schedule::ScheduleTickReport::default());
        assert!(matches!(
            tracker.observe(&ok, start + std::time::Duration::from_secs(1)),
            TickHealthLog::Recovered { after: 2 }
        ));
    }

    /// A fingerprint change mid-outage restarts the heartbeat counter but
    /// must NOT restart the outage counter: recovery reports the whole
    /// unhealthy stretch.
    #[test]
    fn tick_health_tracker_reports_full_outage_across_fingerprint_changes() {
        let heartbeat = std::time::Duration::from_secs(60);
        let mut tracker = TickHealthTracker::new(heartbeat);
        let start = meerkat_core::time_compat::Instant::now();
        let failure_a: Result<meerkat_schedule::ScheduleTickReport, _> = Err(
            meerkat_schedule::ScheduleDomainError::Internal("failure a".to_string()),
        );
        let failure_b: Result<meerkat_schedule::ScheduleTickReport, _> = Err(
            meerkat_schedule::ScheduleDomainError::Internal("failure b".to_string()),
        );
        let ok: Result<_, meerkat_schedule::ScheduleDomainError> =
            Ok(meerkat_schedule::ScheduleTickReport::default());

        for tick in 0..5 {
            let _ = tracker.observe(
                &failure_a,
                start + std::time::Duration::from_millis(250 * tick),
            );
        }
        assert!(matches!(
            tracker.observe(&failure_b, start + std::time::Duration::from_secs(2)),
            TickHealthLog::Incident(_)
        ));
        match tracker.observe(&ok, start + std::time::Duration::from_secs(3)) {
            TickHealthLog::Recovered { after } => assert_eq!(after, 6),
            other => panic!("expected recovery, got {other:?}"),
        }
    }

    /// Ask 16 regression, loop-level: the spawned schedule host loop must
    /// FEED tick outcomes into the health tracker and log them. Reverting
    /// the loop body to `let _ = driver.tick_once().await;` (the exact
    /// silent-discard shape the field incident hit) fails this test.
    #[tokio::test]
    async fn spawn_schedule_host_logs_failing_ticks() {
        use uuid::Uuid;

        struct FailingScheduleStore;

        #[async_trait]
        impl ScheduleStore for FailingScheduleStore {
            fn kind(&self) -> meerkat_schedule::ScheduleStoreKind {
                meerkat_schedule::ScheduleStoreKind::Memory
            }

            async fn get_store_time_utc(
                &self,
            ) -> Result<chrono::DateTime<chrono::Utc>, meerkat_schedule::ScheduleStoreError>
            {
                Ok(chrono::Utc::now())
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

        let store = Arc::new(FailingScheduleStore) as Arc<dyn ScheduleStore>;
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

        let logs = String::from_utf8(buf.lock().expect("log buffer lock").clone())
            .expect("captured logs should be utf8");
        assert!(
            logs.contains("schedule driver tick is failing or degraded"),
            "the host loop must log failing tick outcomes, got: {logs}"
        );
        assert!(
            logs.contains("synthetic store outage"),
            "the incident log must carry the tick failure detail, got: {logs}"
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
        let dispatch = host
            .deliver_mob_target(&occurrence, &binding)
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
            _prompt_system_prompt: Option<&str>,
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
            _dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(occurrence, None))
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            _event_type: String,
            _payload: serde_json::Value,
            _render_metadata: Option<RenderMetadata>,
            _materialized_session_id: Option<SessionId>,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(occurrence, None))
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
            _prompt_system_prompt: Option<&str>,
        ) -> Result<SessionId, ScheduleDomainError> {
            panic!("identity targets must resolve existing materialized sessions")
        }

        async fn deliver_prompt(
            &self,
            session_id: &SessionId,
            occurrence: &Occurrence,
            dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            assert_eq!(dispatch.materialized_session_id, None);
            *self
                .delivered_session_id
                .lock()
                .expect("delivered session lock") = Some(session_id.clone());
            Ok(immediate_completed_dispatch(occurrence, None))
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            _event_type: String,
            _payload: serde_json::Value,
            _render_metadata: Option<RenderMetadata>,
            _materialized_session_id: Option<SessionId>,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(occurrence, None))
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
        let resolved = adapter
            .resolve_session(&stale, stale_binding)
            .await
            .expect("reuse guard should resolve without a delivery failure");

        assert_eq!(resolved.session_id, bound_id);
        assert_eq!(resolved.materialized_session_id, Some(bound_id));
        assert!(
            !resolved.allow_system_prompt_override,
            "reused session must not allow a fresh system-prompt override"
        );
        assert_eq!(
            materialize_calls.load(Ordering::SeqCst),
            0,
            "Layer B guard must reuse the bound id, never call materialize_session"
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

        let dispatch = adapter
            .deliver_occurrence(&occurrence)
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

        let dispatch = adapter
            .deliver_occurrence(&occurrence)
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

        let dispatch = adapter
            .deliver_occurrence(&occurrence)
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

        let dispatch = adapter
            .deliver_occurrence(&occurrence)
            .await
            .expect("delivery dispatch");
        assert_eq!(
            dispatch.receipt.stage,
            DeliveryReceiptStage::DispatchAccepted
        );
        let terminal = dispatch.completion.await.expect("delivery terminal");

        assert_eq!(terminal.phase, OccurrencePhase::Completed);
        assert_eq!(terminal.delivery_failure_reason, None);

        let recorded = invocations.lock().expect("invocation lock");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].occurrence_id, occurrence.occurrence_id);
        assert_eq!(recorded[0].schedule_id, occurrence.schedule_id);
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

        let dispatch = adapter
            .deliver_occurrence(&occurrence)
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
