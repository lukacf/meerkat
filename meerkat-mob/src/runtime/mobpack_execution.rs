#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(target_arch = "wasm32")]
use crate::tokio::time as tokio_time;
use crate::{
    FlowId, MobBuilder, MobDefinition, MobError, MobHandle, MobRun, MobSessionService, MobState,
    MobStorage, Profile, ProfileName, RunId, SpawnMemberSpec, mob_machine_run_status_is_terminal,
};
use async_trait::async_trait;
use meerkat_core::time_compat::{SystemTime, UNIX_EPOCH};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time as tokio_time;

const CALLABLE_POLICY_PATH: &str = "adaptive/policies.toml";
const LAYER_DESTROY_RETRY_INITIAL: Duration = Duration::from_millis(25);
const LAYER_DESTROY_RETRY_MAX: Duration = Duration::from_secs(1);
// Leave enough scheduler margin for the exact planning subphase error and its
// run ledger to reach the outer adaptive owner before the absolute policy
// deadline wins the timeout race.
const PLANNING_DIAGNOSTIC_HANDOFF: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq)]
pub struct MobpackRunOutcome {
    pub run_id: String,
    pub final_result_digest: Option<String>,
    pub final_result: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobpackCallableConfig {
    coordinator_profile: ProfileName,
}

impl MobpackCallableConfig {
    pub fn new(coordinator_profile: ProfileName) -> Self {
        Self {
            coordinator_profile,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobpackRunSpec {
    definition: MobDefinition,
    packed_skills: BTreeMap<String, Vec<u8>>,
    callable: Option<MobpackCallableConfig>,
    policy_files: BTreeMap<String, Vec<u8>>,
    schemas: BTreeMap<String, Vec<u8>>,
}

impl MobpackRunSpec {
    pub fn new(
        definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        callable: Option<MobpackCallableConfig>,
        policy_files: BTreeMap<String, Vec<u8>>,
        schemas: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            definition,
            packed_skills,
            callable,
            policy_files,
            schemas,
        }
    }

    pub fn is_callable(&self) -> bool {
        self.callable.is_some()
    }

    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }

    pub fn packed_skills(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.packed_skills
    }
}

#[cfg(feature = "runtime-adapter")]
pub async fn run_mobpack_callable(
    spec: &MobpackRunSpec,
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
    objective: &str,
) -> Result<MobpackRunOutcome, MobError> {
    if !spec.is_callable() {
        return Err(MobError::Internal(
            "mobpack has no callable flow".to_string(),
        ));
    }
    run_adaptive_callable(spec, control_mob, session_service, objective).await
}

pub(super) struct PackAdaptiveRuntime {
    pub(super) control_mob: MobHandle,
    pub(super) session_service: Arc<dyn MobSessionService>,
    pub(super) active_planning_run: Option<ActivePlanningRun>,
    #[cfg(test)]
    pub(super) layer_created_probe: Option<tokio::sync::mpsc::UnboundedSender<MobHandle>>,
}

#[derive(Clone)]
pub(super) struct ActivePlanningRun {
    run_id: RunId,
    planning_turn: u64,
}

struct PackLayerCancellationOwner {
    guardian_trigger: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl crate::adaptive::AdaptiveLayerCancellationOwner<MobHandle> for PackLayerCancellationOwner {
    fn take_layer_for_cancellation(&self, _layer: MobHandle) {
        // Closing the trigger is a synchronous handoff to a guardian that was
        // started before the first cancellable post-creation await. The
        // guardian already owns its own MobHandle, so no async task is created
        // from this Drop path and teardown authority cannot disappear here.
        self.guardian_trigger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn disarm_after_cleanup(&self) {
        if let Some(trigger) = self
            .guardian_trigger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = trigger.send(());
        }
    }
}

async fn run_pack_layer_cancellation_cleanup(
    driver: crate::adaptive::AdaptiveDriver,
    capability: crate::AdaptiveDriverCapability,
    layer_id: crate::adaptive::LayerId,
    attempt: u64,
    layer: MobHandle,
    deadline: tokio_time::Instant,
) {
    // Make one best-effort pass before physical cleanup. Neither response is
    // allowed to gate destruction; both inputs are replay-safe and are retried
    // below after the child reaches a proven terminal state.
    let _ = tokio_time::timeout_at(deadline, driver.cancel(&capability)).await;
    let _ = tokio_time::timeout_at(
        deadline,
        driver.record_layer_interrupted(&capability, &layer_id, attempt),
    )
    .await;

    loop {
        if tokio_time::Instant::now() >= deadline {
            tracing::error!(
                layer_id = layer_id.as_str(),
                attempt,
                "adaptive layer cleanup guardian exhausted its bounded terminalization window"
            );
            return;
        }
        let destroyed = matches!(
            tokio_time::timeout_at(deadline, layer.destroy()).await,
            Ok(Ok(_))
        ) || matches!(
            tokio_time::timeout_at(deadline, layer.status()).await,
            Ok(Ok(MobState::Destroyed))
        );
        if destroyed {
            break;
        }

        // Destroy is idempotent and retryable. Retaining this owned worker is
        // preferable to reverting to an orphaned self-retaining mob actor.
        tokio_time::sleep(
            Duration::from_millis(25)
                .min(deadline.saturating_duration_since(tokio_time::Instant::now())),
        )
        .await;
    }

    // Lost responses are indistinguishable from committed transitions. Retry
    // the now-idempotent machine inputs until their acknowledgements arrive;
    // the owned worker continues holding the physically destroyed child while
    // control-plane truth converges.
    while !matches!(
        tokio_time::timeout_at(deadline, driver.cancel(&capability)).await,
        Ok(Ok(()))
    ) {
        if tokio_time::Instant::now() >= deadline {
            return;
        }
        tokio_time::sleep(
            Duration::from_millis(25)
                .min(deadline.saturating_duration_since(tokio_time::Instant::now())),
        )
        .await;
    }
    while !matches!(
        tokio_time::timeout_at(
            deadline,
            driver.record_layer_interrupted(&capability, &layer_id, attempt)
        )
        .await,
        Ok(Ok(()))
    ) {
        if tokio_time::Instant::now() >= deadline {
            return;
        }
        tokio_time::sleep(
            Duration::from_millis(25)
                .min(deadline.saturating_duration_since(tokio_time::Instant::now())),
        )
        .await;
    }
    while !matches!(
        tokio_time::timeout_at(
            deadline,
            driver.record_layer_mob_destroyed(&capability, &layer_id, attempt)
        )
        .await,
        Ok(Ok(()))
    ) {
        if tokio_time::Instant::now() >= deadline {
            return;
        }
        tokio_time::sleep(
            Duration::from_millis(25)
                .min(deadline.saturating_duration_since(tokio_time::Instant::now())),
        )
        .await;
    }
}

impl PackAdaptiveRuntime {
    async fn cancel_active_planning_run(
        &mut self,
        deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> Result<(), crate::adaptive::AdaptiveError> {
        let Some(custody) = self.active_planning_run.clone() else {
            return Ok(());
        };
        let mut last_status = "flow row absent".to_string();
        loop {
            let subphase_remaining = deadline.terminalization_subphase_remaining();
            if subphase_remaining.is_zero() {
                tracing::error!(
                    run_id = %custody.run_id,
                    planning_turn = custody.planning_turn,
                    status = %last_status,
                    deadline_ms = deadline.deadline_ms(),
                    "adaptive planning-flow cancellation exhausted its terminalization window"
                );
                return Err(
                    crate::adaptive::AdaptiveError::TerminalizationDeadlineExceeded {
                        stage: crate::adaptive::AdaptiveRuntimeStage::PlanningCancellation,
                        deadline_ms: deadline.deadline_ms(),
                    },
                );
            }
            match tokio_time::timeout(
                subphase_remaining,
                self.control_mob.flow_status(custody.run_id.clone()),
            )
            .await
            {
                Ok(Ok(Some(run))) => {
                    last_status = format!(
                        "status={:?}; failures={:?}; steps={:?}",
                        run.status, run.failure_ledger, run.step_ledger
                    );
                    if mob_machine_run_status_is_terminal(&run.run_id, &run.status)? {
                        self.active_planning_run = None;
                        return Ok(());
                    }
                    match tokio_time::timeout(
                        deadline.terminalization_subphase_remaining(),
                        self.control_mob.cancel_flow(custody.run_id.clone()),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            self.active_planning_run = None;
                            return Ok(());
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(
                                run_id = %custody.run_id,
                                planning_turn = custody.planning_turn,
                                status = %last_status,
                                error = %error,
                                "adaptive planning-flow cancellation will retry"
                            );
                        }
                        Err(_) => {
                            tracing::warn!(
                                run_id = %custody.run_id,
                                planning_turn = custody.planning_turn,
                                status = %last_status,
                                "adaptive planning-flow cancellation subphase timed out"
                            );
                        }
                    }
                }
                Ok(Ok(None)) => {
                    // The stable run identity is acquired before admission.
                    // An outer timeout can therefore race a still-queued
                    // admission command; retain custody and keep checking
                    // until absence or terminality is authoritative.
                }
                Ok(Err(error)) => {
                    tracing::warn!(
                        run_id = %custody.run_id,
                        planning_turn = custody.planning_turn,
                        status = %last_status,
                        error = %error,
                        "adaptive planning-flow status check during cancellation will retry"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        run_id = %custody.run_id,
                        planning_turn = custody.planning_turn,
                        status = %last_status,
                        "adaptive planning-flow status subphase timed out during cancellation"
                    );
                }
            }
            tokio_time::sleep(Duration::from_millis(25).min(deadline.terminalization_remaining()))
                .await;
        }
    }

