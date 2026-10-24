use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat_core::lifecycle::InputId;
use meerkat_core::{ContentInput, Session, SessionId, skills::SkillRef, types::RenderMetadata};
use meerkat_runtime::{
    CompletionHandle, MeerkatMachine, RuntimeDriverError, SessionServiceRuntimeExt,
    completion::{CompletionOutcome, CompletionWaitError},
    meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
};
use meerkat_schedule::{
    DeliveryCompletion, DeliveryCompletionFailureReason, DeliveryDispatch, DeliveryFailureReason,
    DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal, ExistingDelivery,
    ExistingDeliveryAnswer, HostRunnableInvocation, HostRunnableParams, HostRunnableTargetBinding,
    IdentityTargetBinding, MobTargetBinding, Occurrence, OccurrencePhase, RunnableProbe,
    ScheduleDeliveryIdentity, ScheduleDomainError, ScheduleDriver, ScheduleDriverConfig,
    ScheduleFilter, ScheduleRunnableHost, ScheduleService, ScheduleStoreKind,
    ScheduleStoreWakeMode, ScheduleTargetDelivery, ScheduleTargetProbe, ScheduledSessionAction,
    SessionMaterializationSpec, SessionTargetBinding, TargetBinding, TargetProbeOutcome,
    UpdateScheduleRequest,
};
use serde::{Deserialize, Serialize};

#[cfg(target_arch = "wasm32")]
use crate::tokio as schedule_host_tokio;
#[cfg(not(target_arch = "wasm32"))]
use tokio as schedule_host_tokio;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::{oneshot, watch};
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::{oneshot, watch};
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::task::JoinHandle;

pub struct ScheduleHostHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    worker_shutdown: ScheduleHostWorkerShutdown,
    join: Option<JoinHandle<()>>,
}

impl ScheduleHostHandle {
    fn request_shutdown(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.worker_shutdown.signal();
    }

    /// Whether the host supervisor task is still alive.
    ///
    /// Worker failures do not flip this while the supervisor is applying its
    /// bounded restart policy. A finished supervisor is observable by surface
    /// owners, which can replace the stale handle.
    pub fn is_running(&self) -> bool {
        self.join.as_ref().is_some_and(|join| !join.is_finished())
    }
    pub async fn shutdown(mut self) {
        self.request_shutdown();
        // Await through the handle still owned by `self`. If this future is
        // cancelled, Drop can still stop the worker while the detached
        // supervisor completes executor-lease cleanup.
        if let Some(join) = self.join.as_mut()
            && let Err(error) = join.await
        {
            tracing::error!(%error, "schedule host supervisor task terminated");
        }
        drop(self.join.take());
    }
}

