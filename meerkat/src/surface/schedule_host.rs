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
    DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal, IdentityTargetBinding,
    MobTargetBinding, Occurrence, OccurrencePhase, ScheduleDomainError, ScheduleDriver,
    ScheduleDriverConfig, ScheduleFilter, ScheduleService, ScheduleStoreKind,
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
        }
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
        }
    }
}

pub fn schedule_host_supported(kind: ScheduleStoreKind) -> bool {
    !matches!(kind, ScheduleStoreKind::Disabled | ScheduleStoreKind::Jsonl)
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
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {
                    let _ = driver.tick_once().await;
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
        loop {
            tokio_with_wasm::alias::select! {
                _ = &mut shutdown_rx => break,
                () = interval.tick() => {
                    let _ = driver.tick_once().await;
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
        CompletionWaitError::AuthorityUnavailable(_) => {
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
}
