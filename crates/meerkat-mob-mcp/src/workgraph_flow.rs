use async_trait::async_trait;
pub use meerkat_mob::{
    AbandonUncertainWorkGraphFlowRequest, ControlScope, FlowId, FlowRunConfig,
    LaunchWorkGraphFlowRequest, MobDeliveryIdentity, MobError, MobExternalDeliveryAbandonOutcome,
    MobExternalDeliveryBeginOutcome, MobExternalDeliveryIntent, MobExternalDeliveryRecord,
    MobExternalDeliveryTerminal, MobExternalFlowLaunchOutcome, MobId, MobRun, RunId,
    WorkGraphFlowAbandonResult, WorkGraphFlowAdmission, WorkGraphFlowBridge,
    WorkGraphFlowBridgeError, WorkGraphFlowCustodyGuard, WorkGraphFlowExecutionAuthority,
    WorkGraphFlowHost, WorkGraphFlowLaunchResult, WorkGraphFlowObservationAuthority,
    WorkGraphFlowReconcileResult,
};

use crate::MobMcpState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(not(target_arch = "wasm32"))]
use ::tokio;

impl MobMcpState {
    pub(crate) fn authorize_workgraph_flow_realm(
        &self,
        requested_realm: Option<&str>,
    ) -> Result<(), MobError> {
        let service = self.workgraph_service.as_ref().ok_or_else(|| {
            MobError::Internal("WorkGraph service is not configured for this mob surface".into())
        })?;
        if requested_realm.is_some_and(|realm| realm != service.default_realm_id()) {
            return Err(MobError::ScopeDenied(meerkat_mob::ScopeDenial {
                required: ControlScope::List,
                presented: std::collections::BTreeSet::new(),
            }));
        }
        Ok(())
    }

    pub(crate) async fn authorize_workgraph_flow_binding_read(
        &self,
        binding: &meerkat::WorkExecutionBinding,
    ) -> Result<(), MobError> {
        self.authorize_workgraph_flow_realm(Some(&binding.work_ref.realm_id))?;
        let meerkat::WorkExecutionTarget::MobFlow { mob_id, .. } = &binding.target;
        self.admitted_handle_for(&MobId::from(mob_id.as_str()), ControlScope::List)
            .await?;
        Ok(())
    }

    pub(crate) fn authorize_workgraph_flow_binding_list(
        &self,
        requested_realm: Option<&str>,
    ) -> Result<(), MobError> {
        self.authorize_workgraph_flow_realm(requested_realm)?;
        self.require_console_owner(ControlScope::List)
    }

    fn current_workgraph_flow_authority(
        &self,
    ) -> Result<meerkat::WorkExecutionAuthority, MobError> {
        match &self.console_principal {
            meerkat_mob::MobControlPrincipal::Owner => {
                Ok(meerkat::WorkExecutionAuthority::TargetOwner)
            }
            meerkat_mob::MobControlPrincipal::External(principal) => Ok(
                meerkat::WorkExecutionAuthority::principal(principal.clone()),
            ),
            _ => Err(MobError::Internal(
                "unresolved Mob principal cannot mint WorkGraph Flow authority".to_string(),
            )),
        }
    }