impl Drop for ScheduleHostHandle {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

#[derive(Debug, Clone)]
struct ResolvedScheduledSession {
    session_id: SessionId,
    materialized_session_id: Option<SessionId>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum AcceptedScheduledInputCompletion {
    /// The admitted input's live completion. `outcome_read` reads the
    /// input's own terminal when the wait fails with a rolled-back boundary
    /// or an unknown durable outcome
    /// ([`completion_after_wait_failure`]); a handle built without its input
    /// has none.
    Handle {
        wait: AdmittedCompletionWait,
        outcome_read: Option<AdmittedInputOutcomeRead>,
    },
    Terminal(CompletionOutcome),
    AuthorityUnavailable {
        detail: String,
    },
    /// The input was admitted and its terminal outcome cannot be read yet.
    /// The completion re-reads it under the host's bounds while the driver
    /// keeps renewing the claim's lease ([`reread_admitted_input_outcome`]).
    #[cfg(not(target_arch = "wasm32"))]
    Reread(AdmittedOutcomeReread),
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
    /// An admission whose completion is `handle`. The handle alone names no
    /// input to read, so a wait that fails ends the delivery with that
    /// failure, a rolled-back boundary or an unknown durable outcome
    /// included. The runtime-backed hosts admit through
    /// [`runtime_delivery_dispatch_from_admission`], which reads the input's
    /// own terminal on those two failures instead.
    pub fn with_runtime_handle(correlation_id: Option<String>, handle: CompletionHandle) -> Self {
        Self {
            correlation_id,
            admission_outcome: ScheduleAdmissionOutcome::Accepted,
            completion: AcceptedScheduledInputCompletion::Handle {
                wait: Box::pin(handle.try_wait()),
                outcome_read: None,
            },
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

    /// Production reaches a terminal at admission through the bounded read
    /// in [`runtime_delivery_dispatch_from_admission`]; tests build one here.
    #[cfg(test)]
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

#[derive(Debug, Clone)]
pub struct ScheduledEventDispatch {
    pub event_type: String,
    pub payload: serde_json::Value,
    pub render_metadata: Option<RenderMetadata>,
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
    format!(
        "{}{json}",
        meerkat_schedule::MOB_MEMBER_IDENTITY_TARGET_PREFIX
    )
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
    let json = identity.strip_prefix(meerkat_schedule::MOB_MEMBER_IDENTITY_TARGET_PREFIX)?;
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

    /// Whether this host decides a redelivery's existing-only admission
    /// ([`ExistingDelivery`]): its [`Self::deliver_prompt`] and
    /// [`Self::deliver_event`] admit through
    /// [`accept_schedule_runtime_input_with_reconciliation`] (or, when their
    /// admission is fenced,
    /// [`accept_fenced_schedule_runtime_input_with_reconciliation`] under
    /// that fence), on the task that calls them, with the runtime they admit
    /// through. The adapter then
    /// runs a redelivery inside a redelivery scope that helper reads, so the
    /// runtime only finds the earlier admission and never admits the
    /// delivery fresh; a host that declares this and admits any other way is
    /// reported as an incident.
    ///
    /// The default `false` fails closed: the adapter never asks the host for
    /// an [`ExistingDelivery::UnprovenLate`] redelivery (the occurrence ends
    /// misfired, as before), and delivers an [`ExistingDelivery::Admitted`]
    /// one through the ordinary [`Self::deliver_prompt`] or
    /// [`Self::deliver_event`], whose stable-key deduplication finds the
    /// earlier admission (a host that lost it admits the delivery again).
    fn decides_existing_admission(&self) -> bool {
        false
    }

    /// Admit the prompt under `identity.idempotency_key` before returning.
    /// An exact replay must return the original admission result without a
    /// second input. Ordinary errors certify that admission did not occur;
    /// an unknown post-admission outcome uses `DeliveryRepairDeferred`. A
    /// host that declares [`Self::decides_existing_admission`] admits through
    /// [`accept_schedule_runtime_input_with_reconciliation`] on this task.
    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;

    /// Admit the event under the same stable-key and ambiguous-outcome
    /// contract as [`Self::deliver_prompt`].
    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        dispatch: ScheduledEventDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;
}

#[async_trait]
pub trait SurfaceScheduleMobHost: Send + Sync {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError>;

    /// Admit the mob action under `identity.idempotency_key`. Exact replays
    /// must deduplicate, and an ambiguous post-admission outcome must surface
    /// as `DeliveryRepairDeferred` rather than an ordinary terminal error.
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

    /// When this host owns the identity, delivery follows the same stable-key
    /// admission contract as [`Self::deliver_mob_target`].
    async fn deliver_identity_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &IdentityTargetBinding,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        let _ = (occurrence, identity, binding);
        Ok(None)
    }

    /// Redeliver a mob target whose earlier attempt recorded its dispatch,
    /// without a fresh delivery ([`ExistingDelivery`]). The default delivers
    /// an admitted one through [`Self::deliver_mob_target`], whose durable
    /// stable-key records answer an exact replay, and answers
    /// [`ExistingDeliveryAnswer::NotAdmittedPastDeadline`] for an unproven
    /// late one: a host that cannot tell whether it admitted the delivery
    /// never delivers a late occurrence fresh.
    async fn deliver_existing_mob_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &MobTargetBinding,
        existing: ExistingDelivery,
    ) -> Result<ExistingDeliveryAnswer, ScheduleDomainError> {
        match existing {
            ExistingDelivery::Admitted => self
                .deliver_mob_target(occurrence, identity, binding)
                .await
                .map(ExistingDeliveryAnswer::Dispatch),
            _ => Ok(ExistingDeliveryAnswer::NotAdmittedPastDeadline),
        }
    }

    /// The same for an identity this host may own. `None` when it does not
    /// own the identity (the session host delivers it). The default
    /// delivers an admitted one through [`Self::deliver_identity_target`],
    /// and answers an unproven late one for an identity it owns (its probe
    /// answers) with [`ExistingDeliveryAnswer::NotAdmittedPastDeadline`].
    async fn deliver_existing_identity_target(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        binding: &IdentityTargetBinding,
        existing: ExistingDelivery,
    ) -> Result<Option<ExistingDeliveryAnswer>, ScheduleDomainError> {
        match existing {
            ExistingDelivery::Admitted => Ok(self
                .deliver_identity_target(occurrence, identity, binding)
                .await?
                .map(ExistingDeliveryAnswer::Dispatch)),
            _ => Ok(self
                .probe_identity_target(binding)
                .await?
                .map(|_| ExistingDeliveryAnswer::NotAdmittedPastDeadline)),
        }
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

/// The agent a governed host's scheduled sessions act as, and its binding
/// onto a session the host materializes for a firing schedule.
///
/// A host composed with one (see
/// [`SharedScheduleTargetAdapter::with_session_agent_binding`]) fires a
/// schedule only when its recorded commissioning agent
/// ([`meerkat_schedule::ScheduleConfig::commissioning_agent`]) is
/// [`Self::agent`]; every other occurrence, a schedule recorded with no agent
/// included, fails as `schedule_mandate_unavailable` when it is delivered
/// (after the driver's target probe, before the target is resolved for
/// delivery). A session the host materializes or revives for a firing
/// schedule must be bound with [`Self::bind_scheduled_session`] after its
/// registration and before its agent is built, as an interactive session is.
/// The adapter only gates: the session host binds, and no session host in
/// this crate does yet.
#[async_trait]
pub trait ScheduledSessionAgentBinding: Send + Sync {
    /// The agent whose mandate this host's scheduled sessions act under.
    fn agent(&self) -> &meerkat_core::PrincipalRef;

    /// Bind [`Self::agent`] to `session_id`'s current registration on
    /// `machine`, before the session's agent is built. `configured_servers`
    /// are the configured MCP servers the session's build carries.
    async fn bind_scheduled_session(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        configured_servers: &[meerkat_core::McpServerConfig],
    ) -> Result<(), ScheduleDomainError>;
}

pub struct SharedScheduleTargetAdapter {
    /// The governed agent this host fires schedules for, if any; see
    /// [`ScheduledSessionAgentBinding`].
    session_agent_binding: Option<Arc<dyn ScheduledSessionAgentBinding>>,
    schedule_service: ScheduleService,
    session_host: Arc<dyn SurfaceScheduleSessionHost>,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    /// The create-time identity-target probe attached to `schedule_service`.
    /// The service holds it weakly, so it judges for as long as this adapter
    /// (and with it the schedule host that delivers through it) lives.
    _identity_target_probe: Arc<dyn meerkat_schedule::ScheduleIdentityTargetProbe>,
}

/// Create-time identity-target deliverability judgment composed from the
/// same host pair the fire-time probe consults.
///
/// A mob host that recognizes the identity form vouches for the class even
/// when the member is currently absent: Ready, Busy, and Missing are all
/// fire-time conditions governed by `missing_target_policy`. Only an
/// identity no host recognizes (mob host disclaims it and the session host
/// probes Missing) is refused as never deliverable.
struct SurfaceIdentityTargetProbe {
    session_host: Arc<dyn SurfaceScheduleSessionHost>,
    mob_host: Arc<dyn SurfaceScheduleMobHost>,
}

#[async_trait]
impl meerkat_schedule::ScheduleIdentityTargetProbe for SurfaceIdentityTargetProbe {
    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<meerkat_schedule::IdentityTargetDeliverability, ScheduleDomainError> {
        if self
            .mob_host
            .probe_identity_target(binding)
            .await?
            .is_some()
        {
            return Ok(meerkat_schedule::IdentityTargetDeliverability::Deliverable);
        }
        match self.session_host.probe_identity_target(binding).await? {
            TargetProbeOutcome::Ready | TargetProbeOutcome::Busy { .. } => {
                Ok(meerkat_schedule::IdentityTargetDeliverability::Deliverable)
            }
            TargetProbeOutcome::Missing { detail } => {
                Ok(meerkat_schedule::IdentityTargetDeliverability::NeverDeliverable { detail })
            }
        }
    }
}

impl SharedScheduleTargetAdapter {
    /// Compose the delivery adapter for `schedule_service` from the surface's
    /// session and mob hosts.
    ///
    /// This also attaches both hosts to `schedule_service` as its create-time
    /// identity-target probe. The adapter owns the probe and the service holds
    /// it weakly, so the hosts judge identity targets only while this adapter
    /// lives (a schedule host keeps it until the host stops), and no clone of
    /// the service, such as one inside agent schedule tools, keeps the hosts
    /// or what they hold (a session service, mob state) alive. See
    /// [`spawn_schedule_host`] for what an identity create sees once the host
    /// has stopped.
    ///
    /// A host over a stock-governed realm passes the realm's commissioned
    /// service ([`crate::PersistenceBundle::schedule_service`]) and composes
    /// [`crate::PersistenceBundle::scheduled_session_agent_binding`] with
    /// [`Self::with_session_agent_binding`] once its session host binds
    /// scheduled sessions. Without the binding the adapter refuses each
    /// occurrence as `schedule_mandate_unavailable` when it is delivered,
    /// after the driver's target probe, before the target is resolved for
    /// delivery or a session is materialized. A service from
    /// [`ScheduleService::new`] over a governed realm's store commissions no
    /// agent, so the adapter fires its schedules unchecked and each fire
    /// materializes its session and then fails late (see Known issues).
    pub fn new(
        schedule_service: ScheduleService,
        session_host: Arc<dyn SurfaceScheduleSessionHost>,
        mob_host: Arc<dyn SurfaceScheduleMobHost>,
    ) -> Self {
        // Constructing the delivery adapter is the surface's declaration of
        // its host capabilities; attach them as the service's create-time
        // deliverability authority so `schedule_create` fails closed instead
        // of persisting schedules whose every occurrence would misfire.
        let identity_target_probe: Arc<dyn meerkat_schedule::ScheduleIdentityTargetProbe> =
            Arc::new(SurfaceIdentityTargetProbe {
                session_host: Arc::clone(&session_host),
                mob_host: Arc::clone(&mob_host),
            });
        schedule_service.attach_identity_target_probe(&identity_target_probe);
        Self {
            session_agent_binding: None,
            schedule_service,
            session_host,
            mob_host,
            runnable_host: None,
            _identity_target_probe: identity_target_probe,
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

    /// Fire schedules only under `binding`'s agent: a governed host's
    /// composition supplies it (a stock-governed realm's opened agent, see
    /// [`crate::PersistenceBundle::scheduled_session_agent_binding`]). Each
    /// occurrence whose schedule records another commissioning agent, or
    /// none, fails as `schedule_mandate_unavailable` when it is delivered:
    /// after the driver's target probe (a target the probe finds missing
    /// still misfires or fails as `target_missing`), before the target is
    /// resolved for delivery or a session is materialized.
    ///
    /// This only gates. The surface's session host must itself bind the
    /// agent onto each session it materializes or revives for a fire
    /// ([`ScheduledSessionAgentBinding::bind_scheduled_session`]); no session
    /// host in this crate (nor rkat's, rkat-rpc's or the MCP server's) does
    /// yet, so a binding composed with one of them passes the gate and the
    /// fire's session still runs unbound.
    ///
    /// Without a binding, a schedule service that commissions schedules for
    /// an agent ([`ScheduleService::commissioning_agent`]) fires none of them:
    /// this host cannot bind that agent. Only a host whose service
    /// commissions none (an ungoverned realm) fires schedules unchecked.
    #[must_use]
    pub fn with_session_agent_binding(
        mut self,
        binding: Arc<dyn ScheduledSessionAgentBinding>,
    ) -> Self {
        self.session_agent_binding = Some(binding);
        self
    }

    /// The occurrence's failure when its schedule may not fire under this
    /// host's agent, or `None` when it may: its recorded commissioning agent
    /// is the bound agent, or this host is ungoverned.
    async fn refuse_foreign_mandate(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
    ) -> Result<Option<DeliveryDispatch>, ScheduleDomainError> {
        let detail = match (
            &self.session_agent_binding,
            self.schedule_service.commissioning_agent(),
        ) {
            (None, None) => return Ok(None),
            (None, Some(commissioning)) => format!(
                "schedule {} cannot fire: this host cannot bind agent {} onto its sessions",
                occurrence.schedule_id, commissioning.id
            ),
            (Some(binding), _) => {
                let agent = binding.agent();
                let schedule = self.schedule_service.get(&occurrence.schedule_id).await?;
                match schedule.config.commissioning_agent.as_ref() {
                    Some(commissioning) if commissioning == agent => return Ok(None),
                    Some(commissioning) => format!(
                        "schedule {} was commissioned by agent {} and this host acts as agent {}",
                        occurrence.schedule_id, commissioning.id, agent.id
                    ),
                    None => format!(
                        "schedule {} records no commissioning agent and this host acts as agent {}",
                        occurrence.schedule_id, agent.id
                    ),
                }
            }
        };
        // The driver requires every dispatch for the occurrence, a refusal
        // included, to name the session an earlier fire bound to its target.
        Ok(Some(immediate_delivery_failure(
            occurrence,
            detail,
            DeliveryFailureReason::ScheduleMandateUnavailable,
            Some(identity.correlation_id.clone()),
            bound_target_session_id(&occurrence.target_snapshot),
        )))
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
                        ScheduledEventDispatch {
                            event_type: event_type.clone(),
                            payload: payload.clone(),
                            render_metadata: render_metadata.clone(),
                            materialized_session_id: resolved.materialized_session_id,
                        },
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
        // A governed host fires a schedule only under the agent whose mandate
        // commissioned it: refused here, after the driver's target probe and
        // before the target is resolved for delivery or materialized.
        if let Some(refused) = self.refuse_foreign_mandate(occurrence, identity).await? {
            return Ok(refused);
        }
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

    /// Redeliver a reclaimed occurrence whose earlier attempt recorded its
    /// dispatch. Nothing is materialized or created first: an on-demand
    /// session target the occurrence holds unbound is looked for only where an
    /// earlier attempt could have admitted it
    /// (`Self::deliver_existing_unbound_session`). A session target's
    /// runtime admission may only deduplicate against the earlier input (the
    /// runtime's deduplicate-only admission, decided where it decides
    /// deduplication); mob targets and mob-owned identities go to the mob
    /// host.
    #[cfg(not(target_arch = "wasm32"))]
    async fn deliver_existing_occurrence(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        existing: ExistingDelivery,
    ) -> Result<ExistingDeliveryAnswer, ScheduleDomainError> {
        if let Some(refused) = self.refuse_foreign_mandate(occurrence, identity).await? {
            return Ok(ExistingDeliveryAnswer::Dispatch(refused));
        }
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                let session_id = match binding.as_ref() {
                    SessionTargetBinding::ExactSession { session_id, .. }
                    | SessionTargetBinding::ResumableSession { session_id, .. } => {
                        session_id.clone()
                    }
                    SessionTargetBinding::MaterializeOnDemandSession {
                        bound_session_id: Some(session_id),
                        ..
                    } => session_id.clone(),
                    SessionTargetBinding::MaterializeOnDemandSession {
                        bound_session_id: None,
                        ..
                    } => {
                        return self
                            .deliver_existing_unbound_session(
                                occurrence,
                                identity,
                                binding.action(),
                                existing,
                            )
                            .await;
                    }
                };
                let materialized_session_id = bound_target_session_id(&occurrence.target_snapshot);
                let resolved = ResolvedScheduledSession {
                    session_id,
                    materialized_session_id,
                };
                self.deliver_existing_session_action(
                    occurrence,
                    identity,
                    resolved,
                    binding.action(),
                    existing,
                )
                .await
            }
            TargetBinding::Identity(binding) => {
                if let Some(answer) = self
                    .mob_host
                    .deliver_existing_identity_target(occurrence, identity, binding, existing)
                    .await?
                {
                    return Ok(answer);
                }
                // An identity that no longer resolves holds no earlier
                // admission to replay.
                let Ok(resolved) = self.resolve_identity(occurrence, identity, binding).await
                else {
                    return Ok(existing_delivery_unknown(occurrence, identity, existing));
                };
                self.deliver_existing_session_action(
                    occurrence,
                    identity,
                    resolved,
                    binding.action(),
                    existing,
                )
                .await
            }
            TargetBinding::Mob(binding) => {
                self.mob_host
                    .deliver_existing_mob_target(occurrence, identity, binding, existing)
                    .await
            }
            // An in-process callback keeps no admission record: an admitted
            // one runs as before, an unproven late one is never run fresh.
            TargetBinding::HostRunnable(binding) => match existing {
                ExistingDelivery::Admitted => Ok(ExistingDeliveryAnswer::Dispatch(
                    self.deliver_host_runnable(occurrence, identity, binding),
                )),
                _ => Ok(ExistingDeliveryAnswer::NotAdmittedPastDeadline),
            },
        }
    }
}

/// The answer for a redelivery whose target holds no earlier admission: an
/// admitted one fails with nothing admitted, an unproven late one was never
/// admitted.
#[cfg(not(target_arch = "wasm32"))]
fn existing_delivery_unknown(
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    existing: ExistingDelivery,
) -> ExistingDeliveryAnswer {
    match existing {
        ExistingDelivery::Admitted => ExistingDeliveryAnswer::Dispatch(immediate_delivery_failure(
            occurrence,
            "the admitted input is no longer known to the target; nothing was admitted".to_string(),
            DeliveryFailureReason::RuntimeRejected,
            Some(identity.correlation_id.clone()),
            None,
        )),
        _ => ExistingDeliveryAnswer::NotAdmittedPastDeadline,
    }
}

/// A redelivery in progress whose runtime admission may only deduplicate
/// against the earlier input ([`ExistingDelivery`]). The adapter sets it
/// around the session host's delivery, and
/// [`accept_schedule_runtime_input_with_reconciliation`], which every
/// runtime-backed host's delivery reaches, admits through the runtime's
/// deduplicate-only admission while it is set. `not_admitted` records that
/// an unproven late redelivery found no earlier admission; `consumed` that
/// the helper read the scope, the witness that the host admitted through it.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct ExistingAdmissionScope {
    existing: ExistingDelivery,
    not_admitted: Arc<std::sync::atomic::AtomicBool>,
    consumed: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
tokio::task_local! {
    static EXISTING_ADMISSION: ExistingAdmissionScope;
}

/// The redelivery scope of the current delivery, if any.
#[cfg(not(target_arch = "wasm32"))]
fn existing_admission_scope() -> Option<ExistingAdmissionScope> {
    EXISTING_ADMISSION.try_with(Clone::clone).ok()
}

impl SharedScheduleTargetAdapter {
    /// Redeliver to an on-demand session target the occurrence holds unbound.
    ///
    /// Nothing is materialized. The binding is read again first, as the
    /// ordinary delivery does before it materializes: another attempt may
    /// have committed it since this occurrence was claimed. Without one, the
    /// occurrence's own deterministic session
    /// ([`Occurrence::materialized_session_id`]) is probed read-only, since an
    /// earlier attempt that materialized and admitted into it while no bind
    /// reached this occurrence left the admission only there. A session that
    /// does not exist, or a probe that cannot read it, holds no admission this
    /// redelivery can find.
    ///
    /// The dispatch reports the probed session as materialized only when the
    /// runtime found the earlier admission in it (deduplicated), so the
    /// driver binds and records it exactly as it does for an ordinary
    /// dispatch; a failure, or nothing admitted, binds nothing.
    ///
    /// Known limitation: two overlapping occurrences of one on-demand
    /// schedule can each materialize their own session before either bind
    /// lands. The occurrence whose bind loses admits into its own session; if
    /// its process stops before the driver records that session on it, its
    /// reclaim copies the other occurrence's binding onto it (the reclaim's
    /// target sync), and its redelivery looks only in that session, where its
    /// admission is not. An admitted redelivery then ends delivery_failed and
    /// an unproven late one misfires, although the input was admitted.
    #[cfg(not(target_arch = "wasm32"))]
    async fn deliver_existing_unbound_session(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        action: &ScheduledSessionAction,
        existing: ExistingDelivery,
    ) -> Result<ExistingDeliveryAnswer, ScheduleDomainError> {
        if let Some(bound) = self.authoritative_bound_session_id(occurrence).await {
            let resolved = ResolvedScheduledSession {
                session_id: bound.clone(),
                materialized_session_id: Some(bound),
            };
            return self
                .deliver_existing_session_action(occurrence, identity, resolved, action, existing)
                .await;
        }
        let session_id = occurrence.materialized_session_id();
        let probe = SessionTargetBinding::ExactSession {
            session_id: session_id.clone(),
            action: action.clone(),
        };
        match self.session_host.probe_session_target(&probe).await {
            Ok(TargetProbeOutcome::Ready | TargetProbeOutcome::Busy { .. }) => {}
            Ok(TargetProbeOutcome::Missing { .. }) => {
                return Ok(existing_delivery_unknown(occurrence, identity, existing));
            }
            Err(error) => {
                tracing::warn!(
                    occurrence_id = %occurrence.occurrence_id,
                    %session_id,
                    %error,
                    "a redelivery could not probe its occurrence's own session; it finds no earlier admission"
                );
                return Ok(existing_delivery_unknown(occurrence, identity, existing));
            }
        }
        let resolved = ResolvedScheduledSession {
            session_id: session_id.clone(),
            materialized_session_id: None,
        };
        let answer = self
            .deliver_existing_session_action(occurrence, identity, resolved, action, existing)
            .await?;
        let mut dispatch = match answer {
            ExistingDeliveryAnswer::Dispatch(dispatch) => dispatch,
            not_dispatched => return Ok(not_dispatched),
        };
        let found_earlier_admission = dispatch.receipt.stage
            == DeliveryReceiptStage::DispatchAccepted
            && matches!(
                dispatch.receipt.runtime_outcome,
                Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
            );
        if found_earlier_admission {
            dispatch.materialized_session_id = Some(session_id.clone());
            dispatch.receipt.materialized_session_id = Some(session_id);
        }
        Ok(ExistingDeliveryAnswer::Dispatch(dispatch))
    }

    /// Deliver a session action as a redelivery that may only find the
    /// earlier admission.
    ///
    /// A session host that does not decide an existing-only admission
    /// ([`SurfaceScheduleSessionHost::decides_existing_admission`]) is never
    /// asked for an unproven late redelivery, and gets an admitted one
    /// through its ordinary delivery, like the other defaults. A host that
    /// does runs it inside the redelivery scope; if it reports an admission
    /// without its helper having read the scope, it admitted some other way,
    /// possibly fresh: an incident.
    #[cfg(not(target_arch = "wasm32"))]
    async fn deliver_existing_session_action(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        resolved: ResolvedScheduledSession,
        action: &ScheduledSessionAction,
        existing: ExistingDelivery,
    ) -> Result<ExistingDeliveryAnswer, ScheduleDomainError> {
        if !self.session_host.decides_existing_admission() {
            return match existing {
                ExistingDelivery::Admitted => self
                    .deliver_session_action(occurrence, identity, resolved, action)
                    .await
                    .map(ExistingDeliveryAnswer::Dispatch),
                _ => Ok(ExistingDeliveryAnswer::NotAdmittedPastDeadline),
            };
        }
        let not_admitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let consumed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let scope = ExistingAdmissionScope {
            existing,
            not_admitted: Arc::clone(&not_admitted),
            consumed: Arc::clone(&consumed),
        };
        let delivered = EXISTING_ADMISSION
            .scope(
                scope,
                self.deliver_session_action(occurrence, identity, resolved, action),
            )
            .await;
        if not_admitted.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(ExistingDeliveryAnswer::NotAdmittedPastDeadline);
        }
        let bypassed = !consumed.load(std::sync::atomic::Ordering::SeqCst)
            && matches!(
                &delivered,
                Ok(dispatch) if dispatch.receipt.stage == DeliveryReceiptStage::DispatchAccepted
            );
        if bypassed {
            tracing::error!(
                occurrence_id = %occurrence.occurrence_id,
                ?existing,
                "a session host that decides existing-only admission admitted a redelivery without consulting the redelivery scope; it may have admitted the delivery fresh"
            );
        }
        debug_assert!(
            !bypassed,
            "redelivery admitted without consulting the redelivery scope ({existing:?})"
        );
        delivered.map(ExistingDeliveryAnswer::Dispatch)
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

async fn wait_for_worker_shutdown_or<F>(
    shutdown_rx: &mut watch::Receiver<bool>,
    operation: F,
) -> Option<F::Output>
where
    F: std::future::Future,
{
    schedule_host_tokio::pin!(operation);
    schedule_host_tokio::select! {
        biased;
        () = wait_for_worker_shutdown(shutdown_rx) => None,
        output = &mut operation => Some(output),
    }
}

async fn wait_for_worker_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow_and_update() {
            return;
        }
        if shutdown_rx.changed().await.is_err() {
            return;
        }
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

#[derive(Clone)]
struct ScheduleHostWorkerShutdown {
    sender: watch::Sender<bool>,
}

impl Default for ScheduleHostWorkerShutdown {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self { sender }
    }
}

impl ScheduleHostWorkerShutdown {
    fn subscribe(&self) -> watch::Receiver<bool> {
        self.sender.subscribe()
    }

    fn signal(&self) {
        self.sender.send_replace(true);
    }
}

struct ScheduleHostWorkerTask {
    join: JoinHandle<Result<(), ScheduleHostWorkerExit>>,
}

#[derive(Clone, Copy)]
struct ScheduleHostTiming {
    wake_mode: ScheduleStoreWakeMode,
    base_interval: Duration,
    backoff_cap: Duration,
    stable_window: Duration,
}

impl ScheduleHostWorkerTask {
    fn spawn(
        schedule_service: ScheduleService,
        driver: Arc<ScheduleDriver>,
        timing: ScheduleHostTiming,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            join: schedule_host_tokio::task::spawn(run_schedule_host_worker(
                schedule_service,
                driver,
                timing,
                shutdown_rx,
            )),
        }
    }

    fn join_mut(&mut self) -> &mut JoinHandle<Result<(), ScheduleHostWorkerExit>> {
        &mut self.join
    }
}

impl Drop for ScheduleHostWorkerTask {
    fn drop(&mut self) {
        self.join.abort();
    }
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
    timing: ScheduleHostTiming,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ScheduleHostWorkerExit> {
    let mut health = ScheduleHostIncidentTracker::new(std::time::Duration::from_secs(60));

    let mut backoff = TickBackoff::new(
        timing.base_interval,
        timing.backoff_cap,
        IDLE_TICKS_BEFORE_BACKOFF,
    );
    let mut delay = Some(timing.base_interval);
    let mut mutation_rx = schedule_service.subscribe_mutations();
    let store = schedule_service.store();

    loop {
        schedule_host_tokio::select! {
            biased;
            () = wait_for_worker_shutdown(&mut shutdown_rx) => return Ok(()),
            mutation = mutation_rx.changed() => {
                if mutation.is_err() {
                    return Err(ScheduleHostWorkerExit::MutationSignalClosed);
                }
            }
            durable_wake = wait_for_durable_store_wake(Arc::clone(&store), timing.wake_mode) => {
                if let Err(error) = durable_wake {
                    return Err(ScheduleHostWorkerExit::DurableWakeFailed(error));
                }
            }
            () = optional_schedule_host_sleep(delay) => {}
        }

        let outcome = match driver
            .tick_once_until_cancelled(wait_for_worker_shutdown(&mut shutdown_rx))
            .await
        {
            Ok(Some(report)) => Ok(report),
            Ok(None) => return Ok(()),
            Err(error) => Err(error),
        };
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
            Ok(report) => {
                match wait_for_worker_shutdown_or(&mut shutdown_rx, store.next_action_time_utc())
                    .await
                {
                    None => return Ok(()),
                    Some(action) => match action {
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
                                    timing.wake_mode,
                                    backoff.current,
                                    timing.base_interval,
                                ),
                                incident,
                            )
                        }
                    },
                }
            }
        };
        log_schedule_host_health(
            health.observe(incident, meerkat_core::time_compat::Instant::now()),
        );
        let heartbeat = driver.executor_heartbeat_interval();
        delay = Some(next_delay.map_or(heartbeat, |delay| delay.min(heartbeat)));
    }
}

async fn supervise_schedule_host(
    schedule_service: ScheduleService,
    driver: Arc<ScheduleDriver>,
    timing: ScheduleHostTiming,
    mut shutdown_rx: oneshot::Receiver<()>,
    worker_shutdown: ScheduleHostWorkerShutdown,
) {
    let mut health = ScheduleHostIncidentTracker::new(std::time::Duration::from_secs(60));
    let mut restart_backoff = RestartBackoff::new(timing.base_interval, timing.backoff_cap);

    loop {
        let worker_shutdown_rx = worker_shutdown.subscribe();
        let mut worker = ScheduleHostWorkerTask::spawn(
            schedule_service.clone(),
            Arc::clone(&driver),
            timing,
            worker_shutdown_rx,
        );
        let mut stable_timer = Box::pin(schedule_host_tokio::time::sleep(timing.stable_window));
        let mut stable = false;

        let worker_result = loop {
            schedule_host_tokio::select! {
                biased;
                _ = &mut shutdown_rx => {
                    worker_shutdown.signal();
                    match worker.join_mut().await {
                        Ok(Ok(())) => {}
                        Ok(Err(exit)) => log_schedule_host_health(health.observe(
                            Some(exit.into_incident()),
                            meerkat_core::time_compat::Instant::now(),
                        )),
                        Err(error) => log_schedule_host_health(health.observe(
                            Some(ScheduleHostIncident::new(
                                ScheduleHostIncidentClass::WorkerPanicked,
                                format!("schedule host worker task terminated during shutdown: {error}"),
                            )),
                            meerkat_core::time_compat::Instant::now(),
                        )),
                    }
                    if let Err(error) = driver.release_executor_lease().await {
                        tracing::warn!(%error, "failed to release schedule executor lease during shutdown");
                    }
                    return;
                }
                result = worker.join_mut() => break result,
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
            biased;
            _ = &mut shutdown_rx => {
                if let Err(error) = driver.release_executor_lease().await {
                    tracing::warn!(%error, "failed to release schedule executor lease during shutdown");
                }
                return;
            },
            () = schedule_host_tokio::time::sleep(restart_delay) => {}
        }
    }
}

/// The claim lease every schedule host fires occurrences under. The
/// completion waiter renews it at half this period while a delivery runs.
const SCHEDULE_HOST_LEASE_SECONDS: i64 = 60;

/// How long a delivery whose admission is proven re-reads its input's
/// terminal outcome while that outcome cannot be read yet: five periods of
/// [`SCHEDULE_HOST_LEASE_SECONDS`]. The waiter keeps the claim's lease through
/// the wait, so nothing reclaims the occurrence and its misfire policy plays
/// no part. At the bound the occurrence ends delivery_failed with
/// runtime_completion_authority_unavailable.
const ADMITTED_OUTCOME_REREAD_BOUND: Duration = Duration::from_secs(300);

/// Bound on one read of that outcome: a quarter of
/// [`SCHEDULE_HOST_LEASE_SECONDS`]. The first read runs inside the delivery,
/// before the waiter starts renewing, so it must end well inside the claim's
/// lease. A read that hits this bound counts as not readable yet.
const ADMITTED_OUTCOME_READ_BOUND: Duration = Duration::from_secs(15);

/// First pause between two reads. It doubles up to
/// [`ADMITTED_OUTCOME_READ_BOUND`].
const ADMITTED_OUTCOME_REREAD_FIRST_BACKOFF: Duration = Duration::from_secs(1);

/// The claim lease of the [`ScheduleDriver`] the in-crate host tests run
/// with the test profile of the bounds below.
#[cfg(test)]
const TEST_SCHEDULE_DRIVER_LEASE: Duration = Duration::from_secs(1);

/// Test profile of [`ADMITTED_OUTCOME_REREAD_BOUND`]. It exceeds a 2 s
/// misfire window plus one [`TEST_SCHEDULE_DRIVER_LEASE`], so a wait that
/// gave its lease up at the bound would see its reclaim land past the misfire
/// deadline.
#[cfg(test)]
const TEST_ADMITTED_OUTCOME_REREAD_BOUND: Duration = Duration::from_secs(4);

/// Test profile of [`ADMITTED_OUTCOME_READ_BOUND`]. It stays below
/// [`TEST_SCHEDULE_DRIVER_LEASE`]: the first read runs before the waiter
/// renews the claim, and a slower read would let the lease lapse before the
/// wait starts.
#[cfg(test)]
const TEST_ADMITTED_OUTCOME_READ_BOUND: Duration = Duration::from_millis(500);

/// Test profile of [`ADMITTED_OUTCOME_REREAD_FIRST_BACKOFF`].
#[cfg(test)]
const TEST_ADMITTED_OUTCOME_REREAD_FIRST_BACKOFF: Duration = Duration::from_millis(100);

/// Spawn the schedule host: the driver that plans and fires
/// `schedule_service`'s occurrences and delivers them through `adapter`.
/// Dropping the returned handle stops the host without waiting for it;
/// [`ScheduleHostHandle::shutdown`] stops it and waits until it has finished.
///
/// # Teardown
///
/// The host keeps `adapter` until it stops, and with it the identity-target
/// probe that [`SharedScheduleTargetAdapter::new`] attached to
/// `schedule_service`; the service and its clones hold that probe only
/// weakly. Agent-facing schedule tools in a factory builder's
/// `default_schedule_tools` slot hold a clone of the service, and the
/// session service owns that builder, but once this host has stopped (and
/// any delivery still in flight has settled), nothing reached through the
/// service keeps the runtime-backed session host (from
/// `spawn_runtime_backed_schedule_host` and its `_with_mobs` variants) and
/// the session service it holds, or a mob host such as the
/// `meerkat-mob-mcp` schedule host and the mob state it holds. An embedder
/// that tears these down while its process keeps running clears nothing in
/// the builder; anything it holds itself, such as a registry that resolves
/// schedule targets to mob state, stays its own to release.
///
/// Once no attached probe is alive, the service has no surface left to judge
/// identity targets: an identity-target create, or an update that sets an
/// identity target, fails with [`ScheduleDomainError::DriverStopped`], which
/// the agent-facing schedule tools report as temporarily unavailable. Every
/// other schedule operation keeps working against the store, and a host
/// spawned later for the same service attaches its own probe and judges
/// again.
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
            lease_duration: ChronoDuration::seconds(SCHEDULE_HOST_LEASE_SECONDS),
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
    let timing = ScheduleHostTiming {
        wake_mode,
        base_interval,
        backoff_cap,
        stable_window,
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let worker_shutdown = ScheduleHostWorkerShutdown::default();
    let join = schedule_host_tokio::task::spawn(supervise_schedule_host(
        schedule_service,
        driver,
        timing,
        shutdown_rx,
        worker_shutdown.clone(),
    ));

    ScheduleHostHandle {
        shutdown_tx: Some(shutdown_tx),
        worker_shutdown,
        join: Some(join),
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
            AcceptedScheduledInputCompletion::Handle { wait, outcome_read } => match wait.await {
                Ok(outcome) => outcome,
                // The input's known status, from the body-free generated
                // observation; its result or callback arguments are withheld
                // from this reader.
                Err(CompletionWaitError::DisclosureRefused { observed }) => {
                    let outcome = match observed {
                        RuntimeCompletionObservedOutcome::Completed => {
                            meerkat_schedule::RuntimeCompletionOutcome::Completed
                        }
                        RuntimeCompletionObservedOutcome::CallbackPending => {
                            meerkat_schedule::RuntimeCompletionOutcome::CallbackPending
                        }
                        _ => {
                            let error = CompletionWaitError::DisclosureRefused { observed };
                            return Err(ScheduleDomainError::DeliveryCompletionFailed {
                                reason: completion_wait_failure_reason(&error),
                                detail: error.to_string(),
                            });
                        }
                    };
                    return Ok(DeliveryTerminal::runtime_completion(outcome, None, None));
                }
                Err(error) => completion_after_wait_failure(error, outcome_read).await?,
            },
            AcceptedScheduledInputCompletion::Terminal(terminal) => {
                return Ok(delivery_terminal_from_completion_outcome(
                    terminal,
                    materialized_session_id,
                ));
            }
            AcceptedScheduledInputCompletion::AuthorityUnavailable { detail } => {
                return Err(authority_unavailable_completion(detail));
            }
            #[cfg(not(target_arch = "wasm32"))]
            AcceptedScheduledInputCompletion::Reread(reread) => {
                reread_admitted_input_outcome(reread).await?
            }
        };
        Ok(delivery_terminal_from_completion_outcome(
            outcome,
            materialized_session_id,
        ))
    })
}

fn authority_unavailable_completion(detail: String) -> ScheduleDomainError {
    ScheduleDomainError::DeliveryCompletionFailed {
        reason: DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable,
        detail,
    }
}

/// The delivery failure a failed live wait records when nothing reads past
/// it.
fn wait_failure_completion(error: &CompletionWaitError) -> ScheduleDomainError {
    ScheduleDomainError::DeliveryCompletionFailed {
        reason: completion_wait_failure_reason(error),
        detail: format!("runtime completion authority unavailable: {error}"),
    }
}

/// The completion of a live wait that failed.
///
/// A rolled-back boundary ([`CompletionWaitError::BoundaryRolledBack`]), an
/// unknown durable outcome ([`CompletionWaitError::DurableOutcomeUnknown`])
/// or a failure the generated wait-failure authority resolves as resumable (a
/// graceful stop that kept the input queued,
/// [`CompletionWaitError::RequeuedAcrossStop`]) leaves the input durably
/// admitted with its terminal still to come: it runs again after its session
/// reloads, its stalled commit lands, or the next start runs it. None of these
/// failures is the input's outcome, so when the host knows the input
/// (`outcome_read`), the completion reads the input's own terminal through
/// the bounded re-read loop ([`reread_admitted_input_outcome`]), under the
/// driver's lease renewal, and resolves the occurrence from it; at the bound
/// it ends delivery_failed with runtime_completion_authority_unavailable.
/// Every other wait failure ends the delivery with that failure at once.
#[cfg(not(target_arch = "wasm32"))]
async fn completion_after_wait_failure(
    error: CompletionWaitError,
    outcome_read: Option<AdmittedInputOutcomeRead>,
) -> Result<CompletionOutcome, ScheduleDomainError> {
    let Some(outcome_read) = outcome_read else {
        return Err(wait_failure_completion(&error));
    };
    let input_continues =
        error.is_boundary_rolled_back() || error.is_durable_outcome_unknown() || {
            let resumable = (outcome_read.resumable)(error.clone());
            resumable.await
        };
    if input_continues {
        reread_admitted_input_outcome(outcome_read.into_reread(error.to_string())).await
    } else {
        Err(wait_failure_completion(&error))
    }
}

/// Browser builds run no schedule driver, so no lease is renewed across a
/// re-read there: a failed wait ends the delivery with its failure.
#[cfg(target_arch = "wasm32")]
async fn completion_after_wait_failure(
    error: CompletionWaitError,
    _outcome_read: Option<AdmittedInputOutcomeRead>,
) -> Result<CompletionOutcome, ScheduleDomainError> {
    Err(wait_failure_completion(&error))
}

fn completion_wait_failure_reason(error: &CompletionWaitError) -> DeliveryCompletionFailureReason {
    use meerkat_runtime::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation;
    match error.wait_failure_observation() {
        Some(RuntimeCompletionWaitFailureObservation::ChannelClosed) => {
            DeliveryCompletionFailureReason::RuntimeCompletionChannelClosed
        }
        Some(RuntimeCompletionWaitFailureObservation::AuthorityUnavailable) => {
            DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
        }
        // A kept input the host cannot read past: this delivery holds no
        // authority over the outcome the next start gives it.
        Some(RuntimeCompletionWaitFailureObservation::RequeuedAcrossStop) => {
            DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
        }
        // A disclosure refusal is no wait failure. It is only reached for an
        // observation that carries no payload, which a refusal never has.
        None => DeliveryCompletionFailureReason::CompletionFutureFailed,
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

/// The session an earlier fire bound to a materialize-on-demand target.
fn bound_target_session_id(target: &TargetBinding) -> Option<SessionId> {
    let TargetBinding::Session(binding) = target else {
        return None;
    };
    match binding.as_ref() {
        SessionTargetBinding::MaterializeOnDemandSession {
            bound_session_id, ..
        } => bound_session_id.clone(),
        SessionTargetBinding::ExactSession { .. }
        | SessionTargetBinding::ResumableSession { .. } => None,
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

/// One answer of the runtime's exact terminal-completion read for an admitted
/// input ([`SessionServiceRuntimeExt::input_terminal_completion`]).
type AdmittedOutcomeAnswer = Result<Option<CompletionOutcome>, RuntimeDriverError>;

/// One read of an admitted input's terminal outcome. Browser builds run no
/// schedule driver, and their runtime reads are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
type AdmittedOutcomeReadFuture = Pin<Box<dyn Future<Output = AdmittedOutcomeAnswer> + Send>>;
#[cfg(target_arch = "wasm32")]
type AdmittedOutcomeReadFuture = Pin<Box<dyn Future<Output = AdmittedOutcomeAnswer>>>;

/// The read the bounded re-read loop calls for each round: the runtime's
/// read in production, a scripted one in tests.
type AdmittedOutcomeReadFn = Box<dyn FnMut() -> AdmittedOutcomeReadFuture + Send>;

/// One wait on an admitted input's live completion: the runtime handle's
/// [`CompletionHandle::try_wait`] in production, a scripted wait in tests.
type AdmittedCompletionWait =
    Pin<Box<dyn Future<Output = Result<CompletionOutcome, CompletionWaitError>> + Send>>;

/// Whether the generated wait-failure authority resolves a failed live wait
/// as resumable: the input stays admitted and its terminal is still to come.
#[cfg(not(target_arch = "wasm32"))]
type AdmittedWaitResumableFuture = Pin<Box<dyn Future<Output = bool> + Send>>;
#[cfg(target_arch = "wasm32")]
type AdmittedWaitResumableFuture = Pin<Box<dyn Future<Output = bool>>>;

/// The resolution [`completion_after_wait_failure`] asks: the runtime's
/// generated wait-failure authority for the input's session
/// ([`runtime_wait_failure_resumable`]).
type AdmittedWaitResumableFn =
    Box<dyn Fn(CompletionWaitError) -> AdmittedWaitResumableFuture + Send>;

/// How to read one admitted input's own terminal past a live wait that
/// failed without saying it: the input, the bounds of the wait and the read.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct AdmittedInputOutcomeRead {
    input_id: InputId,
    bounds: AdmittedOutcomeRereadBounds,
    read: AdmittedOutcomeReadFn,
    resumable: AdmittedWaitResumableFn,
}

#[cfg(not(target_arch = "wasm32"))]
impl AdmittedInputOutcomeRead {
    /// The bounded re-read loop's input for a wait that failed with
    /// `first_cause`. The overall bound counts from now.
    fn into_reread(self, first_cause: String) -> AdmittedOutcomeReread {
        AdmittedOutcomeReread {
            input_id: self.input_id,
            started: meerkat_core::time_compat::Instant::now(),
            first_cause,
            bounds: self.bounds,
            read: self.read,
        }
    }
}

/// The bounds of one wait for an admitted input's readable outcome.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdmittedOutcomeRereadBounds {
    /// The whole wait, from the first read on.
    overall: Duration,
    /// One read; a read that hits it counts as not readable yet.
    per_read: Duration,
    /// The first pause between reads, doubling up to `per_read`.
    first_backoff: Duration,
}

impl AdmittedOutcomeRereadBounds {
    const HOST: Self = Self {
        overall: ADMITTED_OUTCOME_REREAD_BOUND,
        per_read: ADMITTED_OUTCOME_READ_BOUND,
        first_backoff: ADMITTED_OUTCOME_REREAD_FIRST_BACKOFF,
    };

    #[cfg(test)]
    const TEST: Self = Self {
        overall: TEST_ADMITTED_OUTCOME_REREAD_BOUND,
        per_read: TEST_ADMITTED_OUTCOME_READ_BOUND,
        first_backoff: TEST_ADMITTED_OUTCOME_REREAD_FIRST_BACKOFF,
    };

    /// The bounds every runtime-backed schedule host uses. The in-crate test
    /// profile shortens them, as the host's tick pacing does.
    fn for_host() -> Self {
        #[cfg(test)]
        {
            Self::TEST
        }
        #[cfg(not(test))]
        {
            Self::HOST
        }
    }
}

/// What one bounded read of an admitted input's terminal outcome said.
#[derive(Debug)]
enum AdmittedOutcomeRead {
    /// The input's own exact terminal completion.
    Readable(CompletionOutcome),
    /// Not readable yet, with its cause: no finalized receipt yet, a store
    /// read failure, or a read that hit its bound.
    NotYetReadable(String),
    /// A typed answer no later read changes: the input ended without a
    /// public completion, its receipt cannot be read, or the runtime does not
    /// know it.
    Unavailable(String),
}

/// Classify one answer of the exact terminal-completion read. `Ok(None)` and
/// `Internal` are not readable yet: the first means no finalized receipt
/// yet, and the second is a store read failure, which says nothing about the
/// input. Every other typed error is final.
fn classify_admitted_outcome_answer(
    input_id: &InputId,
    answer: AdmittedOutcomeAnswer,
) -> AdmittedOutcomeRead {
    match answer {
        Ok(Some(outcome)) => AdmittedOutcomeRead::Readable(outcome),
        Ok(None) => AdmittedOutcomeRead::NotYetReadable("no finalized receipt yet".to_string()),
        Err(RuntimeDriverError::Internal(error)) => {
            AdmittedOutcomeRead::NotYetReadable(format!("store read failed: {error}"))
        }
        // A degraded session whose driver a stalled commit still holds: the
        // reload settles that commit, so read again within the bound.
        Err(RuntimeDriverError::InputTerminalNotYetReadable { reason, .. }) => {
            AdmittedOutcomeRead::NotYetReadable(reason)
        }
        Err(error) => AdmittedOutcomeRead::Unavailable(format!(
            "runtime could not provide the exact terminal completion receipt for input {input_id}: {error}"
        )),
    }
}

/// Run one read under `bound`. A read that hits it is not readable yet.
async fn read_admitted_outcome_within(
    input_id: &InputId,
    bound: Duration,
    read: AdmittedOutcomeReadFuture,
) -> AdmittedOutcomeRead {
    match schedule_host_tokio::time::timeout(bound, read).await {
        Ok(answer) => classify_admitted_outcome_answer(input_id, answer),
        Err(_) => {
            AdmittedOutcomeRead::NotYetReadable(format!("the read did not answer within {bound:?}"))
        }
    }
}

/// The input of the bounded re-read loop: the admitted input, when its first
/// read started and the cause that read gave, the bounds, and the read itself.
#[cfg(not(target_arch = "wasm32"))]
struct AdmittedOutcomeReread {
    input_id: InputId,
    started: meerkat_core::time_compat::Instant,
    first_cause: String,
    bounds: AdmittedOutcomeRereadBounds,
    read: AdmittedOutcomeReadFn,
}

/// The one bounded re-read loop for an admitted input whose terminal outcome
/// cannot be read yet. It runs as the delivery's completion, so the driver's
/// waiter keeps renewing the claim's lease through it: the occurrence stays
/// AwaitingCompletion, its attempt count does not change, and nothing
/// reclaims it. Each round backs off (doubling from the first backoff up to
/// the per-read bound) and reads again under the per-read bound:
/// - the input's exact terminal completion ends the loop with it;
/// - a final typed answer ends it with the typed terminal at once;
/// - not readable yet reads again.
///
/// At the overall bound it ends delivery_failed with
/// runtime_completion_authority_unavailable, naming the bound and the last
/// cause. It never gives the occurrence up for a reclaim. Nothing in the
/// loop reloads a degraded session: a reload comes from the session's own
/// traffic, and without one the bound is the only end.
#[cfg(not(target_arch = "wasm32"))]
async fn reread_admitted_input_outcome(
    reread: AdmittedOutcomeReread,
) -> Result<CompletionOutcome, ScheduleDomainError> {
    let AdmittedOutcomeReread {
        input_id,
        started,
        first_cause,
        bounds,
        mut read,
    } = reread;
    let mut cause = first_cause;
    let mut backoff = bounds.first_backoff;
    loop {
        let remaining = bounds.overall.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        schedule_host_tokio::time::sleep(backoff.min(remaining)).await;
        let remaining = bounds.overall.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        match read_admitted_outcome_within(&input_id, bounds.per_read.min(remaining), read()).await
        {
            AdmittedOutcomeRead::Readable(outcome) => return Ok(outcome),
            AdmittedOutcomeRead::Unavailable(detail) => {
                return Err(authority_unavailable_completion(detail));
            }
            AdmittedOutcomeRead::NotYetReadable(next) => cause = next,
        }
        backoff = backoff.saturating_mul(2).min(bounds.per_read);
    }
    Err(authority_unavailable_completion(format!(
        "input {input_id} terminal unreadable for {:?}: {cause}",
        bounds.overall
    )))
}

/// Read an admitted input's terminal outcome once, under the per-read bound,
/// and map the answer to the completion the driver waits for. This read runs
/// inside the delivery, before the waiter starts renewing the claim, and its
/// bound keeps it inside the claim's lease.
/// - The input's exact terminal completion: the occurrence resolves from it.
/// - Not readable yet: the completion enters the bounded re-read loop
///   ([`reread_admitted_input_outcome`]), under the waiter's lease renewal.
/// - Any other typed answer: delivery_failed with
///   runtime_completion_authority_unavailable at once.
async fn admitted_input_completion(
    input_id: InputId,
    bounds: AdmittedOutcomeRereadBounds,
    mut read: AdmittedOutcomeReadFn,
) -> AcceptedScheduledInputCompletion {
    #[cfg(not(target_arch = "wasm32"))]
    let started = meerkat_core::time_compat::Instant::now();
    match read_admitted_outcome_within(&input_id, bounds.per_read, read()).await {
        AdmittedOutcomeRead::Readable(outcome) => {
            AcceptedScheduledInputCompletion::Terminal(outcome)
        }
        AdmittedOutcomeRead::Unavailable(detail) => {
            AcceptedScheduledInputCompletion::AuthorityUnavailable { detail }
        }
        #[cfg(not(target_arch = "wasm32"))]
        AdmittedOutcomeRead::NotYetReadable(first_cause) => {
            AcceptedScheduledInputCompletion::Reread(AdmittedOutcomeReread {
                input_id,
                started,
                first_cause,
                bounds,
                read,
            })
        }
        // Browser builds run no schedule driver, so no lease is renewed
        // across a re-read there.
        #[cfg(target_arch = "wasm32")]
        AdmittedOutcomeRead::NotYetReadable(cause) => {
            let _ = read;
            AcceptedScheduledInputCompletion::AuthorityUnavailable {
                detail: format!("input {input_id} terminal unreadable: {cause}"),
            }
        }
    }
}

/// The runtime's exact terminal-completion read of one admitted input, as
/// [`admitted_input_completion`] and the re-read loop call it.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn runtime_admitted_outcome_read(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
) -> AdmittedOutcomeReadFn {
    let runtime_adapter = runtime_adapter.clone();
    let session_id = session_id.clone();
    let input_id = input_id.clone();
    Box::new(move || {
        let runtime_adapter = runtime_adapter.clone();
        let session_id = session_id.clone();
        let input_id = input_id.clone();
        Box::pin(async move {
            runtime_adapter
                .input_terminal_completion(&session_id, &input_id)
                .await
        })
    })
}

/// The generated wait-failure resolution of `session_id`'s failed waits, as
/// [`completion_after_wait_failure`] asks it. A resolution that fails says
/// not resumable.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn runtime_wait_failure_resumable(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
) -> AdmittedWaitResumableFn {
    let runtime_adapter = runtime_adapter.clone();
    let session_id = session_id.clone();
    Box::new(move |error| {
        let runtime_adapter = runtime_adapter.clone();
        let session_id = session_id.clone();
        Box::pin(async move {
            runtime_adapter
                .resolve_runtime_completion_wait_failure(&session_id, &error)
                .await
                .is_ok_and(|authority| authority.resumable)
        })
    })
}

/// Project one runtime admission into the schedule driver's typed delivery
/// contract.
///
/// Runtime returns no live completion handle when an accepted or
/// deduplicated input is already terminal at admission, or when no
/// completion is registered for it. Admission is proven either way. This
/// single adapter path reads the machine-authorized exact rich completion
/// receipt and replays it, so every schedule surface preserves the same
/// result class and payload; a coarse input-terminal class is never treated
/// as completion authority. The occurrence's terminal comes only from that
/// input's own outcome:
/// - the exact receipt: the occurrence resolves from it;
/// - `Ok(None)`, no finalized receipt yet: the input's receipt batch is
///   staged and not finalized yet (a session whose durability degraded keeps
///   it so until its reload), or the input has left the runtime (cold
///   recovery discarded its row, or a store-less runtime archived it). Like
///   a store read failure (`Internal`) or a read that hits its bound, this
///   is not readable yet: the completion re-reads the outcome while the
///   driver keeps the claim's lease, for up to five lease periods, then ends
///   delivery_failed with runtime_completion_authority_unavailable. An input
///   that left the runtime ends so at the bound, with no second admission;
/// - any other typed error (the input ended without a public completion,
///   its receipt cannot be read, the runtime does not know it):
///   delivery_failed with runtime_completion_authority_unavailable at once.
///
/// With a live completion handle the occurrence resolves from the handle's
/// outcome. A wait on it that fails with a rolled-back boundary or an
/// unknown durable outcome leaves the input admitted with its terminal still
/// to come, so the completion reads that terminal through the same bounded
/// loop instead of recording the wait failure.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
        Some(handle) => {
            // A wait that fails with a rolled-back boundary, an unknown
            // durable outcome or a failure the generated authority resolves
            // as resumable reads this input's own terminal.
            let read = runtime_admitted_outcome_read(runtime_adapter, session_id, &input_id);
            AcceptedScheduledInput {
                correlation_id: correlation_id.clone(),
                admission_outcome: ScheduleAdmissionOutcome::Accepted,
                completion: AcceptedScheduledInputCompletion::Handle {
                    wait: Box::pin(handle.try_wait()),
                    outcome_read: Some(AdmittedInputOutcomeRead {
                        input_id,
                        bounds: AdmittedOutcomeRereadBounds::for_host(),
                        read,
                        resumable: runtime_wait_failure_resumable(runtime_adapter, session_id),
                    }),
                },
            }
        }
        None => {
            let read = runtime_admitted_outcome_read(runtime_adapter, session_id, &input_id);
            AcceptedScheduledInput {
                correlation_id: correlation_id.clone(),
                admission_outcome: ScheduleAdmissionOutcome::Accepted,
                completion: admitted_input_completion(
                    input_id,
                    AdmittedOutcomeRereadBounds::for_host(),
                    read,
                )
                .await,
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

/// Admit one scheduled runtime input and reconcile the ambiguous error seam.
///
/// Persistent runtime admission can fail after the idempotency binding and
/// input row committed (for example while archiving terminal in-memory
/// carriers). An ordinary adapter error would falsely certify that no target
/// effect was admitted and make the Schedule driver terminalize the durable
/// `DispatchStarted` intent. This helper first re-drives the exact same input,
/// then consults the store-owned idempotency index. Only an authoritative miss
/// may return an ordinary pre-admission error; a committed binding or an
/// unavailable reconciliation read remains reclaimable as
/// `DeliveryRepairDeferred`.
///
/// `accept` must admit without an attachment or member-residency fence; a
/// host whose admission is fenced uses
/// [`accept_fenced_schedule_runtime_input_with_reconciliation`], so that a
/// redelivery's existing-only admission keeps the same fence.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub async fn accept_schedule_runtime_input_with_reconciliation<F, Fut, E>(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    input: meerkat_runtime::Input,
    materialized_session_id: Option<SessionId>,
    accept: F,
) -> Result<DeliveryDispatch, ScheduleDomainError>
where
    F: FnMut(meerkat_runtime::Input) -> Fut,
    Fut: std::future::Future<
            Output = Result<
                (
                    meerkat_runtime::AcceptOutcome,
                    Option<meerkat_runtime::CompletionHandle>,
                ),
                E,
            >,
        >,
    E: std::fmt::Display,
{
    accept_fenced_schedule_runtime_input_with_reconciliation(
        runtime_adapter,
        session_id,
        occurrence,
        identity,
        input,
        materialized_session_id,
        ScheduleAdmissionFence::Unfenced,
        accept,
    )
    .await
}

/// The fence a session host admits a scheduled input under. A redelivery's
/// existing-only admission runs under the same fence as the host's ordinary
/// admission ([`accept_fenced_schedule_runtime_input_with_reconciliation`]).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum ScheduleAdmissionFence<'a> {
    /// The runtime's admission with no attachment or member-residency fence.
    Unfenced,
    /// A mob member's session, admitted as
    /// [`MeerkatMachine::accept_input_with_completion_for_attachment_and_member_residency`]
    /// admits: only through this exact executor attachment, and only while
    /// the member's residency is `expected_member` (`None` is a peer-only
    /// member, never a vacant placement).
    AttachmentAndMemberResidency {
        attachment: &'a meerkat_runtime::RuntimeExecutorAttachmentWitness,
        expected_member:
            Option<&'a meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation>,
    },
}

/// [`accept_schedule_runtime_input_with_reconciliation`] for a host whose
/// `accept` admits under `fence`. A redelivery's existing-only admission
/// runs under the same fence, so it never answers from an attachment or a
/// member incarnation the host's ordinary admission would refuse; a fence
/// refusal there says nothing about the earlier admission and leaves the
/// occurrence reclaimable, as any other runtime error does.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[allow(clippy::too_many_arguments)]
pub async fn accept_fenced_schedule_runtime_input_with_reconciliation<F, Fut, E>(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    input: meerkat_runtime::Input,
    materialized_session_id: Option<SessionId>,
    fence: ScheduleAdmissionFence<'_>,
    mut accept: F,
) -> Result<DeliveryDispatch, ScheduleDomainError>
where
    F: FnMut(meerkat_runtime::Input) -> Fut,
    Fut: std::future::Future<
            Output = Result<
                (
                    meerkat_runtime::AcceptOutcome,
                    Option<meerkat_runtime::CompletionHandle>,
                ),
                E,
            >,
        >,
    E: std::fmt::Display,
{
    let idempotency_key = input
        .header()
        .idempotency_key
        .as_ref()
        .map(|key| key.0.clone())
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            ScheduleDomainError::Internal(
                "scheduled runtime input omitted its stable idempotency key".to_string(),
            )
        })?;
    // A redelivery of an occurrence whose earlier attempt recorded its
    // dispatch admits only as a replay of that attempt's input.
    #[cfg(not(target_arch = "wasm32"))]
    let existing = existing_admission_scope();
    #[cfg(target_arch = "wasm32")]
    let existing: Option<std::convert::Infallible> = None;
    if let Some(existing) = existing {
        #[cfg(not(target_arch = "wasm32"))]
        existing
            .consumed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        return accept_existing_schedule_runtime_input(
            runtime_adapter,
            session_id,
            occurrence,
            identity,
            input,
            materialized_session_id,
            existing,
            fence,
        )
        .await;
    }
    let redrive = input.clone();
    let redrive_input_id = redrive.id().clone();
    match accept(input).await {
        Ok((outcome, handle)) => {
            return runtime_delivery_dispatch_from_admission(
                runtime_adapter,
                session_id,
                occurrence,
                identity,
                outcome,
                handle,
                materialized_session_id,
            )
            .await;
        }
        Err(first_error) => match accept(redrive).await {
            Ok((
                outcome @ (meerkat_runtime::AcceptOutcome::Accepted { .. }
                | meerkat_runtime::AcceptOutcome::Deduplicated { .. }),
                handle,
            )) => {
                runtime_delivery_dispatch_from_admission(
                    runtime_adapter,
                    session_id,
                    occurrence,
                    identity,
                    outcome,
                    handle,
                    materialized_session_id,
                )
                .await
            }
            Ok((outcome, _)) => {
                let redrive_detail = format!("runtime returned {outcome:?} on exact redrive");
                reconcile_schedule_runtime_admission_binding(
                    runtime_adapter,
                    session_id,
                    occurrence,
                    identity,
                    redrive_input_id,
                    &idempotency_key,
                    materialized_session_id,
                    first_error.to_string(),
                    redrive_detail,
                    Some(outcome),
                )
                .await
            }
            Err(redrive_error) => {
                reconcile_schedule_runtime_admission_binding(
                    runtime_adapter,
                    session_id,
                    occurrence,
                    identity,
                    redrive_input_id,
                    &idempotency_key,
                    materialized_session_id,
                    first_error.to_string(),
                    redrive_error.to_string(),
                    None,
                )
                .await
            }
        },
    }
}

