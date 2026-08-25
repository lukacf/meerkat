//! Shared provider-neutral coordinator for experimental GPT Live delegation.
//!
//! Provider mechanics only emit typed observations and deliver authorized
//! context appends. Generated MeerkatMachine effects own every semantic edge;
//! MobMachine and the delegation service realize the worker lifecycle.

use std::sync::Arc;

use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveControlObservation, ExperimentalGptLiveControlPlane,
    ExperimentalGptLiveResultDeliveryDispatch,
};
use meerkat_core::exact_operation::ExactOperationIdentity;
use meerkat_core::ops::OperationId;
use meerkat_core::{
    FinalLiveUserTranscriptCommitEvidence, LiveHandoffInputProvenance, LiveHandoffReconciliation,
    LiveUserTurnCorrelation, OpaqueProviderCorrelation, ProvisionalLiveHandoff, SessionId,
};
use meerkat_live::{
    LiveSidebandDelegationRef, LiveSidebandObservation, LiveSidebandObservationKind,
    LiveSidebandTurnRef, ProviderWebrtcBinding,
};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, DelegationCancellationHandle, DelegationExecutionRequest,
    DelegationExecutionService, DelegationTerminalizedExecution, DelegationTurnTerminal,
};
use meerkat_runtime::live_execution::{
    LiveDelegationCancellationDirective, LiveDelegationCancellationOutcome,
    LiveDelegationExecutionAdmission, LiveDelegationResultDeliveryAuthority,
    LiveDelegationResultDeliveryResolution, LiveDelegationResultReleaseAuthority,
    LiveDelegationWorkerTerminalKind, LiveHandoffReconciliationReceipt,
};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const LIVE_DELEGATION_RESULT_BYTES: usize = 16 * 1024;

type ActiveChannelKey = (SessionId, meerkat_core::LiveChannelId);

struct ActiveDelegation {
    retained: Arc<RetainedDelegation>,
    cancellation: DelegationCancellationHandle,
    task: JoinHandle<()>,
}

struct RetainedDelegation {
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    provisional: ProvisionalLiveHandoff,
    runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    admission: LiveDelegationExecutionAdmission,
    delegation: LiveSidebandDelegationRef,
    control: Arc<dyn ExperimentalGptLiveControlPlane>,
    result: Mutex<RetainedDelegationResult>,
}

#[derive(Default)]
struct RetainedDelegationResult {
    reconciliation: Option<LiveHandoffReconciliationReceipt>,
    result_text: Option<String>,
    release_authority: Option<LiveDelegationResultReleaseAuthority>,
    delivery_authority: Option<LiveDelegationResultDeliveryAuthority>,
    terminal_ineligible: bool,
    delivery_in_flight: bool,
}