    fn cancellation_safe_layer(
        &self,
        layer: MobHandle,
        capability: crate::AdaptiveDriverCapability,
        layer_id: crate::adaptive::LayerId,
        attempt: u64,
        deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> crate::adaptive::AdaptiveLayerLease<MobHandle> {
        let cancellation_deadline =
            tokio_time::Instant::now() + deadline.terminalization_remaining();
        let guardian_layer = layer.clone();
        let driver = crate::adaptive::AdaptiveDriver::new(self.control_mob.clone());
        let (guardian_trigger, cancellation) = tokio::sync::oneshot::channel();

        let worker_driver = driver.clone();
        let worker_capability = capability.clone();
        let worker_layer_id = layer_id.clone();
        let guardian = tokio::spawn(async move {
            if cancellation.await.is_ok() {
                return;
            }
            run_pack_layer_cancellation_cleanup(
                worker_driver,
                worker_capability,
                worker_layer_id,
                attempt,
                guardian_layer,
                cancellation_deadline,
            )
            .await;
        });

        // An owned supervisor awaits the exact guardian task. If that worker
        // ends abnormally, the supervisor retains independent typed authority
        // and re-runs the idempotent cleanup rather than silently dropping the
        // child. The supervisor intentionally outlives PackAdaptiveRuntime.
        let reaper_layer = layer.clone();
        tokio::spawn(async move {
            if guardian.await.is_err() {
                run_pack_layer_cancellation_cleanup(
                    driver,
                    capability,
                    layer_id,
                    attempt,
                    reaper_layer,
                    cancellation_deadline,
                )
                .await;
            }
        });

        crate::adaptive::AdaptiveLayerLease::new(
            layer,
            Arc::new(PackLayerCancellationOwner {
                guardian_trigger: Mutex::new(Some(guardian_trigger)),
            }),
        )
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl crate::adaptive::AdaptiveDriverRuntime for PackAdaptiveRuntime {
    type Capability = crate::AdaptiveDriverCapability;
    type Layer = MobHandle;

    fn now_ms(&mut self) -> u64 {
        now_ms()
    }

    async fn run_planning_turn(
        &mut self,
        request: crate::adaptive::PlanningTurnRequest,
        deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> Result<crate::adaptive::LayerDecision, crate::adaptive::AdaptiveError> {
        let flow_id = FlowId::from("plan");
        let idempotency_key = format!(
            "adaptive:{}:planning:{}",
            request.adaptive_run_id.as_str(),
            request.planning_turn
        );
        let identity = crate::store::MobExternalDeliveryIdentity::new(
            &idempotency_key,
            RunId::new().to_string(),
        )
        .map_err(|error| {
            crate::adaptive::AdaptiveError::DriverRuntime(format!(
                "adaptive planning identity is invalid: {error}"
            ))
        })?;
        let run_id =
            RunId::for_external_delivery(self.control_mob.mob_id(), &flow_id, &idempotency_key);
        self.active_planning_run = Some(ActivePlanningRun {
            run_id: run_id.clone(),
            planning_turn: request.planning_turn,
        });
        tracing::debug!(
            %run_id,
            planning_turn = request.planning_turn,
            "adaptive planning flow entering admission"
        );
        let admission_diagnostics = || {
            format!(
                "run_id={run_id}; planning_turn={}; phase=admission-and-target-provisioning",
                request.planning_turn
            )
        };
        let admission_budget = deadline
            .execution_subphase_remaining()
            .saturating_sub(PLANNING_DIAGNOSTIC_HANDOFF);
        let admission: Result<RunId, crate::adaptive::AdaptiveError> = if admission_budget.is_zero()
        {
            Err(deadline.execution_error(
                crate::adaptive::AdaptiveRuntimeStage::PlanningAdmission,
                admission_diagnostics(),
            ))
        } else {
            match tokio_time::timeout(admission_budget, async {
                    self.control_mob
                        .run_flow_with_external_identity(
                            flow_id,
                            serde_json::json!({
                                "adaptive_run_id": request.adaptive_run_id.as_str(),
                                "objective": request.objective,
                                "previous_layer_result": request.previous_layer_result,
                            }),
                            &identity,
                        )
                        .await
                        .map_err(|error| {
                            crate::adaptive::AdaptiveError::DriverRuntime(format!(
                                "adaptive planning admission failed; run_id={run_id}; planning_turn={}; phase=admission-and-target-provisioning; error={error}",
                                request.planning_turn
                            ))
                        })
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(deadline.execution_error(
                        crate::adaptive::AdaptiveRuntimeStage::PlanningAdmission,
                        admission_diagnostics(),
                    )),
                }
        };
        let admitted_run_id = match admission {
            Ok(run_id) => run_id,
            Err(error @ crate::adaptive::AdaptiveError::DeadlineExceeded { .. }) => {
                // The admission future was dropped at the outer deadline.
                // Retain the preselected run identity so the caller can
                // resolve that ambiguous command through exact cancellation.
                return Err(error);
            }
            Err(error) => {
                // A received non-timeout failure is an authoritative negative
                // admission result; there is no planning flow to retain.
                self.active_planning_run = None;
                return Err(error);
            }
        };
        if admitted_run_id != run_id {
            self.active_planning_run = Some(ActivePlanningRun {
                run_id: admitted_run_id.clone(),
                planning_turn: request.planning_turn,
            });
            return Err(crate::adaptive::AdaptiveError::DriverRuntime(format!(
                "adaptive planning admission returned run '{admitted_run_id}' instead of stable run '{run_id}'"
            )));
        }
        tracing::debug!(
            %run_id,
            planning_turn = request.planning_turn,
            "adaptive planning flow admitted; entering terminal wait"
        );
        let run = await_flow_terminal(
            &self.control_mob,
            run_id.clone(),
            deadline,
            crate::adaptive::AdaptiveRuntimeStage::PlanningTerminalWait,
        )
        .await?;
        self.active_planning_run = None;
        let decision = run
            .root_step_outputs
            .get(&crate::StepId::from("plan"))
            .or_else(|| {
                if run.root_step_outputs.len() == 1 {
                    run.root_step_outputs.values().next()
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                crate::adaptive::AdaptiveError::DriverRuntime(format!(
                    "adaptive planning run '{run_id}' produced no LayerDecision output; status={:?}; failures={:?}; steps={:?}",
                    run.status, run.failure_ledger, run.step_ledger
                ))
            })?;
        serde_json::from_value(decision.clone()).map_err(Into::into)
    }

    async fn cancel_planning_turn(
        &mut self,
        deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> Result<(), crate::adaptive::AdaptiveError> {
        self.cancel_active_planning_run(deadline).await
    }

    async fn provision_layer(
        &mut self,
        capability: &Self::Capability,
        layer_id: &crate::adaptive::LayerId,
        attempt: u64,
        compiled: &crate::adaptive::CompiledLayer,
        deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> crate::adaptive::AdaptiveLayerProvision<Self::Layer> {
        let requested_members = compiled.spawn_specs.len() as u64;
        let mut builder = match MobBuilder::from_mobpack(
            compiled.definition.clone(),
            BTreeMap::new(),
            MobStorage::in_memory(),
        ) {
            Ok(builder) => builder.with_session_service(Arc::clone(&self.session_service)),
            Err(error) => {
                return crate::adaptive::AdaptiveLayerProvision::Failed {
                    layer: None,
                    fault: crate::AdaptiveLayerSetupFault::MobCreateFailed,
                    spawned_members: 0,
                    requested_members,
                    error: error.into(),
                };
            }
        };
        if let Some(adapter) = self.session_service.runtime_adapter() {
            builder = builder.with_runtime_adapter(adapter);
        }
        let handle = match builder.create().await {
            Ok(handle) => handle,
            Err(error) => {
                return crate::adaptive::AdaptiveLayerProvision::Failed {
                    layer: None,
                    fault: crate::AdaptiveLayerSetupFault::MobCreateFailed,
                    spawned_members: 0,
                    requested_members,
                    error: error.into(),
                };
            }
        };
        #[cfg(test)]
        if let Some(probe) = &self.layer_created_probe {
            let _ = probe.send(handle.clone());
        }
        let layer = self.cancellation_safe_layer(
            handle,
            capability.clone(),
            layer_id.clone(),
            attempt,
            deadline,
        );
        let spawn_results = match layer.layer().spawn_many(compiled.spawn_specs.clone()).await {
            Ok(results) => results,
            Err(error) => {
                return crate::adaptive::AdaptiveLayerProvision::Failed {
                    layer: Some(layer),
                    fault: crate::AdaptiveLayerSetupFault::SpawnFailed,
                    // The aggregate call can fail after provisioning side
                    // effects but before returning row receipts. Preserve the
                    // upstream conservative bound; destroy owns rollback.
                    spawned_members: requested_members,
                    requested_members,
                    error: error.into(),
                };
            }
        };
        if let Some(failure) = spawn_results
            .iter()
            .find_map(|result| result.as_ref().err())
        {
            return crate::adaptive::AdaptiveLayerProvision::Failed {
                layer: Some(layer),
                fault: crate::AdaptiveLayerSetupFault::SpawnFailed,
                // A failed row can be ambiguous after remote provisioning
                // side effects. Preserve the upstream conservative bound;
                // joined destroy owns every actual materialization.
                spawned_members: requested_members,
                requested_members,
                error: crate::adaptive::AdaptiveError::DriverRuntime(format!(
                    "mobpack layer spawn failed: {failure}"
                )),
            };
        }
        crate::adaptive::AdaptiveLayerProvision::Ready(layer)
    }

    async fn start_layer_flow(
        &mut self,
        layer: &Self::Layer,
        activation_params: BTreeMap<String, serde_json::Value>,
        _deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> Result<RunId, crate::adaptive::AdaptiveError> {
        Ok(layer
            .run_flow(
                FlowId::from("layer-flow"),
                serde_json::to_value(activation_params)?,
            )
            .await?)
    }

    async fn await_layer_terminal(
        &mut self,
        layer: &Self::Layer,
        run_id: RunId,
        deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> Result<MobRun, crate::adaptive::AdaptiveError> {
        await_flow_terminal(
            layer,
            run_id,
            deadline,
            crate::adaptive::AdaptiveRuntimeStage::LayerTerminal,
        )
        .await
    }

    async fn cancel_layer_flow(
        &mut self,
        layer: &Self::Layer,
        run_id: RunId,
        _deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> Result<(), crate::adaptive::AdaptiveError> {
        Ok(layer.cancel_flow(run_id).await?)
    }

    async fn cleanup_layer(
        &mut self,
        layer: &crate::adaptive::AdaptiveLayerLease<Self::Layer>,
        layer_id: &crate::adaptive::LayerId,
        attempt: u64,
        deadline: &crate::adaptive::AdaptiveOperationDeadline,
    ) -> Result<crate::adaptive::AdaptiveLayerCleanup, crate::adaptive::AdaptiveError> {
        let mut retry_delay = LAYER_DESTROY_RETRY_INITIAL;
        loop {
            let remaining = deadline.cleanup_remaining();
            if remaining.is_zero() {
                return Ok(crate::adaptive::AdaptiveLayerCleanup::Retained(
                    crate::AdaptiveLayerDisposition::Retained,
                ));
            }
            let destroy = match tokio_time::timeout(remaining, layer.layer().destroy()).await {
                Ok(result) => result,
                Err(_) => {
                    return Ok(crate::adaptive::AdaptiveLayerCleanup::Retained(
                        crate::AdaptiveLayerDisposition::Retained,
                    ));
                }
            };
            match destroy {
                Ok(_) => return Ok(crate::adaptive::AdaptiveLayerCleanup::Destroyed),
                Err(error) => {
                    let actor_channel_closed = matches!(
                        &error,
                        crate::MobDestroyError::Mob(
                            MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed
                        )
                    );
                    if actor_channel_closed
                        && matches!(
                            layer.layer().status().await,
                            Ok(crate::runtime::MobState::Destroyed)
                        )
                    {
                        // The actor publishes its terminal phase before
                        // exiting. A closed command/reply channel plus that
                        // watch proof is equivalent to the lost success ACK.
                        return Ok(crate::adaptive::AdaptiveLayerCleanup::Destroyed);
                    }
                    tracing::warn!(
                        layer_id = layer_id.as_str(),
                        attempt,
                        error = %error,
                        retry_delay_ms = retry_delay.as_millis(),
                        "adaptive mobpack child destroy incomplete; retaining lease and retrying",
                    );
                    let remaining = deadline.cleanup_remaining();
                    if remaining.is_zero() {
                        return Ok(crate::adaptive::AdaptiveLayerCleanup::Retained(
                            crate::AdaptiveLayerDisposition::Retained,
                        ));
                    }
                    tokio_time::sleep(retry_delay.min(remaining)).await;
                    retry_delay = retry_delay.saturating_mul(2).min(LAYER_DESTROY_RETRY_MAX);
                }
            }
        }
    }
}

#[cfg(feature = "runtime-adapter")]
async fn run_adaptive_callable(
    spec: &MobpackRunSpec,
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
    objective: &str,
) -> Result<MobpackRunOutcome, MobError> {
    let policy = load_policy(spec)?;
    let schema_registry = load_schema_registry(spec)?;
    let profile_templates = load_profile_templates(spec)?;
    if let Some(callable) = &spec.callable {
        let coordinator_profile = callable.coordinator_profile.clone();
        let coordinator_identity = crate::AgentIdentity::from("adaptive-flowmaster");
        control_mob
            .ensure_member(SpawnMemberSpec::new(
                coordinator_profile.clone(),
                coordinator_identity.clone(),
            ))
            .await
            .map_err(|err| {
                MobError::Internal(format!("mobpack FlowMaster ensure failed: {err}"))
            })?;
        let machine_state = control_mob.query_machine_state().await?;
        let dsl_identity =
            crate::machines::mob_machine::AgentIdentity::from_domain(&coordinator_identity);
        let resolved_profile = machine_state
            .member_profile_name_for_identity(&dsl_identity)
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine retained FlowMaster without a profile binding".to_string(),
                )
            })?;
        if resolved_profile != coordinator_profile.as_str()
            || machine_state
                .member_lifecycle_for_identity(&dsl_identity)
                .status
                != crate::machines::mob_machine::MobMemberLifecycleStatus::Active
        {
            return Err(MobError::Internal(format!(
                "MobMachine FlowMaster identity '{coordinator_identity}' resolved to profile '{resolved_profile}' with non-callable lifecycle; expected active profile '{coordinator_profile}'"
            )));
        }
    }
    let adaptive_run_id = fresh_run_id()?;
    let compile_context = crate::adaptive::CompileContext {
        adaptive_run_id: adaptive_run_id.clone(),
        attempt: 1,
        schema_registry,
        profile_templates,
        previous_layer_result: None,
    };
    let driver = crate::adaptive::AdaptiveDriver::new(control_mob.clone());
    let mut runtime = PackAdaptiveRuntime {
        control_mob,
        session_service,
        active_planning_run: None,
        #[cfg(test)]
        layer_created_probe: None,
    };
    let outcome = crate::adaptive::run_adaptive_loop(
        &driver,
        &mut runtime,
        crate::adaptive::AdaptiveRunRequest {
            adaptive_run_id: adaptive_run_id.clone(),
            policy,
            compile_context,
            objective: objective.to_string(),
            started_at_ms: now_ms(),
        },
    )
    .await
    .map_err(|err| MobError::Internal(err.to_string()))?;
    Ok(MobpackRunOutcome {
        run_id: adaptive_run_id.as_str().to_string(),
        final_result_digest: outcome
            .final_result_digest
            .as_ref()
            .map(|digest| digest.as_str().to_string()),
        final_result: outcome.final_result,
    })
}

fn load_policy(spec: &MobpackRunSpec) -> Result<crate::adaptive::AdaptivePolicy, MobError> {
    let bytes = spec
        .policy_files
        .get(CALLABLE_POLICY_PATH)
        .ok_or_else(|| MobError::Internal("mobpack missing adaptive/policies.toml".to_string()))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|err| MobError::Internal(format!("mobpack policy is not valid UTF-8: {err}")))?;
    toml::from_str(text).map_err(|err| MobError::Internal(format!("invalid mobpack policy: {err}")))
}

fn load_schema_registry(
    spec: &MobpackRunSpec,
) -> Result<crate::adaptive::SchemaRegistry, MobError> {
    let registry_bytes = spec
        .schemas
        .get("schemas/registry.json")
        .ok_or_else(|| MobError::Internal("mobpack missing schemas/registry.json".to_string()))?;
    let declared: BTreeMap<String, String> = serde_json::from_slice(registry_bytes)
        .map_err(|err| MobError::Internal(format!("invalid schemas/registry.json: {err}")))?;
    let mut registry = crate::adaptive::SchemaRegistry::default();
    for (name, path) in declared {
        let schema_bytes = spec
            .schemas
            .get(&path)
            .or_else(|| spec.schemas.get(&format!("schemas/{path}")))
            .ok_or_else(|| {
                MobError::Internal(format!("schema registry entry '{name}' missing '{path}'"))
            })?;
        let schema: serde_json::Value = serde_json::from_slice(schema_bytes)
            .map_err(|err| MobError::Internal(format!("invalid schema '{path}': {err}")))?;
        registry
            .insert(
                crate::adaptive::SchemaName::new(name)
                    .map_err(|err| MobError::Internal(err.to_string()))?,
                schema,
            )
            .map_err(|err| MobError::Internal(format!("invalid mobpack schema: {err}")))?;
    }
    Ok(registry)
}

fn load_profile_templates(
    spec: &MobpackRunSpec,
) -> Result<BTreeMap<ProfileName, Profile>, MobError> {
    let mut profiles = BTreeMap::new();
    for (name, binding) in &spec.definition.profiles {
        let Some(profile) = binding.as_inline() else {
            return Err(MobError::Internal(format!(
                "mobpack profile '{name}' uses a realm profile reference; inline profile templates are required"
            )));
        };
        profiles.insert(name.clone(), profile.clone());
    }
    Ok(profiles)
}

async fn await_flow_terminal(
    mob: &MobHandle,
    run_id: RunId,
    deadline: &crate::adaptive::AdaptiveOperationDeadline,
    stage: crate::adaptive::AdaptiveRuntimeStage,
) -> Result<MobRun, crate::adaptive::AdaptiveError> {
    let mut last_status = "flow row absent".to_string();
    loop {
        let status_budget = deadline
            .execution_subphase_remaining()
            .saturating_sub(PLANNING_DIAGNOSTIC_HANDOFF);
        if status_budget.is_zero() {
            tracing::error!(
                %run_id,
                stage = %stage,
                status = %last_status,
                deadline_ms = deadline.deadline_ms(),
                "adaptive flow terminal wait exhausted its execution window"
            );
            return Err(deadline
                .execution_error(stage, format!("run_id={run_id}; last_status={last_status}")));
        }
        let status = match tokio_time::timeout(status_budget, mob.flow_status(run_id.clone())).await
        {
            Ok(result) => result.map_err(crate::adaptive::AdaptiveError::from)?,
            Err(_) => {
                return Err(deadline.execution_error(
                    stage,
                    format!("run_id={run_id}; last_status={last_status}"),
                ));
            }
        };
        if let Some(run) = status {
            let observed_status = format!(
                "status={:?}; failures={:?}; steps={:?}",
                run.status, run.failure_ledger, run.step_ledger
            );
            if observed_status != last_status {
                tracing::debug!(
                    %run_id,
                    stage = %stage,
                    status = %observed_status,
                    "adaptive flow terminal wait observed a status transition"
                );
                last_status = observed_status;
            }
            if mob_machine_run_status_is_terminal(&run_id, &run.status)? {
                return Ok(run);
            }
        }
        tokio_time::sleep(Duration::from_millis(250).min(deadline.execution_subphase_remaining()))
            .await;
    }
}

fn fresh_run_id() -> Result<crate::adaptive::AdaptiveRunId, MobError> {
    crate::adaptive::AdaptiveRunId::new(RunId::new().to_string())
        .map_err(|err| MobError::Internal(err.to_string()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_run_id_is_unique_per_invocation() -> Result<(), Box<dyn std::error::Error>> {
        let first = fresh_run_id()?;
        let second = fresh_run_id()?;

        assert_ne!(first, second);
        let _first: RunId = first.as_str().parse()?;
        let _second: RunId = second.as_str().parse()?;
        Ok(())
    }
}