/// Admit a redelivery through the runtime's deduplicate-only admission,
/// which decides it where it decides deduplication: the earlier input's
/// replay, or [`RuntimeDriverError::NoExistingAdmission`] with nothing
/// admitted. No admission is retried or reconciled here, since none can be
/// fresh.
/// - The replay: the occurrence waits for that input's own outcome.
/// - No earlier admission of an admitted redelivery (recovery discarded the
///   row, or a store-less runtime archived it): delivery_failed, nothing
///   admitted.
/// - No earlier admission of an unproven late redelivery: the adapter answers
///   that the target never admitted it, and the occurrence ends misfired.
/// - Any other error says nothing about the earlier admission: the
///   occurrence stays reclaimable (`DeliveryRepairDeferred`). A refusal of
///   the host's `fence` is one: the lookup did not run, and the next attempt
///   takes the host's attachment and member residency afresh.
///
/// The admission runs under the host's `fence`, as its ordinary admission
/// does.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
async fn accept_existing_schedule_runtime_input(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    input: meerkat_runtime::Input,
    materialized_session_id: Option<SessionId>,
    scope: ExistingAdmissionScope,
    fence: ScheduleAdmissionFence<'_>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    let admitted = match fence {
        ScheduleAdmissionFence::Unfenced => {
            runtime_adapter
                .accept_existing_input_with_completion(session_id, input)
                .await
        }
        ScheduleAdmissionFence::AttachmentAndMemberResidency {
            attachment,
            expected_member,
        } => {
            runtime_adapter
                .accept_existing_input_with_completion_for_attachment_and_member_residency(
                    attachment,
                    input,
                    expected_member,
                )
                .await
        }
    };
    match admitted {
        Ok((outcome, handle)) => {
            runtime_delivery_dispatch_from_admission(
                runtime_adapter,
                session_id,
                occurrence,
                identity,
                outcome,
                handle,
                materialized_session_id,
            )
            .await
        }
        Err(RuntimeDriverError::NoExistingAdmission { key }) => match scope.existing {
            ExistingDelivery::Admitted => Ok(immediate_delivery_failure(
                occurrence,
                format!(
                    "the admitted input is no longer known to the runtime (idempotency key {key}); nothing was admitted"
                ),
                DeliveryFailureReason::RuntimeRejected,
                Some(identity.correlation_id.clone()),
                materialized_session_id,
            )),
            _ => {
                scope
                    .not_admitted
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Err(ScheduleDomainError::DeliveryFailed {
                    occurrence_id: occurrence.occurrence_id.clone(),
                    reason: format!(
                        "the runtime never admitted idempotency key {key}; nothing was admitted"
                    ),
                })
            }
        },
        Err(error) => Err(ScheduleDomainError::DeliveryRepairDeferred {
            detail: format!("the redelivery could not read its earlier admission: {error}"),
        }),
    }
}