struct ActiveProviderTurn {
    authority: meerkat_runtime::meerkat_machine::LiveProviderTurnStartedAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundChannelPhase {
    Prepared,
    Running,
    StopRequested,
    Stopped,
}

struct BoundChannelCustody {
    binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    cancellation: CancellationToken,
    completion: Arc<tokio::sync::Notify>,
    phase: BoundChannelPhase,
}

type BoundChannelMap = Arc<Mutex<std::collections::HashMap<ActiveChannelKey, BoundChannelCustody>>>;

fn provider_binding_from_runtime(
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> ProviderWebrtcBinding {
    ProviderWebrtcBinding::new(
        binding.channel_id().clone(),
        binding.session_id().clone(),
        meerkat_live::LiveRuntimeBindingGeneration::new(binding.generation()),
        meerkat_live::LiveRuntimeBindingFence::new(binding.fence_token()),
    )
}

async fn reserve_bound_channel(
    bound_channels: &BoundChannelMap,
    binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> Result<(), String> {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let mut channels = bound_channels.lock().await;
    if let Some(existing) = channels.get(&key) {
        if existing.binding != binding {
            return Err(
                "experimental live channel retains a different runtime incarnation".to_string(),
            );
        }
        if existing.phase != BoundChannelPhase::Stopped {
            return Err("experimental live control binding is already prepared".to_string());
        }
    }
    channels.insert(
        key,
        BoundChannelCustody {
            binding,
            cancellation: CancellationToken::new(),
            completion: Arc::new(tokio::sync::Notify::new()),
            phase: BoundChannelPhase::Prepared,
        },
    );
    Ok(())
}

async fn begin_bound_channel_run(
    bound_channels: &BoundChannelMap,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> Option<CancellationToken> {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let mut channels = bound_channels.lock().await;
    let custody = channels.get_mut(&key)?;
    if custody.binding != *binding || custody.phase != BoundChannelPhase::Prepared {
        return None;
    }
    custody.phase = BoundChannelPhase::Running;
    Some(custody.cancellation.clone())
}

async fn finish_bound_channel_run(
    bound_channels: &BoundChannelMap,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let completion = {
        let mut channels = bound_channels.lock().await;
        let Some(custody) = channels.get_mut(&key) else {
            return;
        };
        if custody.binding != *binding {
            return;
        }
        custody.phase = BoundChannelPhase::Stopped;
        Arc::clone(&custody.completion)
    };
    completion.notify_waiters();
}

async fn release_bound_channel(
    bound_channels: &BoundChannelMap,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> Result<(), String> {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let (cancellation, completion) = {
        let mut channels = bound_channels.lock().await;
        let Some(custody) = channels.get_mut(&key) else {
            return Ok(());
        };
        if custody.binding != *binding {
            return Err(
                "experimental live deactivation does not match the bound runtime incarnation"
                    .to_string(),
            );
        }
        match custody.phase {
            BoundChannelPhase::Prepared => {
                channels.remove(&key);
                return Ok(());
            }
            BoundChannelPhase::Running => custody.phase = BoundChannelPhase::StopRequested,
            BoundChannelPhase::StopRequested => {}
            BoundChannelPhase::Stopped => {
                channels.remove(&key);
                return Ok(());
            }
        }
        (
            custody.cancellation.clone(),
            Arc::clone(&custody.completion),
        )
    };
    cancellation.cancel();
    loop {
        let completed = completion.notified();
        tokio::pin!(completed);
        completed.as_mut().enable();
        let stopped = bound_channels.lock().await.get(&key).is_none_or(|custody| {
            custody.binding == *binding && custody.phase == BoundChannelPhase::Stopped
        });
        if stopped {
            break;
        }
        completed.await;
    }
    let mut channels = bound_channels.lock().await;
    if channels.get(&key).is_some_and(|custody| {
        custody.binding == *binding && custody.phase == BoundChannelPhase::Stopped
    }) {
        channels.remove(&key);
    }
    Ok(())
}

/// One coordinator per RPC host. Channel actors are fenced by the exact
/// provider binding and retain at most one generated delegation per channel.
#[derive(Clone)]
pub struct ExperimentalLiveDelegationCoordinator {
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
    active: Arc<Mutex<std::collections::HashMap<ActiveChannelKey, ActiveDelegation>>>,
    retained: Arc<Mutex<std::collections::HashMap<OperationId, Arc<RetainedDelegation>>>>,
    active_turns: Arc<Mutex<std::collections::HashMap<ActiveChannelKey, ActiveProviderTurn>>>,
    bound_channels: BoundChannelMap,
}

/// Compose the one shared live-delegation lifecycle owner used by RPC and
/// MobKit hosts. The returned typed handle is also the provider binder's
/// erased `ExperimentalLiveBoundChannelActivator`.
#[must_use]
pub fn compose_experimental_live_delegation_coordinator(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
) -> Arc<ExperimentalLiveDelegationCoordinator> {
    Arc::new(ExperimentalLiveDelegationCoordinator::new(runtime, mobs))
}

impl ExperimentalLiveDelegationCoordinator {
    pub fn new(
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
        mobs: Arc<crate::MobMcpState>,
    ) -> Self {
        Self {
            runtime,
            mobs,
            active: Arc::new(Mutex::new(std::collections::HashMap::new())),
            retained: Arc::new(Mutex::new(std::collections::HashMap::new())),
            active_turns: Arc::new(Mutex::new(std::collections::HashMap::new())),
            bound_channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    async fn prepare_bound_channel(
        &self,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) -> Result<(), String> {
        control
            .active_binding(runtime_binding.session_id())
            .await
            .filter(|binding| {
                binding.channel_id() == runtime_binding.channel_id()
                    && binding.runtime_generation().get() == runtime_binding.generation()
                    && binding.runtime_fence().get() == runtime_binding.fence_token()
            })
            .ok_or_else(|| "experimental live control binding is unavailable".to_string())?;
        reserve_bound_channel(&self.bound_channels, runtime_binding).await
    }

    async fn run_bound_channel(
        &self,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) {
        let Some(cancellation) =
            begin_bound_channel_run(&self.bound_channels, &runtime_binding).await
        else {
            return;
        };
        let binding = control
            .active_binding(runtime_binding.session_id())
            .await
            .filter(|binding| {
                binding.channel_id() == runtime_binding.channel_id()
                    && binding.runtime_generation().get() == runtime_binding.generation()
                    && binding.runtime_fence().get() == runtime_binding.fence_token()
            });
        if let Some(binding) = binding {
            self.run_channel(binding, control, cancellation).await;
        } else {
            self.cancel_channel_binding(&provider_binding_from_runtime(&runtime_binding))
                .await;
        }
        finish_bound_channel_run(&self.bound_channels, &runtime_binding).await;
    }

    async fn deactivate_bound_channel(
        &self,
        runtime_binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), String> {
        release_bound_channel(&self.bound_channels, runtime_binding).await?;
        self.cancel_channel_binding(&provider_binding_from_runtime(runtime_binding))
            .await;
        Ok(())
    }

    /// Candidate-only projection used by the direct Gate0 harness to ask the
    /// SessionDocument owner to seal the provider-final transcript for the
    /// exact provisional handoff already admitted by this coordinator.
    ///
    /// This returns no worker, tool, release, or provider authority. The
    /// caller must still obtain sealed final evidence from the session owner
    /// and return it through `reconcile_exact_final`.
    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[doc(hidden)]
    pub async fn __gate0_candidate_provisional(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_core::LiveChannelId,
        turn_adapter_key: &str,
    ) -> Result<ProvisionalLiveHandoff, String> {
        if turn_adapter_key.trim().is_empty() {
            return Err("Gate0 final turn key is empty".to_string());
        }
        self.retained
            .lock()
            .await
            .values()
            .find(|retained| {
                retained.runtime_binding.session_id() == session_id
                    && retained.runtime_binding.channel_id() == channel_id
                    && retained.provisional.correlation().provider().user_turn_id()
                        == turn_adapter_key
            })
            .map(|retained| retained.provisional.clone())
            .ok_or_else(|| "Gate0 final turn has no exact admitted provisional handoff".to_string())
    }

    async fn run_channel(
        &self,
        binding: ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        cancellation: CancellationToken,
    ) {
        loop {
            let observation = match tokio::select! {
                biased;
                () = cancellation.cancelled() => break,
                observation = control.next_observation(&binding) => observation,
            } {
                Ok(Some(observation)) => observation,
                Ok(None) | Err(_) => break,
            };
            match observation {
                ExperimentalGptLiveControlObservation::Provider(observation) => {
                    if observation.binding() != &binding {
                        break;
                    }
                    if let LiveSidebandObservationKind::DelegationRequested {
                        turn,
                        delegation,
                        actionable_input,
                    } = observation.kind()
                    {
                        if let Err(error) = self
                            .start_delegation(
                                &binding,
                                Arc::clone(&control),
                                turn.clone(),
                                delegation.clone(),
                                actionable_input.clone(),
                            )
                            .await
                        {
                            tracing::warn!(
                                error,
                                "experimental live delegation start failed closed"
                            );
                        }
                    }
                }
                ExperimentalGptLiveControlObservation::AppendResolved(resolution) => {
                    let (authority, outcome) = resolution.into_parts();
                    let runtime_binding = match self
                        .runtime
                        .live_delegation_runtime_binding(
                            authority.session_id(),
                            authority.channel_id(),
                        )
                        .await
                    {
                        Ok(binding) => binding,
                        Err(error) => {
                            tracing::warn!(%error, "live append resolution lost runtime binding");
                            continue;
                        }
                    };
                    if let Err(error) = self
                        .runtime
                        .resolve_live_context_append(
                            runtime_binding.runtime_id(),
                            runtime_binding.fence_token(),
                            runtime_binding.generation(),
                            &authority,
                            outcome,
                        )
                        .await
                    {
                        tracing::warn!(%error, "generated live append resolution failed");
                    }
                }
                ExperimentalGptLiveControlObservation::ResultDeliveryResolved(resolution) => {
                    let (authority, observation) = resolution.into_parts();
                    let operation_id = authority.operation().operation_id().clone();
                    match self
                        .runtime
                        .resolve_live_delegation_result_delivery(&authority, observation)
                        .await
                    {
                        Ok(LiveDelegationResultDeliveryResolution::Resolved(receipt))
                            if !receipt.retry_allowed() =>
                        {
                            let retained = self.retained.lock().await.get(&operation_id).cloned();
                            if let Some(retained) = retained {
                                self.remove_retained_delegation(&retained).await;
                            }
                        }
                        Ok(LiveDelegationResultDeliveryResolution::AmbiguityRecovery(recovery)) => {
                            match self
                                .runtime
                                .realize_live_delegation_result_ambiguity_recovery(recovery)
                                .await
                            {
                                Ok(()) => {
                                    let retained =
                                        self.retained.lock().await.get(&operation_id).cloned();
                                    if let Some(retained) = retained {
                                        self.remove_retained_delegation(&retained).await;
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!(%error, %operation_id, "generated fallback result recovery handoff failed");
                                }
                            }
                        }
                        Ok(LiveDelegationResultDeliveryResolution::Resolved(_)) => {
                            tracing::warn!(
                                %operation_id,
                                "generated fallback result delivery unexpectedly allowed retry"
                            );
                        }
                        Err(error) => {
                            tracing::warn!(%error, %operation_id, "generated fallback result delivery resolution failed");
                        }
                    }
                }
            }
        }
        self.cancel_channel_binding(&binding).await;
    }

    async fn observe_provider_lifecycle(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        let key = (
            observation.binding().session_id().clone(),
            observation.binding().channel_id().clone(),
        );
        let bound = self
            .bound_channels
            .lock()
            .await
            .get(&key)
            .is_some_and(|custody| {
                custody.binding.session_id() == observation.binding().session_id()
                    && custody.binding.channel_id() == observation.binding().channel_id()
                    && custody.binding.generation()
                        == observation.binding().runtime_generation().get()
                    && custody.binding.fence_token() == observation.binding().runtime_fence().get()
                    && matches!(
                        custody.phase,
                        BoundChannelPhase::Prepared | BoundChannelPhase::Running
                    )
            });
        if !bound {
            return Err(
                "provider lifecycle observation has no exact running channel custody".to_string(),
            );
        }
        match observation.kind() {
            LiveSidebandObservationKind::TurnStarted {
                role: meerkat_live::LiveSidebandTurnRole::User,
                ..
            } => self.observe_turn_started(observation).await,
            LiveSidebandObservationKind::TurnFinished {
                role: meerkat_live::LiveSidebandTurnRole::User,
                ..
            } => self.observe_turn_finished(observation).await,
            LiveSidebandObservationKind::TurnStarted { .. }
            | LiveSidebandObservationKind::TurnFinished { .. }
            | LiveSidebandObservationKind::TurnSnapshotDelta { .. } => Ok(()),
            _ => Err("provider lifecycle seam received a non-lifecycle observation".to_string()),
        }
    }

    async fn observe_turn_started(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        let authority = self
            .runtime
            .observe_live_provider_turn_started(observation)
            .await
            .map_err(|error| error.to_string())?;
        let key = (
            authority.binding().session_id().clone(),
            authority.binding().channel_id().clone(),
        );
        let mut turns = self.active_turns.lock().await;
        if turns.contains_key(&key) {
            return Err(
                "provider turn start duplicated an active local turn projection".to_string(),
            );
        }
        turns.insert(key, ActiveProviderTurn { authority });
        Ok(())
    }

    async fn observe_turn_finished(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        let finished = self
            .runtime
            .observe_live_provider_turn_finished(observation)
            .await
            .map_err(|error| error.to_string())?;
        let key = (
            finished.binding().session_id().clone(),
            finished.binding().channel_id().clone(),
        );
        let started = self.active_turns.lock().await.remove(&key).ok_or_else(|| {
            "provider turn finish has no local started-turn projection".to_string()
        })?;
        if started.authority.binding() != finished.binding()
            || started.authority.interaction_id() != finished.interaction_id()
            || started.authority.provider_turn_ref() != finished.provider_turn_ref()
        {
            return Err("provider turn finish does not match the exact started turn".to_string());
        }
        self.runtime
            .drain_live_context_outbox(finished.binding().session_id())
            .await
            .map_err(|error| error.to_string())
    }

    async fn start_delegation(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        turn: LiveSidebandTurnRef,
        delegation: LiveSidebandDelegationRef,
        actionable_input: String,
    ) -> Result<(), String> {
        let session_id = provider_binding.session_id();
        let channel_key = (session_id.clone(), provider_binding.channel_id().clone());
        let turn_authority = self
            .active_turns
            .lock()
            .await
            .get(&channel_key)
            .map(|turn| turn.authority.clone())
            .ok_or_else(|| "delegation has no exact active provider turn".to_string())?;
        if turn_authority.binding().channel_id() != provider_binding.channel_id()
            || turn_authority.binding().session_id() != session_id
            || turn_authority.provider_turn_ref() != turn.adapter_key()
        {
            return Err("delegation turn ref does not match the active generated turn".to_string());
        }
        let provider_correlation =
            OpaqueProviderCorrelation::new(delegation.adapter_key(), turn.adapter_key())
                .map_err(|error| error.to_string())?;
        let correlation = LiveUserTurnCorrelation::new(
            provider_binding.channel_id().clone(),
            turn_authority.interaction_id(),
            provider_correlation,
        )
        .map_err(|error| error.to_string())?;
        let runtime_binding = self
            .runtime
            .live_delegation_runtime_binding(session_id, correlation.channel_id())
            .await
            .map_err(|error| error.to_string())?;
        if runtime_binding.fence_token() != provider_binding.runtime_fence().get()
            || runtime_binding.generation() != provider_binding.runtime_generation().get()
        {
            return Err("delegation observation has a stale runtime binding".to_string());
        }
        let (_, mob_handle, _) = self
            .mobs
            .live_member_owner(session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                "live delegation requires a durable Meerkat-Mob member owner".to_string()
            })?;
        let operation = ExactOperationIdentity::for_domain(OperationId::new(), correlation);
        let provisional = ProvisionalLiveHandoff::new(
            operation.domain_correlation().clone(),
            actionable_input,
            LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .map_err(|error| error.to_string())?;

        if let Some(previous) = self.active.lock().await.remove(&channel_key) {
            if previous.task.is_finished() {
                previous
                    .task
                    .await
                    .map_err(|error| format!("live delegation terminal task failed: {error}"))?;
            } else {
                let directive = match self
                    .runtime
                    .supersede_live_delegation(
                        runtime_binding.runtime_id(),
                        runtime_binding.fence_token(),
                        runtime_binding.generation(),
                        &previous.retained.admission,
                        operation.domain_correlation().interaction_id(),
                    )
                    .await
                {
                    Ok(directive) => directive,
                    Err(_error) if previous.task.is_finished() => {
                        previous.task.await.map_err(|task_error| {
                            format!("live delegation terminal task failed: {task_error}")
                        })?;
                        self.runtime
                            .admit_live_delegation(&runtime_binding, &operation, &provisional)
                            .await
                            .map_err(|admit_error| admit_error.to_string())?;
                        return self
                            .start_admitted_delegation(
                                provider_binding,
                                Arc::clone(&control),
                                channel_key,
                                operation,
                                provisional,
                                runtime_binding,
                                mob_handle,
                                delegation,
                            )
                            .await;
                    }
                    Err(error) => {
                        self.active.lock().await.insert(channel_key, previous);
                        return Err(error.to_string());
                    }
                };
                if let LiveDelegationCancellationDirective::CancellationAuthorized(cancellation) =
                    directive
                {
                    let outcome = previous
                        .cancellation
                        .cancel(&cancellation)
                        .await
                        .unwrap_or(LiveDelegationCancellationOutcome::Failed);
                    if let Err(error) = self
                        .runtime
                        .resolve_live_delegation_cancellation(
                            runtime_binding.runtime_id(),
                            runtime_binding.fence_token(),
                            runtime_binding.generation(),
                            &cancellation,
                            outcome,
                        )
                        .await
                    {
                        self.active.lock().await.insert(channel_key, previous);
                        return Err(error.to_string());
                    }
                }
                previous
                    .task
                    .await
                    .map_err(|error| format!("live delegation terminal task failed: {error}"))?;
            }
        }
        self.runtime
            .admit_live_delegation(&runtime_binding, &operation, &provisional)
            .await
            .map_err(|error| error.to_string())?;

        self.start_admitted_delegation(
            provider_binding,
            control,
            channel_key,
            operation,
            provisional,
            runtime_binding,
            mob_handle,
            delegation,
        )
        .await
    }

    async fn start_admitted_delegation(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        channel_key: ActiveChannelKey,
        operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
        provisional: ProvisionalLiveHandoff,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        mob_handle: meerkat_mob::MobHandle,
        delegation: LiveSidebandDelegationRef,
    ) -> Result<(), String> {
        let session_id = provider_binding.session_id();

        let worker_identity =
            AgentIdentity::from(format!("live-delegation:{}", operation.operation_id()));
        let admission = self
            .runtime
            .authorize_live_delegation_worker_start(
                session_id,
                runtime_binding.runtime_id(),
                runtime_binding.fence_token(),
                runtime_binding.generation(),
                &operation,
                &provisional,
                worker_identity.as_str(),
            )
            .await
            .map_err(|error| error.to_string())?;
        let result_spec =
            BoundedResultSpec::new("gpt_live_delegation", LIVE_DELEGATION_RESULT_BYTES)
                .map_err(|error| error.to_string())?;
        let service = DelegationExecutionService::new(mob_handle);
        let request = DelegationExecutionRequest::new_live(
            worker_identity.clone(),
            provisional.executor_input(),
            result_spec,
            admission.clone(),
        );
        let execution = match service.start(request).await {
            Ok(execution) => execution,
            Err(error) => {
                self.runtime
                    .resolve_live_delegation_worker_start(
                        runtime_binding.runtime_id(),
                        runtime_binding.fence_token(),
                        runtime_binding.generation(),
                        &admission,
                        false,
                    )
                    .await
                    .map_err(|resolve_error| resolve_error.to_string())?;
                let retirement = self
                    .runtime
                    .authorize_live_delegation_worker_retirement(
                        runtime_binding.runtime_id(),
                        runtime_binding.fence_token(),
                        runtime_binding.generation(),
                        &admission,
                    )
                    .await
                    .map_err(|retirement_error| retirement_error.to_string())?;
                let retired = service
                    .retire_live_failed_start(&admission, &retirement)
                    .await
                    .is_ok();
                self.runtime
                    .resolve_live_delegation_worker_retirement(
                        runtime_binding.runtime_id(),
                        runtime_binding.fence_token(),
                        runtime_binding.generation(),
                        &retirement,
                        retired,
                    )
                    .await
                    .map_err(|retirement_error| retirement_error.to_string())?;
                return Err(error.to_string());
            }
        };
        self.runtime
            .resolve_live_delegation_worker_start(
                runtime_binding.runtime_id(),
                runtime_binding.fence_token(),
                runtime_binding.generation(),
                &admission,
                true,
            )
            .await
            .map_err(|error| error.to_string())?;
        let cancellation = execution
            .cancellation_handle()
            .ok_or_else(|| "live execution lost cancellation binding".to_string())?;
        let retained = Arc::new(RetainedDelegation {
            operation,
            provisional,
            runtime_binding,
            admission,
            delegation,
            control,
            result: Mutex::new(RetainedDelegationResult::default()),
        });
        self.retained.lock().await.insert(
            retained.operation.operation_id().clone(),
            Arc::clone(&retained),
        );
        let task_coordinator = Arc::new(self.clone());
        let task_runtime = Arc::clone(&self.runtime);
        let task_retained = Arc::clone(&retained);
        let (task_start_tx, task_start_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            if task_start_rx.await.is_err() {
                return;
            }
            let terminal = realize_terminal(
                task_runtime,
                &task_retained.runtime_binding,
                &task_retained.admission,
                &service,
                execution.await_terminal().await,
            )
            .await;
            task_coordinator
                .record_terminal_realization(&task_retained, terminal)
                .await;
        });
        self.active.lock().await.insert(
            channel_key,
            ActiveDelegation {
                retained,
                cancellation,
                task,
            },
        );
        let _ = task_start_tx.send(());
        Ok(())
    }

    async fn record_terminal_realization(
        &self,
        retained: &Arc<RetainedDelegation>,
        terminal: RealizedDelegationTerminal,
    ) {
        {
            let mut result = retained.result.lock().await;
            result.result_text = terminal.result_text;
            result.terminal_ineligible |= terminal.terminal_ineligible;
        }
        if retained.result.lock().await.terminal_ineligible {
            self.remove_retained_delegation(retained).await;
            return;
        }
        if let Err(error) = self.try_release_retained_result(retained).await {
            tracing::warn!(%error, "experimental live result release remained pending");
        }
    }

    async fn remove_retained_delegation(&self, retained: &Arc<RetainedDelegation>) {
        let operation_id = retained.operation.operation_id();
        let mut retained_by_operation = self.retained.lock().await;
        if retained_by_operation
            .get(operation_id)
            .is_some_and(|current| Arc::ptr_eq(current, retained))
        {
            retained_by_operation.remove(operation_id);
        }
        drop(retained_by_operation);

        let channel_key = (
            retained.runtime_binding.session_id().clone(),
            retained.runtime_binding.channel_id().clone(),
        );
        let mut active = self.active.lock().await;
        if active
            .get(&channel_key)
            .is_some_and(|current| Arc::ptr_eq(&current.retained, retained))
        {
            active.remove(&channel_key);
        }
    }

    async fn try_release_retained_result(
        &self,
        retained: &Arc<RetainedDelegation>,
    ) -> Result<(), String> {
        let (reconciliation, result_text, existing_release, existing_delivery) = {
            let mut result = retained.result.lock().await;
            if result.terminal_ineligible || result.delivery_in_flight {
                return Ok(());
            }
            let (Some(reconciliation), Some(result_text)) =
                (result.reconciliation.clone(), result.result_text.clone())
            else {
                return Ok(());
            };
            result.delivery_in_flight = true;
            (
                reconciliation,
                result_text,
                result.release_authority.clone(),
                result.delivery_authority.clone(),
            )
        };

        let release = match existing_release {
            Some(release) => release,
            None => {
                let release = self
                    .runtime
                    .authorize_live_delegation_result_release(
                        retained.runtime_binding.session_id(),
                        retained.runtime_binding.runtime_id(),
                        retained.runtime_binding.fence_token(),
                        retained.runtime_binding.generation(),
                        &retained.operation,
                        &reconciliation,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                retained.result.lock().await.release_authority = Some(release.clone());
                release
            }
        };
        let delivery = match existing_delivery {
            Some(delivery) => delivery,
            None => {
                let delivery = self
                    .runtime
                    .authorize_live_delegation_result_delivery(&release, &result_text)
                    .await
                    .map_err(|error| error.to_string())?;
                retained.result.lock().await.delivery_authority = Some(delivery.clone());
                delivery
            }
        };
        let resolution = match retained
            .control
            .release_delegation_context(delivery, retained.delegation.clone(), result_text)
            .await
            .map_err(|error| error.to_string())?
        {
            ExperimentalGptLiveResultDeliveryDispatch::AwaitingAcknowledgement(waiter) => {
                waiter.resolve().await.map_err(|error| error.to_string())?
            }
            ExperimentalGptLiveResultDeliveryDispatch::Resolved(resolution) => resolution,
        };
        let (authority, observation) = resolution.into_parts();
        let resolution = self
            .runtime
            .resolve_live_delegation_result_delivery(&authority, observation)
            .await
            .map_err(|error| error.to_string())?;
        match resolution {
            LiveDelegationResultDeliveryResolution::Resolved(receipt) => {
                if receipt.retry_allowed() || receipt.recovery_required() {
                    return Err(
                        "generated terminal result delivery returned invalid retry or recovery facts"
                            .to_string(),
                    );
                }
            }
            LiveDelegationResultDeliveryResolution::AmbiguityRecovery(recovery) => {
                self.runtime
                    .realize_live_delegation_result_ambiguity_recovery(recovery)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        self.remove_retained_delegation(retained).await;
        Ok(())
    }

    /// Supply only SessionDocument-sealed exact final-user evidence. Until the
    /// provider can prove its item/turn join, production never calls this and
    /// the worker's tool gate/result release remain closed.
    pub async fn reconcile_exact_final(
        &self,
        evidence: FinalLiveUserTranscriptCommitEvidence,
    ) -> Result<(), String> {
        let channel_key = (evidence.session_id().clone(), evidence.channel_id().clone());
        let retained = self
            .retained
            .lock()
            .await
            .values()
            .find(|retained| {
                retained.runtime_binding.session_id() == evidence.session_id()
                    && retained.runtime_binding.channel_id() == evidence.channel_id()
                    && retained.operation.domain_correlation().interaction_id()
                        == evidence.interaction_id()
            })
            .cloned()
            .ok_or_else(|| "final transcript has no exact active delegation".to_string())?;
        let binding = retained.runtime_binding.clone();
        let receipt = self
            .runtime
            .reconcile_live_delegation_transcript(
                evidence.session_id(),
                binding.runtime_id(),
                binding.fence_token(),
                binding.generation(),
                &retained.operation,
                &retained.provisional,
                &evidence,
            )
            .await
            .map_err(|error| error.to_string())?;
        if receipt.disposition() == LiveHandoffReconciliation::Confirmed {
            let witness = self
                .runtime
                .authorize_live_consequential_effect(
                    evidence.session_id(),
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &retained.operation,
                    &receipt,
                )
                .await
                .map_err(|error| error.to_string())?;
            if let Err(error) = retained.admission.release_tool_execution(&witness)
                && error
                    != meerkat_runtime::live_execution::LiveExecutionAuthorityError::ToolExecutionAdmissionTerminal
            {
                return Err(error.to_string());
            }
            retained.result.lock().await.reconciliation = Some(receipt);
            self.try_release_retained_result(&retained).await?;
        } else if receipt.cancellation_required() {
            retained.result.lock().await.terminal_ineligible = true;
            let cancellation = self
                .runtime
                .authorize_live_delegation_transcript_cancellation(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &retained.admission,
                )
                .await
                .map_err(|error| error.to_string())?;
            let cancellation_handle = self
                .active
                .lock()
                .await
                .get(&channel_key)
                .filter(|active| Arc::ptr_eq(&active.retained, &retained))
                .map(|active| active.cancellation.clone())
                .ok_or_else(|| {
                    "negative transcript has no exact active worker cancellation handle".to_string()
                })?;
            let outcome = cancellation_handle
                .cancel(&cancellation)
                .await
                .unwrap_or(LiveDelegationCancellationOutcome::Failed);
            self.runtime
                .resolve_live_delegation_cancellation(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &cancellation,
                    outcome,
                )
                .await
                .map_err(|error| error.to_string())?;
        } else {
            retained.result.lock().await.terminal_ineligible = true;
            self.remove_retained_delegation(&retained).await;
        }
        Ok(())
    }

    async fn cancel_channel_binding(&self, binding: &ProviderWebrtcBinding) {
        let key = (binding.session_id().clone(), binding.channel_id().clone());
        self.active_turns.lock().await.remove(&key);
        if let Some(active) = self.active.lock().await.remove(&key) {
            if active.retained.runtime_binding.session_id() != binding.session_id()
                || active.retained.runtime_binding.channel_id() != binding.channel_id()
                || active.retained.runtime_binding.generation()
                    != binding.runtime_generation().get()
                || active.retained.runtime_binding.fence_token() != binding.runtime_fence().get()
            {
                self.active.lock().await.insert(key.clone(), active);
            } else {
                active.retained.result.lock().await.terminal_ineligible = true;
                let runtime_binding = &active.retained.runtime_binding;
                if let Ok(directive) = self
                    .runtime
                    .abandon_live_delegation(
                        runtime_binding.runtime_id(),
                        runtime_binding.fence_token(),
                        runtime_binding.generation(),
                        &active.retained.admission,
                    )
                    .await
                    && let LiveDelegationCancellationDirective::CancellationAuthorized(authority) =
                        directive
                {
                    let outcome = active
                        .cancellation
                        .cancel(&authority)
                        .await
                        .unwrap_or(LiveDelegationCancellationOutcome::Failed);
                    let _ = self
                        .runtime
                        .resolve_live_delegation_cancellation(
                            runtime_binding.runtime_id(),
                            runtime_binding.fence_token(),
                            runtime_binding.generation(),
                            &authority,
                            outcome,
                        )
                        .await;
                }
                let _ = active.task.await;
                self.remove_retained_delegation(&active.retained).await;
            }
        }
        let retained = self
            .retained
            .lock()
            .await
            .values()
            .filter(|retained| {
                retained.runtime_binding.session_id() == binding.session_id()
                    && retained.runtime_binding.channel_id() == binding.channel_id()
                    && retained.runtime_binding.generation() == binding.runtime_generation().get()
                    && retained.runtime_binding.fence_token() == binding.runtime_fence().get()
            })
            .cloned()
            .collect::<Vec<_>>();
        for retained in retained {
            retained.result.lock().await.terminal_ineligible = true;
            self.remove_retained_delegation(&retained).await;
        }
    }
}

#[async_trait::async_trait]
impl meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator
    for ExperimentalLiveDelegationCoordinator
{
    async fn prepare_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) -> Result<(), String> {
        ExperimentalLiveDelegationCoordinator::prepare_bound_channel(self, binding, control).await
    }

    async fn run_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) {
        ExperimentalLiveDelegationCoordinator::run_bound_channel(self, binding, control).await;
    }

    async fn observe_provider_lifecycle(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        ExperimentalLiveDelegationCoordinator::observe_provider_lifecycle(self, observation).await
    }

    async fn deactivate_bound_channel(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), String> {
        ExperimentalLiveDelegationCoordinator::deactivate_bound_channel(self, binding).await
    }
}

struct RealizedDelegationTerminal {
    result_text: Option<String>,
    terminal_ineligible: bool,
}

async fn realize_terminal(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    admission: &LiveDelegationExecutionAdmission,
    service: &DelegationExecutionService,
    terminalized: DelegationTerminalizedExecution,
) -> RealizedDelegationTerminal {
    let terminal_kind = match terminalized.terminal() {
        DelegationTurnTerminal::Completed(_) => LiveDelegationWorkerTerminalKind::Completed,
        DelegationTurnTerminal::Failed(_) => LiveDelegationWorkerTerminalKind::Failed,
        _ => LiveDelegationWorkerTerminalKind::Failed,
    };
    let result_text = match terminalized.terminal() {
        DelegationTurnTerminal::Completed(turn) => Some(turn.result().result().text().to_string()),
        DelegationTurnTerminal::Failed(_) => None,
        _ => None,
    };
    let terminal_receipt = match runtime
        .record_live_delegation_worker_terminal(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
            terminal_kind,
        )
        .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            tracing::warn!(%error, "generated live worker terminal recording failed");
            return RealizedDelegationTerminal {
                result_text: None,
                terminal_ineligible: true,
            };
        }
    };
    let retirement = runtime
        .authorize_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
        )
        .await;
    let retired = match retirement {
        Ok(retirement) => {
            let retired = service
                .retire_live_terminalized(&terminalized, &retirement)
                .await
                .is_ok();
            if let Err(error) = runtime
                .resolve_live_delegation_worker_retirement(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &retirement,
                    retired,
                )
                .await
            {
                tracing::warn!(%error, "generated live worker retirement resolution failed");
            }
            retired
        }
        Err(error) => {
            tracing::warn!(%error, "generated live worker retirement authorization failed");
            false
        }
    };
    let result_text =
        retain_terminal_result(retired, terminal_kind, terminal_receipt.late(), result_text);
    let terminal_ineligible = result_text.is_none();
    RealizedDelegationTerminal {
        result_text,
        terminal_ineligible,
    }
}

fn retain_terminal_result(
    retired: bool,
    terminal: LiveDelegationWorkerTerminalKind,
    late: bool,
    result_text: Option<String>,
) -> Option<String> {
    (retired && terminal == LiveDelegationWorkerTerminalKind::Completed && !late)
        .then_some(result_text)
        .flatten()
        .filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_runtime_binding(
        session_id: SessionId,
        channel: &str,
        generation: u64,
    ) -> meerkat_runtime::live_execution::LiveDelegationRuntimeBinding {
        meerkat_runtime::live_execution::LiveDelegationRuntimeBinding::__test_new(
            session_id,
            meerkat_core::LiveChannelId::new(channel),
            meerkat_runtime::identifiers::LogicalRuntimeId::new("live:test-runtime"),
            generation + 100,
            generation,
        )
    }

    #[test]
    fn bounded_result_is_retained_only_for_retired_non_late_completion() {
        let completed = || Some("bounded executor result".to_string());
        assert_eq!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Completed,
                false,
                completed(),
            )
            .as_deref(),
            Some("bounded executor result")
        );
        assert!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Completed,
                true,
                completed(),
            )
            .is_none()
        );
        assert!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Failed,
                false,
                completed(),
            )
            .is_none()
        );
        assert!(
            retain_terminal_result(
                false,
                LiveDelegationWorkerTerminalKind::Completed,
                false,
                completed(),
            )
            .is_none()
        );
        assert!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Completed,
                false,
                Some("   ".to_string()),
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn prepared_bound_channel_deactivates_without_starting_a_run() {
        let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let binding = test_runtime_binding(SessionId::new(), "live-prepared", 1);
        reserve_bound_channel(&channels, binding.clone())
            .await
            .expect("reserve prepared channel");

        release_bound_channel(&channels, &binding)
            .await
            .expect("prepared deactivate is idempotent cleanup");

        assert!(channels.lock().await.is_empty());
    }