    async fn workgraph_flow_authority_handle(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
    ) -> Result<meerkat_mob::MobHandle, MobError> {
        self.ensure_restored().await?;
        let principal = match authority.execution_authority() {
            meerkat::WorkExecutionAuthority::TargetOwner => meerkat_mob::MobControlPrincipal::Owner,
            meerkat::WorkExecutionAuthority::Principal { principal_id } => {
                meerkat_mob::MobControlPrincipal::External(principal_id.clone())
            }
        };
        self.mobs
            .read()
            .await
            .get(authority.mob_id())
            .map(|managed| {
                managed
                    .handle
                    .clone()
                    .with_command_authority(meerkat_mob::CommandAuthority::principal(principal))
            })
            .ok_or_else(|| MobError::MobNotFound(authority.mob_id().clone()))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WorkGraphFlowHost for MobMcpState {
    fn workgraph_execution_bridge(&self) -> Option<meerkat::WorkExecutionBridge> {
        self.workgraph_service
            .as_ref()
            .map(meerkat::WorkGraphService::execution_bridge)
    }

    async fn acquire_workgraph_flow_custody(
        &self,
        binding_id: &meerkat::WorkExecutionBindingId,
    ) -> WorkGraphFlowCustodyGuard {
        let custody = {
            let mut custodies = self.workgraph_flow_custodies.lock().await;
            custodies.retain(|_, custody| custody.strong_count() > 0);
            let key = binding_id.as_str().to_string();
            if let Some(custody) = custodies.get(&key).and_then(std::sync::Weak::upgrade) {
                custody
            } else {
                let custody = std::sync::Arc::new(tokio::sync::Mutex::new(()));
                custodies.insert(key, std::sync::Arc::downgrade(&custody));
                custody
            }
        };
        WorkGraphFlowCustodyGuard::new(custody.lock_owned().await)
    }

    async fn admit_workgraph_flow(
        &self,
        mob_id: &MobId,
        flow_id: &FlowId,
    ) -> Result<WorkGraphFlowAdmission, MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        let config = FlowRunConfig::from_definition(flow_id.clone(), handle.definition())?;
        let execution_authority = self.current_workgraph_flow_authority()?;
        Ok(WorkGraphFlowAdmission {
            config,
            execution_authority,
        })
    }

    async fn admit_workgraph_flow_caller(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat::WorkExecutionAuthority, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        self.current_workgraph_flow_authority()
    }

    async fn workgraph_flow_config(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
    ) -> Result<FlowRunConfig, MobError> {
        let handle = self.workgraph_flow_authority_handle(authority).await?;
        FlowRunConfig::from_definition(authority.flow_id().clone(), handle.definition())
    }

    async fn workgraph_flow_begin_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryBeginOutcome, MobError> {
        self.workgraph_flow_authority_handle(authority)
            .await?
            .begin_external_delivery(intent)
            .await
    }

    async fn workgraph_flow_complete_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<(), MobError> {
        self.workgraph_flow_authority_handle(authority)
            .await?
            .complete_external_delivery(intent, terminal)
            .await
            .map(|_| ())
    }

    async fn workgraph_flow_complete_external_flow_realization(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        run: &MobRun,
    ) -> Result<(), MobError> {
        self.workgraph_flow_authority_handle(authority)
            .await?
            .complete_external_flow_realization(authority, intent, run)
            .await
            .map(|_| ())
    }

    async fn workgraph_flow_abandon_external_delivery(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<MobExternalDeliveryAbandonOutcome, MobError> {
        self.workgraph_flow_authority_handle(authority)
            .await?
            .abandon_external_delivery(intent, terminal)
            .await
    }

    async fn workgraph_flow_load_external_delivery(
        &self,
        observation: &WorkGraphFlowObservationAuthority,
        idempotency_key: &str,
    ) -> Result<Option<MobExternalDeliveryRecord>, MobError> {
        self.ensure_restored().await?;
        let handle = self
            .mobs
            .read()
            .await
            .get(observation.mob_id())
            .map(|managed| managed.handle.clone())
            .ok_or_else(|| MobError::MobNotFound(observation.mob_id().clone()))?;
        handle.load_external_delivery(idempotency_key).await
    }

    async fn workgraph_flow_run_with_external_identity(
        &self,
        authority: &WorkGraphFlowExecutionAuthority,
        params: serde_json::Value,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalFlowLaunchOutcome, MobError> {
        let handle = self.workgraph_flow_authority_handle(authority).await?;
        handle
            .run_flow_with_external_delivery(authority.flow_id().clone(), params, intent)
            .await
    }

    async fn workgraph_flow_status(
        &self,
        observation: &WorkGraphFlowObservationAuthority,
    ) -> Result<Option<MobRun>, MobError> {
        self.ensure_restored().await?;
        let handle = self
            .mobs
            .read()
            .await
            .get(observation.mob_id())
            .map(|managed| managed.handle.clone())
            .ok_or_else(|| MobError::MobNotFound(observation.mob_id().clone()))?;
        handle.flow_status(observation.run_id().clone()).await
    }
}

impl MobMcpState {
    pub async fn launch_workgraph_flow(
        &self,
        request: LaunchWorkGraphFlowRequest,
    ) -> Result<WorkGraphFlowLaunchResult, WorkGraphFlowBridgeError> {
        let result = WorkGraphFlowBridge::new(self)
            .launch_workgraph_flow(request)
            .await;
        self.note_workgraph_flow_reconcile_needed();
        result
    }

    pub async fn reconcile_workgraph_flow(
        &self,
        realm_id: Option<String>,
        namespace: Option<meerkat::WorkNamespace>,
        binding_id: meerkat::WorkExecutionBindingId,
    ) -> Result<WorkGraphFlowReconcileResult, WorkGraphFlowBridgeError> {
        WorkGraphFlowBridge::new(self)
            .reconcile_workgraph_flow_for_caller(realm_id, namespace, binding_id)
            .await
    }

    pub async fn abandon_uncertain_workgraph_flow(
        &self,
        request: AbandonUncertainWorkGraphFlowRequest,
    ) -> Result<WorkGraphFlowAbandonResult, WorkGraphFlowBridgeError> {
        WorkGraphFlowBridge::new(self)
            .abandon_uncertain_workgraph_flow_for_caller(request)
            .await
    }

    fn note_workgraph_flow_reconcile_needed(&self) {
        self.workgraph_flow_reconcile_epoch
            .send_modify(|epoch| *epoch += 1);
    }

    /// Start the host-owned durable WorkGraph Flow reconciler once.
    ///
    /// This is public for host bootstraps that expose the public MCP surface
    /// without constructing the agent-side dispatcher.
    #[doc(hidden)]
    pub fn start_workgraph_flow_reconciler(self: &std::sync::Arc<Self>) {
        use std::sync::atomic::Ordering;

        if self.workgraph_service.is_none()
            || self
                .workgraph_flow_reconciler_started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let weak = std::sync::Arc::downgrade(self);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                self.workgraph_flow_reconciler_started
                    .store(false, Ordering::Release);
                tracing::warn!("WorkGraph Flow reconciler could not start outside a Tokio runtime");
                return;
            };
            runtime.spawn(async move {
                Self::run_workgraph_flow_reconciler(weak).await;
            });
        }
        #[cfg(target_arch = "wasm32")]
        tokio::spawn(async move {
            Self::run_workgraph_flow_reconciler(weak).await;
        });
    }

    async fn run_workgraph_flow_reconciler(weak: std::sync::Weak<Self>) {
        use futures::{FutureExt as _, StreamExt as _, stream::FuturesUnordered};

        loop {
            let Some(state) = weak.upgrade() else {
                return;
            };
            if let Err(error) = state.ensure_restored().await {
                tracing::error!(error = %error, "WorkGraph Flow reconciler could not restore Mob state");
            }

            // Establish every wakeup subscription before reading the durable
            // recovery snapshot. A change concurrent with the scan then leaves
            // the receiver behind and makes `changed()` return immediately,
            // rather than becoming a permanently lost wakeup.
            let mut mob_set_changes = state.mob_set_epoch.subscribe();
            let mut binding_changes = state.workgraph_flow_reconcile_epoch.subscribe();
            let handles = state
                .mobs
                .read()
                .await
                .values()
                .map(|managed| managed.handle.clone())
                .collect::<Vec<_>>();
            let mut machine_changes = FuturesUnordered::new();
            for handle in handles {
                let mut changes = handle.machine_state_changes();
                // A retained handle may outlive an unexpectedly exited actor.
                // Closed watch receivers complete immediately forever, so
                // never put one into the steady-state select set.
                if changes.is_closed() {
                    continue;
                }
                #[cfg(not(target_arch = "wasm32"))]
                machine_changes.push(
                    async move {
                        let _ = changes.changed().await;
                    }
                    .boxed(),
                );
                #[cfg(target_arch = "wasm32")]
                machine_changes.push(
                    async move {
                        let _ = changes.changed().await;
                    }
                    .boxed_local(),
                );
            }

            if let Some(workgraph) = state.workgraph_service.clone() {
                match workgraph.execution_bindings_for_recovery(None).await {
                    Ok(bindings) => {
                        for binding in bindings {
                            match meerkat::WorkExecutionMachine::retry_eligible(&binding) {
                                Ok(true) => continue,
                                Ok(false) => {}
                                Err(error) => {
                                    tracing::error!(
                                        binding_id = %binding.binding_id,
                                        error = %error,
                                        "WorkGraph Flow reconciler rejected a durable binding projection"
                                    );
                                    continue;
                                }
                            }
                            if let Err(error) = WorkGraphFlowBridge::new(state.as_ref())
                                .reconcile_workgraph_flow(
                                    Some(binding.work_ref.realm_id.clone()),
                                    Some(binding.work_ref.namespace.clone()),
                                    binding.binding_id.clone(),
                                )
                                .await
                            {
                                match error {
                                    WorkGraphFlowBridgeError::AmbiguousLaunch { .. }
                                    | WorkGraphFlowBridgeError::RunMissing { .. }
                                    | WorkGraphFlowBridgeError::LaunchFailed { .. } => {
                                        tracing::warn!(
                                            binding_id = %binding.binding_id,
                                            error = %error,
                                            "WorkGraph Flow binding requires durable recovery action"
                                        );
                                    }
                                    _ => tracing::error!(
                                        binding_id = %binding.binding_id,
                                        error = %error,
                                        "WorkGraph Flow reconciliation failed"
                                    ),
                                }
                            }
                        }
                    }
                    Err(error) => tracing::error!(
                        error = %error,
                        "WorkGraph Flow reconciler could not enumerate durable bindings"
                    ),
                }
            }
            drop(state);

            // Event wakes provide normal low-latency progress. The slow tick
            // is a recovery safety net for transient store/runtime errors,
            // because error recovery itself need not emit any watched Mob or
            // binding state change.
            let safety_tick = tokio::time::sleep(std::time::Duration::from_secs(30));
            tokio::pin!(safety_tick);

            if machine_changes.is_empty() {
                tokio::select! {
                    _ = mob_set_changes.changed() => {}
                    _ = binding_changes.changed() => {}
                    () = &mut safety_tick => {}
                }
            } else {
                tokio::select! {
                    _ = mob_set_changes.changed() => {}
                    _ = binding_changes.changed() => {}
                    _ = machine_changes.next() => {}
                    () = &mut safety_tick => {}
                }
            }
        }
    }
}