/// Browser builds run no schedule driver, so nothing redelivers there.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
async fn accept_existing_schedule_runtime_input(
    _runtime_adapter: &MeerkatMachine,
    _session_id: &SessionId,
    _occurrence: &Occurrence,
    _identity: &ScheduleDeliveryIdentity,
    _input: meerkat_runtime::Input,
    _materialized_session_id: Option<SessionId>,
    scope: std::convert::Infallible,
    _fence: ScheduleAdmissionFence<'_>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    match scope {}
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_schedule_runtime_admission_binding(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    redrive_input_id: meerkat_core::lifecycle::InputId,
    idempotency_key: &str,
    materialized_session_id: Option<SessionId>,
    first_error: String,
    redrive_detail: String,
    no_binding_outcome: Option<meerkat_runtime::AcceptOutcome>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    match runtime_adapter
        .input_state_by_idempotency_key(session_id, idempotency_key)
        .await
    {
        Ok(Some(stored)) => {
            let existing_id = stored.state.input_id.clone();
            tracing::warn!(
                %idempotency_key,
                %existing_id,
                %first_error,
                %redrive_detail,
                "recovered scheduled runtime admission from durable idempotency binding"
            );
            runtime_delivery_dispatch_from_admission(
                runtime_adapter,
                session_id,
                occurrence,
                identity,
                meerkat_runtime::AcceptOutcome::Deduplicated {
                    input_id: redrive_input_id,
                    existing_id,
                    existing_seed: stored.seed,
                },
                None,
                materialized_session_id,
            )
            .await
        }
        Ok(None) => match no_binding_outcome {
            Some(outcome) => {
                runtime_delivery_dispatch_from_admission(
                    runtime_adapter,
                    session_id,
                    occurrence,
                    identity,
                    outcome,
                    None,
                    materialized_session_id,
                )
                .await
            }
            None => Err(ScheduleDomainError::Internal(format!(
                "runtime admission failed before any durable binding for stable key {idempotency_key} ({first_error}; redrive: {redrive_detail})"
            ))),
        },
        Err(reconciliation_error) => Err(ScheduleDomainError::DeliveryRepairDeferred {
            detail: format!(
                "runtime admission outcome for stable key {idempotency_key} is unknown after errors ({first_error}; redrive: {redrive_detail}); durable reconciliation failed: {reconciliation_error}"
            ),
        }),
    }
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

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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

    #[tokio::test]
    async fn unpolled_schedule_host_shutdown_stops_blocked_supervisor() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TaskDrop(Arc<AtomicBool>);

        impl Drop for TaskDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let task_dropped = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let join = schedule_host_tokio::task::spawn({
            let task_dropped = Arc::clone(&task_dropped);
            async move {
                let _drop = TaskDrop(task_dropped);
                let _ = started_tx.send(());
                let _ = shutdown_rx.await;
            }
        });
        started_rx.await.expect("blocked supervisor should start");
        let handle = ScheduleHostHandle {
            shutdown_tx: Some(shutdown_tx),
            worker_shutdown: ScheduleHostWorkerShutdown::default(),
            join: Some(join),
        };

        let shutdown = handle.shutdown();
        drop(shutdown);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !task_dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping an unpolled shutdown must stop the supervisor");
    }

    #[tokio::test]
    async fn cancelling_polled_shutdown_stops_worker_installed_after_signal() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TaskDrop(Arc<AtomicBool>);

        impl Drop for TaskDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let worker_dropped = Arc::new(AtomicBool::new(false));
        let worker_shutdown = ScheduleHostWorkerShutdown::default();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (shutdown_observed_tx, shutdown_observed_rx) = oneshot::channel();
        let (cleanup_tx, cleanup_rx) = oneshot::channel();
        let (cleanup_complete_tx, cleanup_complete_rx) = oneshot::channel();
        let join = schedule_host_tokio::task::spawn({
            let worker_dropped = Arc::clone(&worker_dropped);
            let worker_shutdown = worker_shutdown.clone();
            async move {
                let _ = shutdown_rx.await;
                let mut worker_shutdown_rx = worker_shutdown.subscribe();
                {
                    let _drop = TaskDrop(worker_dropped);
                    wait_for_worker_shutdown(&mut worker_shutdown_rx).await;
                }
                let _ = shutdown_observed_tx.send(());
                let _ = cleanup_rx.await;
                let _ = cleanup_complete_tx.send(());
            }
        });
        let handle = ScheduleHostHandle {
            shutdown_tx: Some(shutdown_tx),
            worker_shutdown,
            join: Some(join),
        };

        let shutdown_task = schedule_host_tokio::task::spawn(handle.shutdown());
        tokio::time::timeout(Duration::from_secs(1), shutdown_observed_rx)
            .await
            .expect("shutdown should reach the supervisor")
            .expect("supervisor should install the in-flight worker");
        shutdown_task.abort();
        let _ = shutdown_task.await;

        assert!(worker_dropped.load(Ordering::Acquire));
        let _ = cleanup_tx.send(());
        tokio::time::timeout(Duration::from_secs(1), cleanup_complete_rx)
            .await
            .expect("detached supervisor cleanup should finish")
            .expect("supervisor should report cleanup completion");
    }

    #[tokio::test]
    async fn shutdown_cancels_pending_next_action_read() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct OperationDrop(Arc<AtomicBool>);

        impl Drop for OperationDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let operation_dropped = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (started_tx, started_rx) = oneshot::channel();
        let wait_task = schedule_host_tokio::task::spawn({
            let operation_dropped = Arc::clone(&operation_dropped);
            async move {
                wait_for_worker_shutdown_or(&mut shutdown_rx, async move {
                    let _drop = OperationDrop(operation_dropped);
                    let _ = started_tx.send(());
                    std::future::pending::<()>().await;
                })
                .await
            }
        });
        started_rx
            .await
            .expect("pending next-action read should start");

        shutdown_tx.send_replace(true);
        let result = tokio::time::timeout(Duration::from_secs(1), wait_task)
            .await
            .expect("shutdown should stop the pending next-action read")
            .expect("wait task should not panic");
        assert!(result.is_none());
        assert!(operation_dropped.load(Ordering::Acquire));
    }

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

            async fn acquire_executor_lease(
                &self,
                request: meerkat_schedule::AcquireScheduleExecutorLeaseRequest,
            ) -> Result<
                meerkat_schedule::AcquireScheduleExecutorLeaseOutcome,
                meerkat_schedule::ScheduleStoreError,
            > {
                let now = chrono::Utc::now();
                Ok(
                    meerkat_schedule::AcquireScheduleExecutorLeaseOutcome::Acquired(
                        meerkat_schedule::ScheduleExecutorLease::from_store_commit(
                            request.owner_id,
                            uuid::Uuid::now_v7(),
                            1,
                            now,
                            now + request.lease_duration,
                        ),
                    ),
                )
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
            ) -> Result<
                meerkat_schedule::ScheduleTransitionCommit,
                meerkat_schedule::ScheduleStoreError,
            > {
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
            ) -> Result<
                meerkat_schedule::OccurrenceTransitionCommit,
                meerkat_schedule::ScheduleStoreError,
            > {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_occurrence_writes(
                &self,
                _writes: Vec<meerkat_schedule::AuthorizedOccurrenceWrite>,
            ) -> Result<
                Vec<meerkat_schedule::OccurrenceTransitionCommit>,
                meerkat_schedule::ScheduleStoreError,
            > {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_schedule_mutation(
                &self,
                _schedule: meerkat_schedule::AuthorizedScheduleWrite,
                _occurrences: Vec<meerkat_schedule::AuthorizedOccurrenceWrite>,
            ) -> Result<
                meerkat_schedule::ScheduleMutationCommit,
                meerkat_schedule::ScheduleStoreError,
            > {
                Err(meerkat_schedule::ScheduleStoreError::Internal(
                    "synthetic store outage".to_string(),
                ))
            }

            async fn commit_schedule_refill(
                &self,
                _schedule: meerkat_schedule::AuthorizedScheduleWrite,
                _occurrences: Vec<meerkat_schedule::AuthorizedOccurrenceWrite>,
                _next_refill_at_utc: Option<chrono::DateTime<chrono::Utc>>,
            ) -> Result<
                meerkat_schedule::ScheduleMutationCommit,
                meerkat_schedule::ScheduleStoreError,
            > {
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
                _lease: &meerkat_schedule::ScheduleExecutorLease,
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
                Option<meerkat_schedule::OccurrenceTransitionCommit>,
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
            ) -> Result<
                Option<meerkat_schedule::OccurrenceTransitionCommit>,
                meerkat_schedule::ScheduleStoreError,
            > {
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
                dedupe_key: None,
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

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A session host that must never be asked to materialize. Any
    /// `materialize_session` call records a hit and returns an error so the
    /// Layer B reuse guard regression is caught as a test failure rather than
    /// a silent duplicate session.
    struct PanicOnMaterializeHost {
        materialize_calls: Arc<AtomicUsize>,
    }

    struct BlockingProbeHost {
        entered: tokio::sync::Notify,
        probe_calls: Arc<AtomicUsize>,
        delivery_calls: Arc<AtomicUsize>,
        probe_future_dropped: Arc<AtomicBool>,
    }

    struct ProbeFutureDrop(Arc<AtomicBool>);

    impl Drop for ProbeFutureDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
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
            _dispatch: ScheduledEventDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for BlockingProbeHost {
        async fn probe_session_target(
            &self,
            _binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            self.probe_calls.fetch_add(1, Ordering::AcqRel);
            let _drop = ProbeFutureDrop(Arc::clone(&self.probe_future_dropped));
            self.entered.notify_one();
            std::future::pending().await
        }

        async fn materialize_session(
            &self,
            _occurrence: &Occurrence,
            _create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            panic!("blocked probe must prevent materialization")
        }

        async fn deliver_prompt(
            &self,
            _session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            _dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.delivery_calls.fetch_add(1, Ordering::AcqRel);
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
            _dispatch: ScheduledEventDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.delivery_calls.fetch_add(1, Ordering::AcqRel);
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }
    }

    #[tokio::test]
    async fn dropping_real_schedule_host_stops_tick_releases_lease_and_prevents_delivery() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(Arc::clone(&store));
        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("abort-safe-real-host".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() - ChronoDuration::seconds(1),
                },
                target: TargetBinding::session(SessionTargetBinding::ExactSession {
                    session_id: SessionId::new(),
                    action: ScheduledSessionAction::Prompt {
                        prompt: ContentInput::Text("must not outlive shutdown".to_string()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                }),
                misfire_policy: meerkat_schedule::MisfirePolicy::CatchUpWithin {
                    window_seconds: 60,
                },
                overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("schedule create should plan the due occurrence");

        let probe_calls = Arc::new(AtomicUsize::new(0));
        let delivery_calls = Arc::new(AtomicUsize::new(0));
        let probe_future_dropped = Arc::new(AtomicBool::new(false));
        let blocking_host = Arc::new(BlockingProbeHost {
            entered: tokio::sync::Notify::new(),
            probe_calls: Arc::clone(&probe_calls),
            delivery_calls: Arc::clone(&delivery_calls),
            probe_future_dropped: Arc::clone(&probe_future_dropped),
        });
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = blocking_host.clone();
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = Arc::new(SharedScheduleTargetAdapter::new(
            service.clone(),
            session_host,
            mob_host,
        ));
        let handle = spawn_schedule_host(service.clone(), adapter, "abort-safe-owner");

        tokio::time::timeout(Duration::from_secs(1), blocking_host.entered.notified())
            .await
            .expect("real driver should block inside target probe");
        assert_eq!(probe_calls.load(Ordering::Acquire), 1);
        assert_eq!(delivery_calls.load(Ordering::Acquire), 0);

        let shutdown = handle.shutdown();
        drop(shutdown);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !probe_future_dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping shutdown must stop the real blocked tick");

        let replacement_lease = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let request = meerkat_schedule::AcquireScheduleExecutorLeaseRequest {
                    owner_id: "replacement-owner".to_string(),
                    lease_duration: ChronoDuration::seconds(60),
                };
                match store
                    .acquire_executor_lease(request)
                    .await
                    .expect("replacement lease acquisition should remain readable")
                {
                    meerkat_schedule::AcquireScheduleExecutorLeaseOutcome::Acquired(lease) => {
                        break lease;
                    }
                    meerkat_schedule::AcquireScheduleExecutorLeaseOutcome::Busy { .. } => {
                        tokio::task::yield_now().await;
                    }
                }
            }
        })
        .await
        .expect("Drop must let the supervisor release its executor lease");
        assert_eq!(replacement_lease.owner_id(), "replacement-owner");
        store
            .release_executor_lease(replacement_lease)
            .await
            .expect("replacement lease should remain releasable");

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(probe_calls.load(Ordering::Acquire), 1);
        assert_eq!(delivery_calls.load(Ordering::Acquire), 0);
        let occurrences = service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("occurrence should remain inspectable");
        assert_eq!(occurrences.len(), 1);
        assert_eq!(occurrences[0].attempt_count, 1);
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
            _dispatch: ScheduledEventDispatch,
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
            _dispatch: ScheduledEventDispatch,
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

    fn identity_create_request(identity: &str) -> meerkat_schedule::CreateScheduleRequest {
        meerkat_schedule::CreateScheduleRequest {
            name: Some("identity-target".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + ChronoDuration::days(1),
            },
            target: TargetBinding::identity(IdentityTargetBinding::resumable(
                identity,
                ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text("check in".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            )),
            misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
            overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        }
    }

    /// A redelivery never materializes a session: an on-demand target that
    /// was never bound, and whose own deterministic session does not exist,
    /// holds no earlier admission. The redelivery probes that session
    /// read-only. An admitted redelivery ends delivery_failed with nothing
    /// admitted, and an unproven late one answers that the target never
    /// admitted it.
    #[tokio::test]
    async fn redelivery_of_an_unbound_on_demand_target_materializes_nothing() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let materialize_calls = Arc::new(AtomicUsize::new(0));
        let probes = Arc::new(AtomicUsize::new(0));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(OwnSessionHost {
            runtime: Arc::new(MeerkatMachine::ephemeral()),
            present: None,
            probes: Arc::clone(&probes),
            materialize_calls: Arc::clone(&materialize_calls),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> =
            Arc::new(NoopScheduleMobHost::new("mob targets unused here"));
        let adapter = SharedScheduleTargetAdapter::new(service.clone(), session_host, mob_host);
        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("redelivery-unbound".to_string()),
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
            .expect("one planned occurrence");
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);

        let admitted = adapter
            .deliver_existing_occurrence(&occurrence, &identity, ExistingDelivery::Admitted)
            .await
            .expect("the admitted redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(dispatch) = admitted else {
            panic!("an admitted redelivery answers with a dispatch: {admitted:?}");
        };
        assert_eq!(
            dispatch.receipt.stage,
            DeliveryReceiptStage::DispatchStarted
        );
        let terminal = dispatch.completion.await.expect("the failure terminal");
        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
        assert!(
            terminal
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no longer known")),
            "{:?}",
            terminal.detail
        );

        let unproven = adapter
            .deliver_existing_occurrence(&occurrence, &identity, ExistingDelivery::UnprovenLate)
            .await
            .expect("the unproven redelivery answers");
        assert!(matches!(
            unproven,
            ExistingDeliveryAnswer::NotAdmittedPastDeadline
        ));
        assert_eq!(
            materialize_calls.load(Ordering::SeqCst),
            0,
            "a redelivery materializes nothing"
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "each redelivery probes the occurrence's own session"
        );
    }

    /// A session host whose sessions are the runtime's: it holds exactly the
    /// session `present` (every other exact session probes missing), decides
    /// a redelivery's existing-only admission, admits through
    /// [`accept_schedule_runtime_input_with_reconciliation`], and never
    /// materializes.
    struct OwnSessionHost {
        runtime: Arc<MeerkatMachine>,
        present: Option<SessionId>,
        probes: Arc<AtomicUsize>,
        materialize_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for OwnSessionHost {
        fn decides_existing_admission(&self) -> bool {
            true
        }

        async fn probe_session_target(
            &self,
            binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            match binding.resolved_session_id() {
                Some(session_id) if self.present.as_ref() != Some(session_id) => {
                    Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("session not found: {session_id}")),
                    })
                }
                _ => Ok(TargetProbeOutcome::Ready),
            }
        }

        async fn materialize_session(
            &self,
            _occurrence: &Occurrence,
            _create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            self.materialize_calls.fetch_add(1, Ordering::SeqCst);
            Err(ScheduleDomainError::Internal(
                "this host never materializes".to_string(),
            ))
        }

        async fn deliver_prompt(
            &self,
            session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let mut prompt =
                meerkat_runtime::PromptInput::from_content_input(dispatch.prompt, None);
            prompt.header.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(
                identity.idempotency_key.clone(),
            ));
            prompt.header.correlation_id = Some(schedule_runtime_correlation_id(identity)?);
            accept_schedule_runtime_input_with_reconciliation(
                &self.runtime,
                session_id,
                occurrence,
                identity,
                meerkat_runtime::Input::Prompt(prompt),
                dispatch.materialized_session_id,
                |input| self.runtime.accept_input_with_completion(session_id, input),
            )
            .await
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            _occurrence: &Occurrence,
            _identity: &ScheduleDeliveryIdentity,
            _dispatch: ScheduledEventDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Err(ScheduleDomainError::Internal(
                "this host delivers prompts only".to_string(),
            ))
        }
    }

    /// The planned occurrences of a new schedule whose target is an unbound
    /// on-demand session, earliest first.
    async fn unbound_on_demand_occurrences(
        service: &ScheduleService,
        name: &str,
        occurrences: u32,
    ) -> Vec<Occurrence> {
        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some(name.to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Interval(
                    meerkat_schedule::IntervalTriggerSpec {
                        start_at_utc: chrono::Utc::now(),
                        every_seconds: 60,
                        end_at_utc: None,
                    },
                ),
                target: materialize_on_demand_target(),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::AllowConcurrent,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(occurrences),
            })
            .await
            .expect("schedule create should plan its occurrences");
        let mut planned = service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("list occurrences");
        planned.sort_by_key(|occurrence| occurrence.due_at_utc);
        assert_eq!(planned.len(), occurrences as usize);
        planned
    }

    /// The prompt an on-demand target's occurrence dispatches, as its first
    /// attempt does before any bind reaches the occurrence.
    fn unbound_prompt_dispatch() -> ScheduledPromptDispatch {
        ScheduledPromptDispatch {
            prompt: ContentInput::Text("scheduled prompt".to_string()),
            transient_turn_context: None,
            system_prompt: None,
            render_metadata: None,
            skill_refs: Vec::new(),
            additional_instructions: Vec::new(),
            materialized_session_id: None,
        }
    }

    /// Constructed state. No sequence at this base leaves an occurrence that
    /// is unbound at its redelivery holding an admission in its own
    /// deterministic session: the ordinary delivery binds what it
    /// materialized before it admits, a failed bind admits nothing, a bind
    /// skipped because the schedule moved to another revision or went away
    /// supersedes the occurrence or fails its reclaim, and the reclaim copies
    /// the schedule's binding onto the occurrence before it redelivers. The
    /// redelivery still looks there, read-only, as a defensive step: a late
    /// one finds the earlier admission instead of misfiring a delivery that
    /// ran, admits nothing new, and reports the session as materialized so
    /// the driver binds and records it. Before, it answered not admitted
    /// without a read.
    #[tokio::test]
    async fn a_late_redelivery_finds_an_earlier_admission_in_the_occurrences_own_session() {
        let service = ScheduleService::new(
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>
        );
        let occurrence = unbound_on_demand_occurrences(&service, "redelivery-own-session", 1)
            .await
            .remove(0);
        let session_id = occurrence.materialized_session_id();
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        runtime
            .register_session(session_id.clone())
            .await
            .expect("register the occurrence's own session");
        let probes = Arc::new(AtomicUsize::new(0));
        let materialize_calls = Arc::new(AtomicUsize::new(0));
        let host = Arc::new(OwnSessionHost {
            runtime: Arc::clone(&runtime),
            present: Some(session_id.clone()),
            probes: Arc::clone(&probes),
            materialize_calls: Arc::clone(&materialize_calls),
        });
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let earlier = host
            .deliver_prompt(
                &session_id,
                &occurrence,
                &identity,
                unbound_prompt_dispatch(),
            )
            .await
            .expect("the earlier attempt admits into the occurrence's own session");
        assert_eq!(
            earlier.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionAccepted)
        );
        let mob_host: Arc<dyn SurfaceScheduleMobHost> =
            Arc::new(NoopScheduleMobHost::new("mob targets unused here"));
        let adapter = SharedScheduleTargetAdapter::new(service, host, mob_host);

        let answer = adapter
            .deliver_existing_occurrence(&occurrence, &identity, ExistingDelivery::UnprovenLate)
            .await
            .expect("the unproven late redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(dispatch) = answer else {
            panic!("the redelivery finds the earlier admission: {answer:?}");
        };
        assert_eq!(
            dispatch.receipt.stage,
            DeliveryReceiptStage::DispatchAccepted
        );
        assert_eq!(
            dispatch.receipt.runtime_outcome,
            Some(meerkat_schedule::RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        assert_eq!(dispatch.materialized_session_id, Some(session_id.clone()));
        assert_eq!(
            dispatch.receipt.materialized_session_id,
            Some(session_id.clone())
        );
        assert_eq!(
            admitted_input_count(&runtime, &session_id).await,
            1,
            "the redelivery admits nothing new"
        );
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        assert_eq!(materialize_calls.load(Ordering::SeqCst), 0);
    }

    /// The occurrence's own session exists but holds no admission of it: an
    /// admitted redelivery ends delivery_failed and an unproven late one
    /// answers not admitted, nothing is admitted, and neither reports the
    /// session, so the driver binds nothing.
    #[tokio::test]
    async fn a_redelivery_into_its_own_session_without_the_admission_binds_nothing() {
        let service = ScheduleService::new(
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>
        );
        let occurrence = unbound_on_demand_occurrences(&service, "redelivery-own-empty", 1)
            .await
            .remove(0);
        let session_id = occurrence.materialized_session_id();
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        runtime
            .register_session(session_id.clone())
            .await
            .expect("register the occurrence's own session");
        let host = Arc::new(OwnSessionHost {
            runtime: Arc::clone(&runtime),
            present: Some(session_id.clone()),
            probes: Arc::new(AtomicUsize::new(0)),
            materialize_calls: Arc::new(AtomicUsize::new(0)),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> =
            Arc::new(NoopScheduleMobHost::new("mob targets unused here"));
        let adapter = SharedScheduleTargetAdapter::new(service, host, mob_host);
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);

        let admitted = adapter
            .deliver_existing_occurrence(&occurrence, &identity, ExistingDelivery::Admitted)
            .await
            .expect("the admitted redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(dispatch) = admitted else {
            panic!("an admitted redelivery answers with a dispatch: {admitted:?}");
        };
        assert_eq!(dispatch.materialized_session_id, None);
        assert_eq!(dispatch.receipt.materialized_session_id, None);
        let terminal = dispatch.completion.await.expect("the failure terminal");
        assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);

        let unproven = adapter
            .deliver_existing_occurrence(&occurrence, &identity, ExistingDelivery::UnprovenLate)
            .await
            .expect("the unproven redelivery answers");
        assert!(
            matches!(unproven, ExistingDeliveryAnswer::NotAdmittedPastDeadline),
            "{unproven:?}"
        );
        assert_eq!(admitted_input_count(&runtime, &session_id).await, 0);
    }

    /// The binding a redelivery's found admission leads to is the ordinary
    /// one: the driver binds the reported session for the schedule and its
    /// pending occurrences (the same bind as an ordinary dispatch), so a
    /// later ordinary occurrence delivers into it and materializes nothing;
    /// and that bind never overwrites a binding another occurrence committed
    /// first.
    #[tokio::test]
    async fn a_found_admission_binds_its_session_for_later_occurrences_and_never_rebinds() {
        let service = ScheduleService::new(
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>
        );
        let planned = unbound_on_demand_occurrences(&service, "redelivery-binds", 2).await;
        let (first, later) = (&planned[0], &planned[1]);
        let session_id = first.materialized_session_id();
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        runtime
            .register_session(session_id.clone())
            .await
            .expect("register the first occurrence's own session");
        let materialize_calls = Arc::new(AtomicUsize::new(0));
        let host = Arc::new(OwnSessionHost {
            runtime: Arc::clone(&runtime),
            present: Some(session_id.clone()),
            probes: Arc::new(AtomicUsize::new(0)),
            materialize_calls: Arc::clone(&materialize_calls),
        });
        let first_identity = ScheduleDeliveryIdentity::for_occurrence(first);
        host.deliver_prompt(
            &session_id,
            first,
            &first_identity,
            unbound_prompt_dispatch(),
        )
        .await
        .expect("the earlier attempt admits");
        let mob_host: Arc<dyn SurfaceScheduleMobHost> =
            Arc::new(NoopScheduleMobHost::new("mob targets unused here"));
        let adapter = SharedScheduleTargetAdapter::new(
            service.clone(),
            Arc::clone(&host) as Arc<dyn SurfaceScheduleSessionHost>,
            mob_host,
        );
        let answer = adapter
            .deliver_existing_occurrence(first, &first_identity, ExistingDelivery::UnprovenLate)
            .await
            .expect("the redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(dispatch) = answer else {
            panic!("the redelivery finds the earlier admission: {answer:?}");
        };
        let reported = dispatch
            .materialized_session_id
            .clone()
            .expect("the found admission reports its session");
        // What the driver does with a dispatch that reports its session.
        service
            .bind_materialized_session_for_occurrence(first, &reported)
            .await
            .expect("the driver's bind");

        let later = service
            .list_occurrences(&later.schedule_id)
            .await
            .expect("list occurrences")
            .into_iter()
            .find(|occurrence| occurrence.occurrence_id == later.occurrence_id)
            .expect("the later occurrence is still planned");
        let later_dispatch = adapter
            .deliver_occurrence(&later, &ScheduleDeliveryIdentity::for_occurrence(&later))
            .await
            .expect("the later occurrence delivers");
        assert_eq!(
            later_dispatch.materialized_session_id,
            Some(session_id.clone())
        );
        assert_eq!(admitted_input_count(&runtime, &session_id).await, 2);
        assert_eq!(materialize_calls.load(Ordering::SeqCst), 0);

        service
            .bind_materialized_session_for_occurrence(first, &SessionId::new())
            .await
            .expect("a second bind answers");
        let schedule = service.get(&first.schedule_id).await.expect("the schedule");
        let TargetBinding::Session(binding) = &schedule.target else {
            panic!("a session target");
        };
        assert_eq!(
            binding.resolved_session_id(),
            Some(&session_id),
            "a bind never overwrites the committed binding"
        );
    }

    /// A session host that admits a scheduled prompt through its own runtime
    /// call instead of [`accept_schedule_runtime_input_with_reconciliation`],
    /// so it never reads a redelivery's scope.
    struct OwnAdmissionHost {
        runtime: Arc<MeerkatMachine>,
        deliveries: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for OwnAdmissionHost {
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
            Err(ScheduleDomainError::Internal(
                "this host targets an exact session".to_string(),
            ))
        }

        async fn deliver_prompt(
            &self,
            session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.deliveries.fetch_add(1, Ordering::SeqCst);
            let mut prompt =
                meerkat_runtime::PromptInput::from_content_input(dispatch.prompt, None);
            prompt.header.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(
                identity.idempotency_key.clone(),
            ));
            prompt.header.correlation_id = Some(schedule_runtime_correlation_id(identity)?);
            let (outcome, handle) = self
                .runtime
                .accept_input_with_completion(session_id, meerkat_runtime::Input::Prompt(prompt))
                .await
                .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
            runtime_delivery_dispatch_from_admission(
                &self.runtime,
                session_id,
                occurrence,
                identity,
                outcome,
                handle,
                dispatch.materialized_session_id,
            )
            .await
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            _occurrence: &Occurrence,
            _identity: &ScheduleDeliveryIdentity,
            _dispatch: ScheduledEventDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Err(ScheduleDomainError::Internal(
                "this host delivers prompts only".to_string(),
            ))
        }
    }

    /// A planned occurrence (ordinal `ordinal`) whose target is the exact
    /// session `session_id`, as its first attempt.
    fn exact_session_occurrence(session_id: &SessionId, ordinal: u64) -> Occurrence {
        let schedule = meerkat_schedule::Schedule::new(meerkat_schedule::CreateScheduleRequest {
            name: Some("existing-admission".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Interval(
                meerkat_schedule::IntervalTriggerSpec {
                    start_at_utc: chrono::Utc::now(),
                    every_seconds: 60,
                    end_at_utc: None,
                },
            ),
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: session_id.clone(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::Text("scheduled prompt".to_string()),
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
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("schedule creation should pass generated authority");
        let mut occurrence = Occurrence::planned_from_schedule(
            &schedule,
            meerkat_schedule::OccurrenceOrdinal(ordinal),
            chrono::Utc::now(),
        )
        .expect("occurrence planning should pass generated authority");
        occurrence.attempt_count = 1;
        occurrence
    }

    async fn admitted_input_count(runtime: &MeerkatMachine, session_id: &SessionId) -> usize {
        runtime
            .meerkat_machine_spine_snapshot(session_id)
            .await
            .expect("the session is registered")
            .inputs
            .admission_order
            .len()
    }

    /// A session host that keeps the trait's default does not decide an
    /// existing-only admission, so the adapter never asks it to redeliver an
    /// unproven late occurrence: the answer is not admitted, and nothing
    /// reaches the runtime. An admitted redelivery goes through its ordinary
    /// delivery, like the other defaults. Before the capability, the adapter
    /// asked every host, and one that admits through its own runtime call
    /// admitted the late delivery fresh.
    #[tokio::test]
    async fn a_host_that_does_not_decide_existing_admission_never_redelivers_late() {
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        runtime
            .register_session(session_id.clone())
            .await
            .expect("register the target session");
        let deliveries = Arc::new(AtomicUsize::new(0));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(OwnAdmissionHost {
            runtime: Arc::clone(&runtime),
            deliveries: Arc::clone(&deliveries),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> =
            Arc::new(NoopScheduleMobHost::new("mob targets unused here"));
        let service = ScheduleService::new(
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>
        );
        let adapter = SharedScheduleTargetAdapter::new(service, session_host, mob_host);

        let late = exact_session_occurrence(&session_id, 0);
        let answer = adapter
            .deliver_existing_occurrence(
                &late,
                &ScheduleDeliveryIdentity::for_occurrence(&late),
                ExistingDelivery::UnprovenLate,
            )
            .await
            .expect("the unproven late redelivery answers");
        assert!(
            matches!(answer, ExistingDeliveryAnswer::NotAdmittedPastDeadline),
            "an unproven late redelivery is never asked of this host: {answer:?}"
        );
        assert_eq!(
            deliveries.load(Ordering::SeqCst),
            0,
            "the host is not asked"
        );
        assert_eq!(
            admitted_input_count(&runtime, &session_id).await,
            0,
            "nothing reaches the runtime"
        );

        let admitted = exact_session_occurrence(&session_id, 1);
        let answer = adapter
            .deliver_existing_occurrence(
                &admitted,
                &ScheduleDeliveryIdentity::for_occurrence(&admitted),
                ExistingDelivery::Admitted,
            )
            .await
            .expect("the admitted redelivery answers");
        let ExistingDeliveryAnswer::Dispatch(dispatch) = answer else {
            panic!("an admitted redelivery answers with a dispatch: {answer:?}");
        };
        assert_eq!(
            dispatch.receipt.stage,
            DeliveryReceiptStage::DispatchAccepted
        );
        assert_eq!(
            deliveries.load(Ordering::SeqCst),
            1,
            "an admitted redelivery goes through the host's ordinary delivery"
        );
    }

    /// [`OwnAdmissionHost`] declaring that it decides existing-only admission
    /// while it still admits through its own runtime call.
    struct DeclaringOwnAdmissionHost(OwnAdmissionHost);

    #[async_trait]
    impl SurfaceScheduleSessionHost for DeclaringOwnAdmissionHost {
        fn decides_existing_admission(&self) -> bool {
            true
        }

        async fn probe_session_target(
            &self,
            binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            self.0.probe_session_target(binding).await
        }

        async fn materialize_session(
            &self,
            occurrence: &Occurrence,
            create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            self.0.materialize_session(occurrence, create).await
        }

        async fn deliver_prompt(
            &self,
            session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.0
                .deliver_prompt(session_id, occurrence, identity, dispatch)
                .await
        }

        async fn deliver_event(
            &self,
            session_id: &SessionId,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            dispatch: ScheduledEventDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.0
                .deliver_event(session_id, occurrence, identity, dispatch)
                .await
        }
    }

    /// A host that declares it decides existing-only admission but admits
    /// through its own runtime call never reads the redelivery scope: the
    /// adapter reports the incident, and a debug build stops on it, so a
    /// host that stops consulting the scope fails its rows instead of
    /// passing silently.
    #[cfg(debug_assertions)]
    #[tokio::test]
    #[should_panic(expected = "redelivery admitted without consulting the redelivery scope")]
    async fn a_declaring_host_that_bypasses_the_redelivery_scope_is_an_incident() {
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        runtime
            .register_session(session_id.clone())
            .await
            .expect("register the target session");
        let session_host: Arc<dyn SurfaceScheduleSessionHost> =
            Arc::new(DeclaringOwnAdmissionHost(OwnAdmissionHost {
                runtime: Arc::clone(&runtime),
                deliveries: Arc::new(AtomicUsize::new(0)),
            }));
        let mob_host: Arc<dyn SurfaceScheduleMobHost> =
            Arc::new(NoopScheduleMobHost::new("mob targets unused here"));
        let service = ScheduleService::new(
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>
        );
        let adapter = SharedScheduleTargetAdapter::new(service, session_host, mob_host);
        let late = exact_session_occurrence(&session_id, 0);
        let _answer = adapter
            .deliver_existing_occurrence(
                &late,
                &ScheduleDeliveryIdentity::for_occurrence(&late),
                ExistingDelivery::UnprovenLate,
            )
            .await;
    }

    /// A service with a probe attached, and the adapter that owns the probe:
    /// the service holds it weakly, so it judges only while the caller keeps
    /// the adapter.
    fn probe_attached_service() -> (ScheduleService, SharedScheduleTargetAdapter) {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(PanicOnMaterializeHost {
            materialize_calls: Arc::new(AtomicUsize::new(0)),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = SharedScheduleTargetAdapter::new(service.clone(), session_host, mob_host);
        (service, adapter)
    }

    /// GAP 2 regression: an identity target no host on the surface
    /// recognizes could previously be created and then misfire
    /// `target_missing` on every occurrence forever. Constructing the
    /// delivery adapter attaches a create-time deliverability probe.
    #[tokio::test]
    async fn create_refuses_identity_target_no_surface_host_recognizes() {
        let (service, _adapter) = probe_attached_service();
        let error = service
            .create(identity_create_request("domain:security"))
            .await
            .expect_err("a bare identity no host recognizes must fail at create");
        assert!(
            matches!(
                &error,
                meerkat_schedule::ScheduleDomainError::InvalidSchedule(reason)
                    if reason.contains("not deliverable")
                        // The refusal must carry the exact detail the fire
                        // path would have reported on every misfire.
                        && reason.contains(
                            "scheduled identity targets are not supported by this session host"
                        )
            ),
            "expected a create-time deliverability refusal carrying the fire-path detail, found {error:?}"
        );
    }

    /// The agent-facing `current_session` shortcut keeps resolving with the
    /// create-time probe attached: for a mob member the resolver rewrites it
    /// to the member's identity form, which the mob host recognizes.
    #[tokio::test]
    async fn current_session_shortcut_still_resolves_with_probe_attached() {
        use meerkat_core::AgentToolDispatcher;

        struct MemberRecognizingMobHost;

        #[async_trait]
        impl SurfaceScheduleMobHost for MemberRecognizingMobHost {
            async fn probe_mob_target(
                &self,
                _binding: &MobTargetBinding,
            ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
                Ok(TargetProbeOutcome::Missing { detail: None })
            }

            async fn deliver_mob_target(
                &self,
                _occurrence: &Occurrence,
                _identity: &ScheduleDeliveryIdentity,
                _binding: &MobTargetBinding,
            ) -> Result<DeliveryDispatch, ScheduleDomainError> {
                Err(ScheduleDomainError::Internal(
                    "delivery is out of scope for this test".to_string(),
                ))
            }

            async fn probe_identity_target(
                &self,
                binding: &IdentityTargetBinding,
            ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
                Ok(parse_mob_member_schedule_identity(binding.identity())
                    .map(|_| TargetProbeOutcome::Ready))
            }
        }

        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(PanicOnMaterializeHost {
            materialize_calls: Arc::new(AtomicUsize::new(0)),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(MemberRecognizingMobHost);
        let _adapter = SharedScheduleTargetAdapter::new(service.clone(), session_host, mob_host);

        let binding = meerkat_core::MobMemberBinding {
            mob_id: "ops".to_string(),
            role: "watcher".to_string(),
            member: "watcher".to_string(),
        };
        let dispatcher = meerkat_schedule::CurrentSessionScheduleToolDispatcher::new_with_resolver(
            Arc::new(meerkat_schedule::ScheduleToolDispatcher::new(
                service.clone(),
            )),
            SessionId::new(),
            Arc::new(MobMemberCurrentSessionScheduleResolver::new(
                binding.clone(),
            )),
        );

        let request = serde_json::json!({
            "name": "self-followup",
            "trigger": {
                "type": "once",
                "due_at_utc": (chrono::Utc::now() + ChronoDuration::days(1)).to_rfc3339(),
            },
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": { "type": "prompt", "prompt": "check in" }
            },
            "misfire_policy": { "type": "skip" },
            "overlap_policy": "skip_if_running",
            "missing_target_policy": "mark_misfired",
            "planning_horizon_occurrences": 1
        });
        let raw = serde_json::value::RawValue::from_string(request.to_string())
            .expect("encode tool arguments");
        let outcome = dispatcher
            .dispatch(meerkat_core::ToolCallView {
                id: "sched-current-session-probe",
                name: "meerkat_schedule_create",
                args: raw.as_ref(),
            })
            .await
            .expect("current_session must keep resolving with the probe attached");
        let created: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("decode created schedule");
        assert_eq!(created["target"]["target_kind"].as_str(), Some("identity"));
        assert_eq!(
            created["target"]["identity"].as_str(),
            Some(mob_member_schedule_identity(&binding).as_str()),
            "the shortcut must persist the member's identity form"
        );
    }

    /// The recognized-but-currently-absent member class stays creatable:
    /// absence at fire time is governed by `missing_target_policy`, and the
    /// member may exist by the first occurrence.
    #[tokio::test]
    async fn create_accepts_recognized_identity_target_with_currently_missing_member() {
        struct RecognizingMobHost;

        #[async_trait]
        impl SurfaceScheduleMobHost for RecognizingMobHost {
            async fn probe_mob_target(
                &self,
                _binding: &MobTargetBinding,
            ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
                Ok(TargetProbeOutcome::Missing { detail: None })
            }

            async fn deliver_mob_target(
                &self,
                _occurrence: &Occurrence,
                _identity: &ScheduleDeliveryIdentity,
                _binding: &MobTargetBinding,
            ) -> Result<DeliveryDispatch, ScheduleDomainError> {
                Err(ScheduleDomainError::Internal(
                    "delivery is out of scope for this test".to_string(),
                ))
            }

            async fn probe_identity_target(
                &self,
                _binding: &IdentityTargetBinding,
            ) -> Result<Option<TargetProbeOutcome>, ScheduleDomainError> {
                Ok(Some(TargetProbeOutcome::Missing {
                    detail: Some("mob member not found: watcher".to_string()),
                }))
            }
        }

        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(PanicOnMaterializeHost {
            materialize_calls: Arc::new(AtomicUsize::new(0)),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(RecognizingMobHost);
        let _adapter = SharedScheduleTargetAdapter::new(service.clone(), session_host, mob_host);

        service
            .create(identity_create_request(
                r#"mob_member:{"schema":"meerkat.schedule.mob_member_identity.v2","mob_id":"ops","member":"watcher"}"#,
            ))
            .await
            .expect("a recognized identity form must keep creating even while the member is absent");
    }

    #[tokio::test]
    async fn create_keeps_accepting_session_targets_with_probe_attached() {
        let (service, _adapter) = probe_attached_service();
        service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("session-target".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() + ChronoDuration::days(1),
                },
                target: materialize_on_demand_target(),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("session targets are never probed at create");
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

    /// A session host with a fixed identity judgement whose lifetime a test
    /// watches through a `Weak` of `_alive`.
    struct FixedIdentityHost {
        _alive: Arc<()>,
        identity: TargetProbeOutcome,
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for FixedIdentityHost {
        async fn probe_session_target(
            &self,
            _binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }

        async fn probe_identity_target(
            &self,
            _binding: &IdentityTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(self.identity.clone())
        }

        async fn materialize_session(
            &self,
            _occurrence: &Occurrence,
            _create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            Err(ScheduleDomainError::Internal(
                "delivery is out of scope for this test".to_string(),
            ))
        }

        async fn deliver_prompt(
            &self,
            _session_id: &SessionId,
            _occurrence: &Occurrence,
            _identity: &ScheduleDeliveryIdentity,
            _dispatch: ScheduledPromptDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Err(ScheduleDomainError::Internal(
                "delivery is out of scope for this test".to_string(),
            ))
        }

        async fn deliver_event(
            &self,
            _session_id: &SessionId,
            _occurrence: &Occurrence,
            _identity: &ScheduleDeliveryIdentity,
            _dispatch: ScheduledEventDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Err(ScheduleDomainError::Internal(
                "delivery is out of scope for this test".to_string(),
            ))
        }
    }

    /// Install agent schedule tools on `service` (as a factory builder's
    /// `default_schedule_tools` slot holds them), spawn a host whose session
    /// host vouches for every identity, and stop that host. Returns the tools
    /// and a watch on the stopped host's session host.
    async fn stop_a_host_under_installed_tools(
        service: &ScheduleService,
    ) -> (
        meerkat_schedule::ScheduleToolDispatcher,
        std::sync::Weak<()>,
    ) {
        let tools = meerkat_schedule::ScheduleToolDispatcher::new(service.clone());
        let alive = Arc::new(());
        let watch = Arc::downgrade(&alive);
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(FixedIdentityHost {
            _alive: alive,
            identity: TargetProbeOutcome::Ready,
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = Arc::new(SharedScheduleTargetAdapter::new(
            service.clone(),
            session_host,
            mob_host,
        ));
        spawn_schedule_host(service.clone(), adapter, "stopped-host")
            .shutdown()
            .await;
        (tools, watch)
    }

    fn memory_schedule_service() -> ScheduleService {
        ScheduleService::new(
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>
        )
    }

    /// Schedule tools installed on a builder hold a clone of the service, and
    /// the session service owns that builder. Once the host has stopped they
    /// must not keep its session host (and through it the session service)
    /// alive, so an embedder needs no manual clear after teardown.
    #[tokio::test]
    async fn a_stopped_schedule_host_releases_its_session_host_under_installed_tools() {
        let service = memory_schedule_service();
        let (tools, watch) = stop_a_host_under_installed_tools(&service).await;
        let released = tokio::time::timeout(Duration::from_secs(5), async {
            while watch.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            released.is_ok(),
            "the stopped host's session host is still owned through the schedule tools"
        );
        drop(tools);
    }

    /// After its host has stopped, an identity-target create through the
    /// installed tools is a typed unavailable result, never a judgement by
    /// the stopped host.
    #[tokio::test]
    async fn installed_schedule_tools_report_a_stopped_host_as_unavailable() {
        use meerkat_core::AgentToolDispatcher;

        let service = memory_schedule_service();
        let (tools, _watch) = stop_a_host_under_installed_tools(&service).await;
        let arguments = serde_json::to_string(&identity_create_request("domain:ops"))
            .expect("encode create arguments");
        let raw =
            serde_json::value::RawValue::from_string(arguments).expect("encode tool arguments");
        let error = tools
            .dispatch(meerkat_core::ToolCallView {
                id: "create-after-teardown",
                name: "meerkat_schedule_create",
                args: raw.as_ref(),
            })
            .await
            .expect_err("no stopped host may judge an identity target");
        assert!(
            matches!(error, meerkat_core::ToolError::Unavailable { .. }),
            "expected a typed unavailable result, found {error:?}"
        );
    }

    /// A host attached after the first one stopped judges identity targets
    /// itself: the first attach no longer wins once its host is gone.
    #[tokio::test]
    async fn a_host_attached_after_teardown_judges_identity_targets() {
        let service = memory_schedule_service();
        let (_tools, _watch) = stop_a_host_under_installed_tools(&service).await;
        let refusing: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(FixedIdentityHost {
            _alive: Arc::new(()),
            identity: TargetProbeOutcome::Missing {
                detail: Some("unknown to the second host".to_string()),
            },
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let _second = SharedScheduleTargetAdapter::new(service.clone(), refusing, mob_host);
        let error = service
            .create(identity_create_request("domain:ops"))
            .await
            .expect_err("the second host refuses the identity");
        assert!(
            matches!(
                &error,
                ScheduleDomainError::InvalidSchedule(reason)
                    if reason.contains("unknown to the second host")
            ),
            "expected the second host's refusal, found {error:?}"
        );
    }

    /// Call one schedule tool through `tools` and decode its result.
    async fn call_schedule_tool(
        tools: &meerkat_schedule::ScheduleToolDispatcher,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, meerkat_core::ToolError> {
        use meerkat_core::AgentToolDispatcher;

        let raw = serde_json::value::RawValue::from_string(arguments.to_string())
            .expect("encode tool arguments");
        let outcome = tools
            .dispatch(meerkat_core::ToolCallView {
                id: name,
                name,
                args: raw.as_ref(),
            })
            .await?;
        Ok(serde_json::from_str(&outcome.result.text_content()).expect("decode tool result"))
    }

    /// Teardown takes away only the identity judgement: tool calls that read
    /// or change the store alone keep working through the installed tools
    /// after the host has stopped.
    #[tokio::test]
    async fn installed_schedule_tools_keep_store_only_calls_after_teardown() {
        let service = memory_schedule_service();
        let (tools, _watch) = stop_a_host_under_installed_tools(&service).await;
        let schedule_id = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("after-teardown".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() + ChronoDuration::days(1),
                },
                target: materialize_on_demand_target(),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("a session target is never judged by a host")
            .schedule_id
            .to_string();
        let by_id = serde_json::json!({ "schedule_id": schedule_id });

        let listed = call_schedule_tool(&tools, "meerkat_schedule_list", &serde_json::json!({}))
            .await
            .expect("list keeps working after teardown");
        assert_eq!(
            listed["schedules"][0]["schedule_id"].as_str(),
            Some(schedule_id.as_str())
        );
        let fetched = call_schedule_tool(&tools, "meerkat_schedule_get", &by_id)
            .await
            .expect("get keeps working after teardown");
        assert_eq!(fetched["phase"].as_str(), Some("active"));
        let paused = call_schedule_tool(&tools, "meerkat_schedule_pause", &by_id)
            .await
            .expect("pause keeps working after teardown");
        assert_eq!(paused["phase"].as_str(), Some("paused"));
    }

    /// Two real hosts over one shared service: the earlier one judges
    /// identity targets while it runs, the later one judges once the earlier
    /// one has stopped, and with both stopped identity targets are refused as
    /// stopped.
    #[tokio::test]
    async fn two_hosts_over_one_service_judge_in_attach_order_and_hand_off() {
        let service = memory_schedule_service();
        let adapter = |identity: TargetProbeOutcome| {
            let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(FixedIdentityHost {
                _alive: Arc::new(()),
                identity,
            });
            let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
                "mob targets unsupported in this test",
            ));
            Arc::new(SharedScheduleTargetAdapter::new(
                service.clone(),
                session_host,
                mob_host,
            ))
        };
        let first = spawn_schedule_host(
            service.clone(),
            adapter(TargetProbeOutcome::Missing {
                detail: Some("unknown to the first host".to_string()),
            }),
            "first-host",
        );
        let second = spawn_schedule_host(
            service.clone(),
            adapter(TargetProbeOutcome::Ready),
            "second-host",
        );

        let refused = service
            .create(identity_create_request("domain:ops"))
            .await
            .expect_err("the first host judges while it runs");
        assert!(
            matches!(
                &refused,
                ScheduleDomainError::InvalidSchedule(reason)
                    if reason.contains("unknown to the first host")
            ),
            "expected the first host's refusal, found {refused:?}"
        );

        first.shutdown().await;
        service
            .create(identity_create_request("domain:ops"))
            .await
            .expect("the second host judges once the first has stopped");

        second.shutdown().await;
        let stopped = service
            .create(identity_create_request("domain:ops"))
            .await
            .expect_err("no running host is left to judge");
        assert!(
            matches!(stopped, ScheduleDomainError::DriverStopped),
            "expected DriverStopped, found {stopped:?}"
        );
    }

    fn mandate_agent(name: &str) -> meerkat_core::PrincipalRef {
        meerkat_core::PrincipalRef::new(meerkat_core::PrincipalKind::PersonalAgent, name)
            .expect("agent principal")
    }

    /// A governed host's agent; the adapter only reads it, and the session
    /// host (not under test here) binds it.
    struct FixedAgentBinding(meerkat_core::PrincipalRef);

    #[async_trait]
    impl ScheduledSessionAgentBinding for FixedAgentBinding {
        fn agent(&self) -> &meerkat_core::PrincipalRef {
            &self.0
        }

        async fn bind_scheduled_session(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &SessionId,
            _configured_servers: &[meerkat_core::McpServerConfig],
        ) -> Result<(), ScheduleDomainError> {
            Ok(())
        }
    }

    struct CountingMaterializeHost {
        materialize_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SurfaceScheduleSessionHost for CountingMaterializeHost {
        async fn probe_session_target(
            &self,
            _binding: &SessionTargetBinding,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }

        async fn materialize_session(
            &self,
            occurrence: &Occurrence,
            _create: &SessionMaterializationSpec,
        ) -> Result<SessionId, ScheduleDomainError> {
            self.materialize_calls.fetch_add(1, Ordering::SeqCst);
            Ok(occurrence.materialized_session_id())
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
            _dispatch: ScheduledEventDispatch,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            Ok(immediate_completed_dispatch(
                occurrence,
                Some(identity.correlation_id.clone()),
            ))
        }
    }

    /// The planned occurrence of one due materialize-on-demand schedule
    /// created through `service` (so with `service`'s commissioning agent).
    async fn planned_commissioned_occurrence(service: &ScheduleService) -> Occurrence {
        planned_commissioned_occurrence_due_at(
            service,
            chrono::Utc::now() - ChronoDuration::seconds(1),
        )
        .await
    }

    /// As [`planned_commissioned_occurrence`], due at `due_at_utc`.
    async fn planned_commissioned_occurrence_due_at(
        service: &ScheduleService,
        due_at_utc: chrono::DateTime<chrono::Utc>,
    ) -> Occurrence {
        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("commissioned".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once { due_at_utc },
                target: materialize_on_demand_target(),
                misfire_policy: meerkat_schedule::MisfirePolicy::Skip,
                overlap_policy: meerkat_schedule::OverlapPolicy::AllowConcurrent,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::Skip,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("schedule create");
        service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("list occurrences")
            .into_iter()
            .next()
            .expect("planned occurrence")
    }

    /// Deliver `occurrence` through an adapter over `host_service`, bound to
    /// `host_agent` when given: the terminal's phase, failure reason and
    /// detail, and how many sessions the host materialized.
    async fn deliver_under_host_agent(
        host_service: ScheduleService,
        host_agent: Option<meerkat_core::PrincipalRef>,
        occurrence: &Occurrence,
    ) -> (
        OccurrencePhase,
        Option<DeliveryFailureReason>,
        Option<String>,
        usize,
    ) {
        let materialize_calls = Arc::new(AtomicUsize::new(0));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(CountingMaterializeHost {
            materialize_calls: Arc::clone(&materialize_calls),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        let adapter = SharedScheduleTargetAdapter::new(host_service, session_host, mob_host);
        let adapter = match host_agent {
            Some(agent) => adapter.with_session_agent_binding(Arc::new(FixedAgentBinding(agent))),
            None => adapter,
        };
        let identity = ScheduleDeliveryIdentity::for_occurrence(occurrence);
        let dispatch = adapter
            .deliver_occurrence(occurrence, &identity)
            .await
            .expect("delivery dispatch");
        assert_eq!(
            dispatch.correlation_id.as_deref(),
            Some(identity.correlation_id.as_str())
        );
        let terminal = dispatch.completion.await.expect("delivery completion");
        (
            terminal.phase,
            terminal.delivery_failure_reason,
            terminal.detail,
            materialize_calls.load(Ordering::SeqCst),
        )
    }

    #[tokio::test]
    async fn governed_host_fires_a_schedule_its_own_agent_commissioned() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new_commissioned(store, mandate_agent("agent-a"));
        let occurrence = planned_commissioned_occurrence(&service).await;

        let (phase, reason, _detail, materialized) =
            deliver_under_host_agent(service, Some(mandate_agent("agent-a")), &occurrence).await;

        assert_eq!(phase, OccurrencePhase::Completed);
        assert_eq!(reason, None);
        assert_eq!(materialized, 1);
    }

    #[tokio::test]
    async fn governed_host_refuses_a_schedule_another_agent_commissioned() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let commissioning =
            ScheduleService::new_commissioned(Arc::clone(&store), mandate_agent("agent-b"));
        let occurrence = planned_commissioned_occurrence(&commissioning).await;
        let host_service = ScheduleService::new_commissioned(store, mandate_agent("agent-a"));

        let (phase, reason, detail, materialized) =
            deliver_under_host_agent(host_service, Some(mandate_agent("agent-a")), &occurrence)
                .await;

        assert_eq!(phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            reason,
            Some(DeliveryFailureReason::ScheduleMandateUnavailable)
        );
        assert!(
            detail
                .as_deref()
                .is_some_and(|detail| detail.contains("agent-b") && detail.contains("agent-a")),
            "the refusal names both agents: {detail:?}"
        );
        assert_eq!(materialized, 0, "a refused fire materializes no session");
    }

    /// A schedule recorded before commissioning agents existed (or by an
    /// ungoverned host) is refused by a governed host: fail closed.
    #[tokio::test]
    async fn governed_host_refuses_a_schedule_recorded_without_an_agent() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let occurrence =
            planned_commissioned_occurrence(&ScheduleService::new(Arc::clone(&store))).await;
        let host_service = ScheduleService::new_commissioned(store, mandate_agent("agent-a"));

        let (phase, reason, detail, materialized) =
            deliver_under_host_agent(host_service, Some(mandate_agent("agent-a")), &occurrence)
                .await;

        assert_eq!(phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            reason,
            Some(DeliveryFailureReason::ScheduleMandateUnavailable)
        );
        assert!(
            detail
                .as_deref()
                .is_some_and(|detail| detail.contains("records no commissioning agent")),
            "the refusal says the schedule records no agent: {detail:?}"
        );
        assert_eq!(materialized, 0);
    }

    /// A host whose service commissions schedules for an agent, composed
    /// without that agent's binding, fires none: it cannot bind the agent.
    #[tokio::test]
    async fn commissioned_service_without_a_binding_fires_no_schedule() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new_commissioned(store, mandate_agent("agent-a"));
        let occurrence = planned_commissioned_occurrence(&service).await;

        let (phase, reason, _detail, materialized) =
            deliver_under_host_agent(service, None, &occurrence).await;

        assert_eq!(phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            reason,
            Some(DeliveryFailureReason::ScheduleMandateUnavailable)
        );
        assert_eq!(materialized, 0);
    }

    #[tokio::test]
    async fn ungoverned_host_fires_schedules_unchecked() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store);
        let occurrence = planned_commissioned_occurrence(&service).await;

        let (phase, reason, _detail, materialized) =
            deliver_under_host_agent(service, None, &occurrence).await;

        assert_eq!(phase, OccurrencePhase::Completed);
        assert_eq!(reason, None);
        assert_eq!(materialized, 1);
    }

    /// Through the real driver and the generated occurrence authority, a
    /// refused fire is durably `DeliveryFailed` with the typed
    /// `schedule_mandate_unavailable` class on the occurrence and its
    /// terminal receipt, and the target never runs.
    #[tokio::test]
    async fn refused_mandate_records_schedule_mandate_unavailable_through_driver() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let commissioning =
            ScheduleService::new_commissioned(Arc::clone(&store), mandate_agent("agent-b"));
        let schedule = create_host_runnable_schedule(&commissioning, "nightly-report", None).await;
        assert_eq!(
            schedule.config.commissioning_agent,
            Some(mandate_agent("agent-b"))
        );

        let host_service =
            ScheduleService::new_commissioned(Arc::clone(&store), mandate_agent("agent-a"));
        let (runnable, invocations) = recording_runnable(None);
        let adapter = Arc::new(
            host_runnable_adapter(
                host_service.clone(),
                Some(registry_with("nightly-report", runnable)),
            )
            .with_session_agent_binding(Arc::new(FixedAgentBinding(mandate_agent("agent-a")))),
        );
        let driver = host_runnable_driver(host_service.clone(), Arc::clone(&store), adapter);

        driver.tick_once().await.expect("driver tick");

        let occurrence = wait_for_occurrence_phase(
            &host_service,
            &schedule.schedule_id,
            OccurrencePhase::DeliveryFailed,
        )
        .await;
        assert_eq!(
            occurrence.failure_class,
            Some(OccurrenceFailureClass::ScheduleMandateUnavailable)
        );
        let receipts = store
            .list_receipts(&occurrence.occurrence_id)
            .await
            .expect("receipts");
        let last_receipt = receipts.last().expect("terminal receipt");
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DeliveryFailed);
        assert_eq!(
            last_receipt.failure_class,
            Some(OccurrenceFailureClass::ScheduleMandateUnavailable)
        );
        assert!(
            invocations.lock().expect("invocation lock").is_empty(),
            "a refused fire never runs its target"
        );
    }

    /// Bind `occurrence`'s materialize-on-demand target to a session, as an
    /// earlier fire does: the pending occurrence carries the bound session.
    async fn bind_pending_target(service: &ScheduleService, occurrence: &Occurrence) {
        service
            .bind_materialized_session_for_occurrence(occurrence, &SessionId::new())
            .await
            .expect("bind the materialized session");
        let pending = service
            .list_occurrences(&occurrence.schedule_id)
            .await
            .expect("list occurrences")
            .into_iter()
            .find(|candidate| candidate.occurrence_id == occurrence.occurrence_id)
            .expect("pending occurrence");
        let TargetBinding::Session(binding) = &pending.target_snapshot else {
            panic!("expected a session target binding");
        };
        assert!(
            binding.resolved_session_id().is_some(),
            "the pending occurrence carries the bound session"
        );
    }

    /// An adapter over `host_service` for a governed host acting as `agent`,
    /// whose session host counts materializations into `materialize_calls`.
    fn governed_counting_adapter(
        host_service: ScheduleService,
        agent: &str,
        materialize_calls: &Arc<AtomicUsize>,
    ) -> Arc<SharedScheduleTargetAdapter> {
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(CountingMaterializeHost {
            materialize_calls: Arc::clone(materialize_calls),
        });
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = Arc::new(NoopScheduleMobHost::new(
            "mob targets unsupported in this test",
        ));
        Arc::new(
            SharedScheduleTargetAdapter::new(host_service, session_host, mob_host)
                .with_session_agent_binding(Arc::new(FixedAgentBinding(mandate_agent(agent)))),
        )
    }

    /// A governed host refuses a fire whose materialize-on-demand target an
    /// earlier fire bound. The refusal names that bound session, as every
    /// dispatch for the occurrence must, so the driver records it: the
    /// occurrence ends `DeliveryFailed` with `schedule_mandate_unavailable` in
    /// one tick, and the tick does not fail.
    #[tokio::test]
    async fn refused_mandate_on_a_bound_target_ends_the_occurrence_in_one_tick() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let commissioning =
            ScheduleService::new_commissioned(Arc::clone(&store), mandate_agent("agent-b"));
        let planned = planned_commissioned_occurrence(&commissioning).await;
        bind_pending_target(&commissioning, &planned).await;

        let host_service =
            ScheduleService::new_commissioned(Arc::clone(&store), mandate_agent("agent-a"));
        let materialize_calls = Arc::new(AtomicUsize::new(0));
        let adapter =
            governed_counting_adapter(host_service.clone(), "agent-a", &materialize_calls);
        let driver = host_runnable_driver(host_service.clone(), Arc::clone(&store), adapter);

        driver
            .tick_once()
            .await
            .expect("a refused fire is not a driver fault");

        let occurrence = wait_for_occurrence_phase(
            &host_service,
            &planned.schedule_id,
            OccurrencePhase::DeliveryFailed,
        )
        .await;
        assert_eq!(
            occurrence.failure_class,
            Some(OccurrenceFailureClass::ScheduleMandateUnavailable)
        );
        let receipts = store
            .list_receipts(&occurrence.occurrence_id)
            .await
            .expect("receipts");
        let last_receipt = receipts.last().expect("terminal receipt");
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DeliveryFailed);
        assert_eq!(
            last_receipt.failure_class,
            Some(OccurrenceFailureClass::ScheduleMandateUnavailable)
        );
        assert_eq!(
            materialize_calls.load(Ordering::SeqCst),
            0,
            "a refused fire materializes no session"
        );
    }

    /// One refused fire does not hold back the rest of the tick. A schedule
    /// recorded without a commissioning agent, whose target an earlier fire
    /// bound, is claimed first and refused; the host agent's own schedule,
    /// claimed in the same tick, still fires.
    #[tokio::test]
    async fn a_refused_bound_target_does_not_hold_back_the_rest_of_the_tick() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let unrecorded = ScheduleService::new(Arc::clone(&store));
        // Due first, so claimed first.
        let legacy = planned_commissioned_occurrence_due_at(
            &unrecorded,
            chrono::Utc::now() - ChronoDuration::seconds(10),
        )
        .await;
        bind_pending_target(&unrecorded, &legacy).await;
        let host_service =
            ScheduleService::new_commissioned(Arc::clone(&store), mandate_agent("agent-a"));
        let own = planned_commissioned_occurrence(&host_service).await;

        let materialize_calls = Arc::new(AtomicUsize::new(0));
        let adapter =
            governed_counting_adapter(host_service.clone(), "agent-a", &materialize_calls);
        let driver = host_runnable_driver(host_service.clone(), Arc::clone(&store), adapter);

        let tick = driver.tick_once().await;

        wait_for_occurrence_phase(&host_service, &own.schedule_id, OccurrencePhase::Completed)
            .await;
        assert_eq!(materialize_calls.load(Ordering::SeqCst), 1);
        let refused = wait_for_occurrence_phase(
            &host_service,
            &legacy.schedule_id,
            OccurrencePhase::DeliveryFailed,
        )
        .await;
        assert_eq!(
            refused.failure_class,
            Some(OccurrenceFailureClass::ScheduleMandateUnavailable)
        );
        let report = tick.expect("a refused fire is not a driver fault");
        assert_eq!(report.claimed_occurrences, 2);
    }

    /// An update by a writer that commissions no agent clears the schedule's
    /// agent (a redefinition is commissioned by its writer), and a gated host
    /// then refuses the schedule's fire.
    #[tokio::test]
    async fn an_uncommissioned_update_clears_the_agent_and_a_gated_host_refuses() {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let host_service =
            ScheduleService::new_commissioned(Arc::clone(&store), mandate_agent("agent-a"));
        let commissioned = planned_commissioned_occurrence(&host_service).await;

        let updated = ScheduleService::new(Arc::clone(&store))
            .update(
                &commissioned.schedule_id,
                meerkat_schedule::UpdateScheduleRequest {
                    name: Some("rewritten".to_string()),
                    ..meerkat_schedule::UpdateScheduleRequest::default()
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.config.commissioning_agent, None);
        let occurrence = host_service
            .list_occurrences(&commissioned.schedule_id)
            .await
            .expect("list occurrences")
            .into_iter()
            .rev()
            .find(|occurrence| occurrence.phase == OccurrencePhase::Pending)
            .unwrap_or(commissioned);

        let (phase, reason, detail, materialized) =
            deliver_under_host_agent(host_service, Some(mandate_agent("agent-a")), &occurrence)
                .await;

        assert_eq!(phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            reason,
            Some(DeliveryFailureReason::ScheduleMandateUnavailable)
        );
        assert!(
            detail
                .as_deref()
                .is_some_and(|detail| detail.contains("records no commissioning agent")),
            "the refusal says the schedule records no agent: {detail:?}"
        );
        assert_eq!(materialized, 0);
    }

    // --- An admitted input's outcome that cannot be read yet ---
    //
    // The host reads an admitted input's exact terminal completion when the
    // runtime returned no live handle. The read is injected, so these tests
    // script the runtime's answers: no finalized receipt yet (`Ok(None)`, the
    // answer a degraded session gives until its reload), a store read failure
    // (`Internal`), a read that never answers, or a final typed error.

    /// The misfire window the policy rows leave after the claim: a Skip row
    /// due 28 s ago, against Skip's fixed 30 s grace.
    const POLICY_ROW_MISFIRE_WINDOW: Duration = Duration::from_secs(2);

    /// A scripted exact terminal-completion read. `answer` gets the number of
    /// earlier calls and returns this call's answer; `None` is a read that
    /// never answers.
    fn scripted_outcome_read(
        calls: Arc<AtomicUsize>,
        answer: impl Fn(usize) -> Option<AdmittedOutcomeAnswer> + Send + Sync + 'static,
    ) -> AdmittedOutcomeReadFn {
        Box::new(move || {
            let answer = answer(calls.fetch_add(1, Ordering::SeqCst));
            Box::pin(async move {
                match answer {
                    Some(answer) => answer,
                    None => std::future::pending().await,
                }
            })
        })
    }

    #[test]
    fn admitted_outcome_reread_bounds_are_named_lease_periods() {
        let lease = Duration::from_secs(
            u64::try_from(SCHEDULE_HOST_LEASE_SECONDS).expect("the host lease is positive"),
        );
        assert_eq!(
            ADMITTED_OUTCOME_REREAD_BOUND,
            lease * 5,
            "the overall bound is five periods of the host lease"
        );
        assert_eq!(
            ADMITTED_OUTCOME_READ_BOUND,
            lease / 4,
            "one read is bounded at a quarter of the host lease"
        );
        assert_eq!(
            ADMITTED_OUTCOME_REREAD_FIRST_BACKOFF,
            Duration::from_secs(1)
        );
        assert_eq!(
            AdmittedOutcomeRereadBounds::HOST,
            AdmittedOutcomeRereadBounds {
                overall: ADMITTED_OUTCOME_REREAD_BOUND,
                per_read: ADMITTED_OUTCOME_READ_BOUND,
                first_backoff: ADMITTED_OUTCOME_REREAD_FIRST_BACKOFF,
            }
        );

        let test = AdmittedOutcomeRereadBounds::TEST;
        assert_eq!(AdmittedOutcomeRereadBounds::for_host(), test);
        assert!(
            test.per_read < TEST_SCHEDULE_DRIVER_LEASE,
            "a first read that outlived the claim lease would let it lapse before the wait starts"
        );
        assert!(
            test.overall > POLICY_ROW_MISFIRE_WINDOW + TEST_SCHEDULE_DRIVER_LEASE,
            "a wait that gave its lease up at the bound must reclaim past the misfire deadline"
        );
        assert!(test.first_backoff <= test.per_read);
    }

    /// Which completion one first answer of the read yields.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ExpectedAdmittedCompletion {
        /// The input's own terminal, at once.
        Terminal,
        /// The bounded re-read loop: the next read decides.
        Reread,
        /// The typed terminal at once.
        AuthorityUnavailable,
    }

    /// The table of the fallback read's answers. `Ok(None)`, `Internal` and a
    /// read that hits its bound enter the re-read loop (here the next read
    /// answers Completed); every other typed error ends authority-unavailable
    /// at once, and an exact receipt resolves at once.
    #[tokio::test]
    async fn admitted_outcome_read_maps_each_answer_to_its_completion() {
        use ExpectedAdmittedCompletion::{AuthorityUnavailable, Reread, Terminal};

        let input_id = InputId::new();
        let rows: Vec<(
            &str,
            Option<AdmittedOutcomeAnswer>,
            ExpectedAdmittedCompletion,
        )> = vec![
            (
                "an exact receipt",
                Some(Ok(Some(CompletionOutcome::CompletedWithoutResult))),
                Terminal,
            ),
            ("no finalized receipt yet", Some(Ok(None)), Reread),
            (
                "a stalled commit holds the degraded session's driver",
                Some(Err(RuntimeDriverError::InputTerminalNotYetReadable {
                    input_id: InputId::new(),
                    reason: "the terminal is not known yet".to_string(),
                })),
                Reread,
            ),
            (
                "a store read failure",
                Some(Err(RuntimeDriverError::Internal(
                    "database is locked".to_string(),
                ))),
                Reread,
            ),
            ("a read that hits its bound", None, Reread),
            (
                "a terminal without a receipt",
                Some(Err(RuntimeDriverError::InputTerminalWithoutReceipt {
                    input_id: input_id.clone(),
                    terminal: meerkat_runtime::InputTerminalOutcome::Consumed,
                })),
                AuthorityUnavailable,
            ),
            (
                "a repair-blocked row",
                Some(Err(RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: "terminal row without its recorded result".to_string(),
                })),
                AuthorityUnavailable,
            ),
            (
                "a corrupt row",
                Some(Err(RuntimeDriverError::RecoveryCorruption {
                    reason: "terminal row without a receipt".to_string(),
                })),
                AuthorityUnavailable,
            ),
            (
                "an unknown runtime",
                Some(Err(RuntimeDriverError::NotFound {
                    runtime_id: meerkat_runtime::LogicalRuntimeId("gone".to_string()),
                })),
                AuthorityUnavailable,
            ),
            (
                "a destroyed runtime",
                Some(Err(RuntimeDriverError::NotReady {
                    state: meerkat_runtime::RuntimeState::Destroyed,
                })),
                AuthorityUnavailable,
            ),
        ];

        let occurrence = sample_occurrence();
        for (row, first_answer, expected) in rows {
            let error_text = match &first_answer {
                Some(Err(error)) => Some(error.to_string()),
                _ => None,
            };
            let calls = Arc::new(AtomicUsize::new(0));
            let read = scripted_outcome_read(Arc::clone(&calls), move |call| {
                if call == 0 {
                    first_answer.clone()
                } else {
                    Some(Ok(Some(CompletionOutcome::CompletedWithoutResult)))
                }
            });
            let completion = admitted_input_completion(
                input_id.clone(),
                AdmittedOutcomeRereadBounds::for_host(),
                read,
            )
            .await;
            let yielded = match &completion {
                AcceptedScheduledInputCompletion::Terminal(_) => Terminal,
                AcceptedScheduledInputCompletion::Reread(_) => Reread,
                AcceptedScheduledInputCompletion::AuthorityUnavailable { .. } => {
                    AuthorityUnavailable
                }
                AcceptedScheduledInputCompletion::Handle { .. } => {
                    panic!("{row}: the fallback read never yields a live handle")
                }
            };
            assert_eq!(yielded, expected, "{row}");

            let accepted = AcceptedScheduledInput {
                correlation_id: Some("stable-correlation".to_string()),
                admission_outcome: ScheduleAdmissionOutcome::Deduplicated,
                completion,
            };
            let result = build_dispatch_from_accepted(&occurrence, accepted, None)
                .completion
                .await;
            match expected {
                Terminal | Reread => {
                    let terminal = result.unwrap_or_else(|error| {
                        panic!("{row}: the input's own outcome resolves it: {error}")
                    });
                    assert_eq!(
                        terminal.runtime_completion_outcome,
                        Some(meerkat_schedule::RuntimeCompletionOutcome::Completed),
                        "{row}"
                    );
                    let expected_calls = if expected == Terminal { 1 } else { 2 };
                    assert_eq!(calls.load(Ordering::SeqCst), expected_calls, "{row}");
                }
                AuthorityUnavailable => {
                    match result {
                        Err(ScheduleDomainError::DeliveryCompletionFailed { reason, detail }) => {
                            assert_eq!(
                                reason,
                                DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable,
                                "{row}"
                            );
                            let error_text = error_text.as_deref().unwrap_or_default();
                            assert!(
                                detail.contains(error_text),
                                "{row}: the detail carries the runtime's typed error: {detail}"
                            );
                        }
                        other => panic!("{row}: expected the typed terminal, got {other:?}"),
                    }
                    assert_eq!(calls.load(Ordering::SeqCst), 1, "{row}: no re-read");
                }
            }
        }
    }

    /// A delivery that admits each occurrence once, as the runtime does
    /// without a live completion handle. Its completion is the host's own
    /// admitted-outcome completion over the scripted read `read` builds.
    /// With `wait_failure` it admits with a live completion instead, whose
    /// wait fails with that error, and `read` reads the input's own terminal
    /// past it, as [`runtime_delivery_dispatch_from_admission`] wires it.
    struct ScriptedAdmissionDelivery {
        admission_outcome: ScheduleAdmissionOutcome,
        wait_failure: Option<CompletionWaitError>,
        read: Box<dyn Fn() -> AdmittedOutcomeReadFn + Send + Sync>,
        admissions: AtomicUsize,
    }

    #[async_trait]
    impl ScheduleTargetProbe for ScriptedAdmissionDelivery {
        async fn probe_target(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }
    }

    #[async_trait]
    impl ScheduleTargetDelivery for ScriptedAdmissionDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.admissions.fetch_add(1, Ordering::SeqCst);
            let completion = match self.wait_failure.clone() {
                Some(error) => AcceptedScheduledInputCompletion::Handle {
                    wait: Box::pin(async move { Err(error) }),
                    outcome_read: Some(AdmittedInputOutcomeRead {
                        input_id: InputId::new(),
                        bounds: AdmittedOutcomeRereadBounds::for_host(),
                        read: (self.read)(),
                        resumable: unregistered_runtime_wait_failure_resumable(),
                    }),
                },
                None => {
                    admitted_input_completion(
                        InputId::new(),
                        AdmittedOutcomeRereadBounds::for_host(),
                        (self.read)(),
                    )
                    .await
                }
            };
            Ok(build_dispatch_from_accepted(
                occurrence,
                AcceptedScheduledInput {
                    correlation_id: Some(identity.correlation_id.clone()),
                    admission_outcome: self.admission_outcome,
                    completion,
                },
                None,
            ))
        }
    }

    /// One occurrence driven to its end through a real [`ScheduleDriver`]
    /// with the test lease.
    struct AdmittedOutcomeRun {
        occurrence: Occurrence,
        reports: Vec<meerkat_schedule::ScheduleTickReport>,
        /// The occurrence's phase and attempt count after each tick.
        observed: Vec<(OccurrencePhase, u32)>,
        admissions: usize,
        elapsed: Duration,
    }

    fn occurrence_phase_is_terminal(phase: OccurrencePhase) -> bool {
        matches!(
            phase,
            OccurrencePhase::Completed
                | OccurrencePhase::Skipped
                | OccurrencePhase::Misfired
                | OccurrencePhase::Superseded
                | OccurrencePhase::DeliveryFailed
        )
    }

    /// Plan one occurrence due at `due_at_utc` under `misfire_policy` and drive
    /// it through a [`ScheduleDriver`] whose lease is
    /// [`TEST_SCHEDULE_DRIVER_LEASE`], ticking every 100 ms as the host loop
    /// does, until the occurrence is terminal. The run fails after 15 s.
    async fn run_admitted_outcome_occurrence(
        misfire_policy: meerkat_schedule::MisfirePolicy,
        due_at_utc: chrono::DateTime<chrono::Utc>,
        delivery: ScriptedAdmissionDelivery,
    ) -> AdmittedOutcomeRun {
        let store =
            Arc::new(meerkat_schedule::MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(meerkat_schedule::CreateScheduleRequest {
                name: Some("admitted-outcome".to_string()),
                description: None,
                trigger: meerkat_schedule::TriggerSpec::Once { due_at_utc },
                target: TargetBinding::session(SessionTargetBinding::ExactSession {
                    session_id: SessionId::new(),
                    action: ScheduledSessionAction::Prompt {
                        prompt: ContentInput::Text("scheduled prompt".to_string()),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                }),
                misfire_policy,
                overlap_policy: meerkat_schedule::OverlapPolicy::SkipIfRunning,
                missing_target_policy: meerkat_schedule::MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("create the schedule");
        let delivery = Arc::new(delivery);
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            delivery.clone(),
            delivery.clone(),
            "admitted-outcome-driver",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: ChronoDuration::from_std(TEST_SCHEDULE_DRIVER_LEASE)
                    .expect("test lease"),
            },
        );

        let started = std::time::Instant::now();
        let mut reports = Vec::new();
        let mut observed = Vec::new();
        loop {
            reports.push(driver.tick_once().await.expect("driver tick"));
            let occurrence = service
                .list_occurrences(&schedule.schedule_id)
                .await
                .expect("list occurrences")
                .into_iter()
                .next();
            if let Some(occurrence) = occurrence {
                observed.push((occurrence.phase, occurrence.attempt_count));
                if occurrence_phase_is_terminal(occurrence.phase) {
                    return AdmittedOutcomeRun {
                        occurrence,
                        reports,
                        observed,
                        admissions: delivery.admissions.load(Ordering::SeqCst),
                        elapsed: started.elapsed(),
                    };
                }
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "the occurrence never ended; observed {observed:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// What every run whose outcome stayed unreadable shows: one attempt,
    /// lease kept, no misfire, and the typed terminal at the bound.
    fn assert_ended_at_the_bound(run: &AdmittedOutcomeRun, cause: &str) {
        let bound = AdmittedOutcomeRereadBounds::for_host().overall;
        assert!(
            run.observed.iter().all(|(phase, _)| {
                !matches!(phase, OccurrencePhase::Misfired | OccurrencePhase::Skipped)
            }),
            "a proven admission never ends misfired or skipped: {:?}",
            run.observed
        );
        // A tick that only planned leaves a Pending row before the claim.
        let claimed: Vec<_> = run
            .observed
            .iter()
            .skip_while(|(phase, _)| *phase == OccurrencePhase::Pending)
            .collect();
        assert!(
            claimed.iter().all(|(_, attempt)| *attempt == 1),
            "the lease is renewed through the wait, so nothing reclaims: {:?}",
            run.observed
        );
        let waiting = &claimed[..claimed.len().saturating_sub(1)];
        assert!(
            waiting
                .iter()
                .all(|(phase, _)| *phase == OccurrencePhase::AwaitingCompletion),
            "the occurrence waits in AwaitingCompletion: {:?}",
            run.observed
        );
        assert!(
            run.reports.iter().all(|report| report.fault_count() == 0),
            "no tick reports a row fault, a refused lease-expiry reclaim included"
        );
        assert_eq!(run.admissions, 1, "one admission, no redelivery");
        assert!(run.elapsed >= bound, "it ends at the bound, not before");
        assert_eq!(run.occurrence.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            run.occurrence.failure_class,
            Some(OccurrenceFailureClass::InternalError),
            "runtime_completion_authority_unavailable records the internal class"
        );
        let detail = run.occurrence.failure_detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains(&format!("terminal unreadable for {bound:?}"))
                && detail.contains(cause),
            "the detail names the bound and the last cause: {detail}"
        );
    }

    /// Not readable yet on every read (the answer while the session's
    /// degradation holds). The occurrence keeps its lease and ends typed at
    /// the bound.
    fn always_not_yet_readable() -> ScriptedAdmissionDelivery {
        ScriptedAdmissionDelivery {
            admission_outcome: ScheduleAdmissionOutcome::Deduplicated,
            wait_failure: None,
            read: Box::new(|| {
                scripted_outcome_read(Arc::new(AtomicUsize::new(0)), |_| Some(Ok(None)))
            }),
            admissions: AtomicUsize::new(0),
        }
    }

    /// The policy row under Skip: the misfire deadline falls inside the wait,
    /// which outlasts it. A wait that gave its lease up at the bound would be
    /// reclaimed past the deadline and end misfired with a row fault.
    #[tokio::test]
    async fn unreadable_outcome_under_skip_never_misfires_and_ends_typed_at_the_bound() {
        let due_at_utc = chrono::Utc::now() - ChronoDuration::seconds(30)
            + ChronoDuration::from_std(POLICY_ROW_MISFIRE_WINDOW).expect("window");
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            due_at_utc,
            always_not_yet_readable(),
        )
        .await;
        assert_ended_at_the_bound(&run, "no finalized receipt yet");
    }

    /// The policy row under a one-second catch-up window.
    #[tokio::test]
    async fn unreadable_outcome_under_catch_up_within_never_misfires_and_ends_typed_at_the_bound() {
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::CatchUpWithin { window_seconds: 1 },
            chrono::Utc::now() - ChronoDuration::milliseconds(200),
            always_not_yet_readable(),
        )
        .await;
        assert_ended_at_the_bound(&run, "no finalized receipt yet");
    }

    /// A deduplicated admission whose session degraded before the outcome
    /// read: the outcome becomes readable once the degradation clears (its
    /// reload settles the receipt), before the bound. The occurrence resolves
    /// from the input's own outcome on its first attempt.
    #[tokio::test]
    async fn deduplicated_admission_resolves_once_its_degradation_clears_before_the_bound() {
        let degraded = Arc::new(AtomicBool::new(true));
        let clearing = Arc::clone(&degraded);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            clearing.store(false, Ordering::SeqCst);
        });
        let read_degraded = Arc::clone(&degraded);
        let delivery = ScriptedAdmissionDelivery {
            admission_outcome: ScheduleAdmissionOutcome::Deduplicated,
            wait_failure: None,
            read: Box::new(move || {
                let degraded = Arc::clone(&read_degraded);
                scripted_outcome_read(Arc::new(AtomicUsize::new(0)), move |_| {
                    if degraded.load(Ordering::SeqCst) {
                        Some(Ok(None))
                    } else {
                        Some(Ok(Some(CompletionOutcome::CompletedWithoutResult)))
                    }
                })
            }),
            admissions: AtomicUsize::new(0),
        };
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            chrono::Utc::now() - ChronoDuration::seconds(1),
            delivery,
        )
        .await;
        assert_eq!(run.occurrence.phase, OccurrencePhase::Completed);
        assert_eq!(run.occurrence.failure_class, None);
        assert_eq!(run.occurrence.attempt_count, 1);
        assert_eq!(run.admissions, 1);
        assert!(
            run.elapsed < AdmittedOutcomeRereadBounds::for_host().overall,
            "it resolves before the bound"
        );
    }

    /// The same degradation held past the bound: the typed terminal.
    #[tokio::test]
    async fn deduplicated_admission_whose_degradation_outlasts_the_bound_ends_typed() {
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            chrono::Utc::now() - ChronoDuration::seconds(1),
            always_not_yet_readable(),
        )
        .await;
        assert_ended_at_the_bound(&run, "no finalized receipt yet");
    }

    /// The accepted path: an accepted input already terminal at admission
    /// whose outcome read answers no finalized receipt yet. Before the
    /// bounded wait it ended delivery_failed at once; now it waits like a
    /// deduplicated admission, under the same policy row.
    #[tokio::test]
    async fn accepted_input_terminal_at_admission_waits_for_its_receipt_under_the_bound() {
        let due_at_utc = chrono::Utc::now() - ChronoDuration::seconds(30)
            + ChronoDuration::from_std(POLICY_ROW_MISFIRE_WINDOW).expect("window");
        let mut delivery = always_not_yet_readable();
        delivery.admission_outcome = ScheduleAdmissionOutcome::Accepted;
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            due_at_utc,
            delivery,
        )
        .await;
        assert_ended_at_the_bound(&run, "no finalized receipt yet");
    }

    /// A store read failure on every read: not readable yet, never final on
    /// its own. The bound ends it, and the detail carries the store's text.
    #[tokio::test]
    async fn store_read_failure_on_every_read_ends_typed_at_the_bound() {
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reads);
        let delivery = ScriptedAdmissionDelivery {
            admission_outcome: ScheduleAdmissionOutcome::Deduplicated,
            wait_failure: None,
            read: Box::new(move || {
                scripted_outcome_read(Arc::clone(&counted), |_| {
                    Some(Err(RuntimeDriverError::Internal(
                        "database is locked".to_string(),
                    )))
                })
            }),
            admissions: AtomicUsize::new(0),
        };
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            chrono::Utc::now() - ChronoDuration::seconds(1),
            delivery,
        )
        .await;
        assert_ended_at_the_bound(&run, "database is locked");
        assert!(
            reads.load(Ordering::SeqCst) > 2,
            "the loop re-reads a store read failure"
        );
    }

    /// The control: a store read failure on the first three reads, then the
    /// input's exact receipt before the bound. The occurrence completes from
    /// it.
    #[tokio::test]
    async fn store_read_failures_then_a_receipt_complete_from_the_input() {
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reads);
        let delivery = ScriptedAdmissionDelivery {
            admission_outcome: ScheduleAdmissionOutcome::Deduplicated,
            wait_failure: None,
            read: Box::new(move || {
                scripted_outcome_read(Arc::clone(&counted), |call| {
                    if call < 3 {
                        Some(Err(RuntimeDriverError::Internal(
                            "database is locked".to_string(),
                        )))
                    } else {
                        Some(Ok(Some(CompletionOutcome::CompletedWithoutResult)))
                    }
                })
            }),
            admissions: AtomicUsize::new(0),
        };
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            chrono::Utc::now() - ChronoDuration::seconds(1),
            delivery,
        )
        .await;
        assert_eq!(run.occurrence.phase, OccurrencePhase::Completed);
        assert_eq!(run.occurrence.failure_class, None);
        assert_eq!(run.occurrence.attempt_count, 1);
        assert_eq!(reads.load(Ordering::SeqCst), 4);
    }

    // --- A live wait that fails without saying the input's outcome ---
    //
    // A run whose boundary commit rolled back (`BoundaryRolledBack`) or whose
    // durable outcome is unknown (`DurableOutcomeUnknown`) fails its live
    // completion waiters. The input stays admitted and reaches its own
    // terminal after its session reloads. A graceful stop that kept a queued
    // input (`RequeuedAcrossStop`) leaves it admitted the same way, until the
    // next start runs it. These tests script the failed wait and the reads of
    // the input's terminal that follow it.

    fn rolled_back_boundary() -> CompletionWaitError {
        CompletionWaitError::BoundaryRolledBack {
            run_id: meerkat_core::lifecycle::RunId::new(),
            detail: "the store rejected the boundary commit".to_string(),
        }
    }

    fn unknown_durable_outcome() -> CompletionWaitError {
        CompletionWaitError::DurableOutcomeUnknown {
            run_id: meerkat_core::lifecycle::RunId::new(),
        }
    }

    fn requeued_across_stop() -> CompletionWaitError {
        CompletionWaitError::RequeuedAcrossStop {
            stopped_at: chrono::Utc::now(),
        }
    }

    /// The production resolution ([`runtime_wait_failure_resumable`]) on a
    /// runtime that holds no session: only a failure the generated authority
    /// resolves without the session, a graceful stop's requeue, is
    /// resumable.
    fn unregistered_runtime_wait_failure_resumable() -> AdmittedWaitResumableFn {
        runtime_wait_failure_resumable(&MeerkatMachine::ephemeral(), &SessionId::new())
    }

    /// Which wait failures read the input's own terminal: a rolled-back
    /// boundary, an unknown durable outcome and a failure the generated
    /// wait-failure authority resolves as resumable (a graceful stop's
    /// requeue), when the host knows the input. Every other wait failure, and a handle
    /// built without its input (`AcceptedScheduledInput::with_runtime_handle`),
    /// ends the delivery with the wait failure at once, with no read.
    #[tokio::test]
    async fn only_a_wait_that_leaves_the_input_admitted_reads_its_terminal() {
        use DeliveryCompletionFailureReason::{
            RuntimeCompletionAuthorityUnavailable, RuntimeCompletionChannelClosed,
        };

        let rows: Vec<(
            &str,
            CompletionWaitError,
            bool,
            Option<DeliveryCompletionFailureReason>,
        )> = vec![
            ("a rolled-back boundary", rolled_back_boundary(), true, None),
            (
                "an unknown durable outcome",
                unknown_durable_outcome(),
                true,
                None,
            ),
            (
                "a rolled-back boundary without its input",
                rolled_back_boundary(),
                false,
                Some(RuntimeCompletionAuthorityUnavailable),
            ),
            (
                "an unknown durable outcome without its input",
                unknown_durable_outcome(),
                false,
                Some(RuntimeCompletionAuthorityUnavailable),
            ),
            (
                "a requeue across a graceful stop",
                requeued_across_stop(),
                true,
                None,
            ),
            (
                "a requeue across a graceful stop without its input",
                requeued_across_stop(),
                false,
                Some(RuntimeCompletionAuthorityUnavailable),
            ),
            (
                "a closed channel",
                CompletionWaitError::ChannelClosed,
                true,
                Some(RuntimeCompletionChannelClosed),
            ),
            (
                "a replaced attachment",
                CompletionWaitError::AttachmentReplaced,
                true,
                Some(RuntimeCompletionAuthorityUnavailable),
            ),
            (
                "unavailable authority",
                CompletionWaitError::AuthorityUnavailable("no completion authority".to_string()),
                true,
                Some(RuntimeCompletionAuthorityUnavailable),
            ),
        ];

        let occurrence = sample_occurrence();
        for (row, error, knows_input, immediate) in rows {
            let calls = Arc::new(AtomicUsize::new(0));
            let outcome_read = knows_input.then(|| AdmittedInputOutcomeRead {
                input_id: InputId::new(),
                bounds: AdmittedOutcomeRereadBounds::for_host(),
                read: scripted_outcome_read(Arc::clone(&calls), |_| {
                    Some(Ok(Some(CompletionOutcome::CompletedWithoutResult)))
                }),
                resumable: unregistered_runtime_wait_failure_resumable(),
            });
            let wait_error = error.clone();
            let accepted = AcceptedScheduledInput {
                correlation_id: Some("stable-correlation".to_string()),
                admission_outcome: ScheduleAdmissionOutcome::Accepted,
                completion: AcceptedScheduledInputCompletion::Handle {
                    wait: Box::pin(async move { Err(wait_error) }),
                    outcome_read,
                },
            };
            let result = build_dispatch_from_accepted(&occurrence, accepted, None)
                .completion
                .await;
            match immediate {
                None => {
                    let terminal = result.unwrap_or_else(|failure| {
                        panic!("{row}: the input's own terminal resolves it: {failure}")
                    });
                    assert_eq!(
                        terminal.runtime_completion_outcome,
                        Some(meerkat_schedule::RuntimeCompletionOutcome::Completed),
                        "{row}"
                    );
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        1,
                        "{row}: one read after the failed wait"
                    );
                }
                Some(expected_reason) => {
                    match result {
                        Err(ScheduleDomainError::DeliveryCompletionFailed { reason, detail }) => {
                            assert_eq!(reason, expected_reason, "{row}");
                            assert!(
                                detail.contains(&error.to_string()),
                                "{row}: the detail names the wait failure: {detail}"
                            );
                        }
                        other => panic!("{row}: expected the wait failure, got {other:?}"),
                    }
                    assert_eq!(calls.load(Ordering::SeqCst), 0, "{row}: no read");
                }
            }
        }
    }

    /// A delivery admitted with a live completion whose wait fails with
    /// `wait_failure`; the reads of the input's terminal that follow answer
    /// `answer`, which gets the number of earlier reads.
    fn failed_wait_delivery(
        wait_failure: CompletionWaitError,
        reads: Arc<AtomicUsize>,
        answer: impl Fn(usize) -> Option<AdmittedOutcomeAnswer> + Clone + Send + Sync + 'static,
    ) -> ScriptedAdmissionDelivery {
        ScriptedAdmissionDelivery {
            admission_outcome: ScheduleAdmissionOutcome::Accepted,
            wait_failure: Some(wait_failure),
            read: Box::new(move || scripted_outcome_read(Arc::clone(&reads), answer.clone())),
            admissions: AtomicUsize::new(0),
        }
    }

    /// No proven terminal on the first two reads (the session reloads and the
    /// input runs again), then `terminal`.
    fn terminal_after_the_reload(
        terminal: CompletionOutcome,
    ) -> impl Fn(usize) -> Option<AdmittedOutcomeAnswer> + Clone + Send + Sync + 'static {
        move |read| {
            if read < 2 {
                Some(Ok(None))
            } else {
                Some(Ok(Some(terminal.clone())))
            }
        }
    }

    /// A scheduled delivery whose run's boundary commit was rejected.
    /// The input stays admitted, runs again after its session reloads and
    /// fails there. The occurrence ends with the input's own failed terminal
    /// (runtime_rejected, with the input's detail), never with the wait
    /// failure (internal_error, naming the rolled-back boundary).
    #[tokio::test]
    async fn rolled_back_boundary_ends_with_the_inputs_later_failed_terminal() {
        let reads = Arc::new(AtomicUsize::new(0));
        let delivery = failed_wait_delivery(
            rolled_back_boundary(),
            Arc::clone(&reads),
            terminal_after_the_reload(CompletionOutcome::Abandoned {
                reason: "the input failed after the reload".to_string(),
                error: meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                    "the input failed after the reload",
                ),
            }),
        );
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            chrono::Utc::now() - ChronoDuration::seconds(1),
            delivery,
        )
        .await;
        assert_eq!(run.occurrence.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            run.occurrence.failure_class,
            Some(OccurrenceFailureClass::RuntimeRejected),
            "the input's own failure, not the wait failure's internal_error"
        );
        let detail = run.occurrence.failure_detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains("the input failed after the reload"),
            "the detail is the input's own: {detail}"
        );
        assert!(
            !detail.contains("rolled back"),
            "the wait failure is not the occurrence's outcome: {detail}"
        );
        assert_eq!(run.occurrence.attempt_count, 1);
        assert_eq!(run.admissions, 1);
        assert_eq!(reads.load(Ordering::SeqCst), 3);
    }

    /// The same, and the input succeeds after the reload: the occurrence
    /// completes.
    #[tokio::test]
    async fn rolled_back_boundary_ends_completed_when_the_input_later_succeeds() {
        let reads = Arc::new(AtomicUsize::new(0));
        let delivery = failed_wait_delivery(
            rolled_back_boundary(),
            Arc::clone(&reads),
            terminal_after_the_reload(CompletionOutcome::CompletedWithoutResult),
        );
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            chrono::Utc::now() - ChronoDuration::seconds(1),
            delivery,
        )
        .await;
        assert_eq!(run.occurrence.phase, OccurrencePhase::Completed);
        assert_eq!(run.occurrence.failure_class, None);
        assert_eq!(run.occurrence.attempt_count, 1);
        assert_eq!(run.admissions, 1);
        assert_eq!(reads.load(Ordering::SeqCst), 3);
    }

    /// An unknown durable outcome: the commit past its deadline still
    /// holds the degraded session's driver, so the reads answer
    /// InputTerminalNotYetReadable until the store answers and the reload
    /// settles the run; then the input's terminal resolves the occurrence.
    #[tokio::test]
    async fn unknown_durable_outcome_ends_with_the_inputs_terminal_once_readable() {
        let reads = Arc::new(AtomicUsize::new(0));
        let delivery =
            failed_wait_delivery(unknown_durable_outcome(), Arc::clone(&reads), |read| {
                if read < 2 {
                    Some(Err(RuntimeDriverError::InputTerminalNotYetReadable {
                        input_id: InputId::new(),
                        reason: "the terminal is not known yet".to_string(),
                    }))
                } else {
                    Some(Ok(Some(CompletionOutcome::CompletedWithoutResult)))
                }
            });
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            chrono::Utc::now() - ChronoDuration::seconds(1),
            delivery,
        )
        .await;
        assert_eq!(run.occurrence.phase, OccurrencePhase::Completed);
        assert_eq!(run.occurrence.failure_class, None);
        assert_eq!(run.occurrence.attempt_count, 1);
        assert_eq!(reads.load(Ordering::SeqCst), 3);
    }

    /// At the bound: a rolled-back boundary whose session no traffic
    /// reloads within the bound. The input's terminal never becomes
    /// readable, and the occurrence ends with the loop's typed end, under
    /// the Skip policy row: never misfired, one attempt, no row fault.
    #[tokio::test]
    async fn rolled_back_boundary_whose_input_stays_unreadable_ends_typed_at_the_bound() {
        let due_at_utc = chrono::Utc::now() - ChronoDuration::seconds(30)
            + ChronoDuration::from_std(POLICY_ROW_MISFIRE_WINDOW).expect("window");
        let delivery = failed_wait_delivery(
            rolled_back_boundary(),
            Arc::new(AtomicUsize::new(0)),
            |_| Some(Ok(None)),
        );
        let run = run_admitted_outcome_occurrence(
            meerkat_schedule::MisfirePolicy::Skip,
            due_at_utc,
            delivery,
        )
        .await;
        assert_ended_at_the_bound(&run, "no finalized receipt yet");
    }
}