    #[tokio::test]
    async fn running_bound_channel_cancel_waits_for_run_completion() {
        let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let binding = test_runtime_binding(SessionId::new(), "live-running", 1);
        reserve_bound_channel(&channels, binding.clone())
            .await
            .expect("reserve running channel");
        let cancellation = begin_bound_channel_run(&channels, &binding)
            .await
            .expect("begin exact channel run");

        let release_channels = Arc::clone(&channels);
        let release_binding = binding.clone();
        let release = tokio::spawn(async move {
            release_bound_channel(&release_channels, &release_binding).await
        });
        cancellation.cancelled().await;
        tokio::task::yield_now().await;
        assert!(
            !release.is_finished(),
            "deactivate returned before run exit"
        );

        finish_bound_channel_run(&channels, &binding).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), release)
            .await
            .expect("deactivate cannot miss completion notification")
            .expect("deactivate task joins")
            .expect("deactivate succeeds");
        assert!(channels.lock().await.is_empty());
    }

    #[tokio::test]
    async fn completion_race_cannot_strand_bound_channel_deactivation() {
        for generation in 1..=64 {
            let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
            let binding =
                test_runtime_binding(SessionId::new(), "live-completion-race", generation);
            reserve_bound_channel(&channels, binding.clone())
                .await
                .expect("reserve raced channel");
            begin_bound_channel_run(&channels, &binding)
                .await
                .expect("begin raced run");

            let release_channels = Arc::clone(&channels);
            let release_binding = binding.clone();
            let release = tokio::spawn(async move {
                release_bound_channel(&release_channels, &release_binding).await
            });
            finish_bound_channel_run(&channels, &binding).await;
            tokio::time::timeout(std::time::Duration::from_secs(1), release)
                .await
                .expect("notification race cannot hang")
                .expect("raced deactivate task joins")
                .expect("raced deactivate succeeds");
            assert!(channels.lock().await.is_empty());
        }
    }

    #[tokio::test]
    async fn stale_bound_channel_deactivation_is_rejected_and_repeat_is_idempotent() {
        let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let session_id = SessionId::new();
        let current = test_runtime_binding(session_id.clone(), "live-stale", 1);
        let stale = test_runtime_binding(session_id, "live-stale", 2);
        reserve_bound_channel(&channels, current.clone())
            .await
            .expect("reserve current channel");

        let error = release_bound_channel(&channels, &stale)
            .await
            .expect_err("stale incarnation cannot deactivate current channel");
        assert!(error.contains("does not match"));
        assert_eq!(channels.lock().await.len(), 1);

        release_bound_channel(&channels, &current)
            .await
            .expect("current prepared channel deactivates");
        release_bound_channel(&channels, &current)
            .await
            .expect("repeated deactivation is idempotent");
        assert!(channels.lock().await.is_empty());
    }
}
