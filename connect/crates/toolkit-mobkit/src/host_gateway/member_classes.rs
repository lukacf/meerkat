//! Held additions retain their post-native authorization work with this host.
//! The canonical policy owner stores the route; only actual Created enables it.
use super::*;
use crate::installed_host::{
    dynamic_members::AdmittedMemberClass,
    preparation::{PreparedControllerIntent, PreparedControllerTarget},
};
use meerkat_mobkit::identity_first::{
    AgentIdentity, DurableAgentSpec, HeldBootstrapStart, IdentityRuntimeError,
    PublicIdentityFirstContext, RestoreFlowResult, RestoreOutcome,
};
use meerkat_toolkit::host_lifetime::policy::{
    CompiledNativeSourceOrigin, CompiledNativeSourceRoute,
};
use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::{
    sync::{Notify, oneshot},
    task::JoinSet,
};

#[derive(Debug, thiserror::Error)]
pub enum MemberClassAddError {
    #[error(transparent)]
    Host(#[from] HostLifetimeError),
    #[error(transparent)]
    Native(#[from] IdentityRuntimeError),
    /// This call took no reservation ownership and changed no admission. The
    /// caller can observe or explicitly retire its reservation; an earlier
    /// accepted operation may already own the same correlation.
    #[error("member add was not accepted: {error}; admission unchanged")]
    ReservationNotAccepted { error: HostLifetimeError },
    /// An accepted operation failed before native entry. Its existing host
    /// owner joined retirement; a failed cleanup remains an explicit result.
    #[error("member add preflight refused: {error}; cleanup: {cleanup:?}")]
    Preflight {
        error: HostLifetimeError,
        cleanup: Result<(), HostLifetimeError>,
    },
}

/// Native settlement and Connect authorization are independent facts. A native
/// Created is retained even if route publication or subsequent narrowing fails.
#[derive(Debug)]
pub struct MemberClassAddOutcome {
    pub native: Result<RestoreFlowResult, IdentityRuntimeError>,
    pub authorization: Result<(), HostLifetimeError>,
    pub cleanup: Result<(), HostLifetimeError>,
}

/// Read-only terminal facts supplied by the existing accepted operation owner.
/// A resource callback must not treat missing native Created evidence as proof
/// that no native member exists. Unknown outcomes retain physical resources.
pub enum MemberClassCleanupObservation<'a> {
    Preflight {
        error: HostLifetimeError,
        cleanup: Result<(), HostLifetimeError>,
    },
    Settled(&'a MemberClassAddOutcome),
    UnacceptedRetirement(Result<(), HostLifetimeError>),
}

/// Optional application resource cleanup, joined inside the host's accepted
/// operation before its terminal result. It adds no admission or native truth.
#[async_trait::async_trait]
pub trait MemberClassResourceCleanup: Send + Sync {
    async fn cleanup(
        &self,
        observation: MemberClassCleanupObservation<'_>,
    ) -> Result<(), HostLifetimeError>;
}

async fn resource_cleanup(
    selected: Option<&Arc<dyn MemberClassResourceCleanup>>,
    observation: MemberClassCleanupObservation<'_>,
) -> Result<(), HostLifetimeError> {
    match selected {
        Some(selected) => selected.cleanup(observation).await,
        None => Ok(()),
    }
}

#[derive(Debug)]
pub enum MemberClassAddStart {
    Settled(MemberClassAddOutcome),
    Unsettled(MemberClassSettlement),
}

/// An observation of the host-owned operation. Dropping this receiver does not
/// drop native settlement or an entered policy mutation. Host closure joins it.
#[derive(Debug)]
pub struct MemberClassSettlement {
    result: oneshot::Receiver<MemberClassAddOutcome>,
}
impl MemberClassSettlement {
    pub async fn settled(self) -> Result<MemberClassAddOutcome, HostLifetimeError> {
        self.result.await.map_err(|_| HostLifetimeError::NotSettled)
    }
}

#[derive(Default)]
struct Operations {
    tasks: JoinSet<Result<(), HostLifetimeError>>,
    failure: Option<HostLifetimeError>,
}
#[derive(Default)]
pub(super) struct MemberClassOperations {
    stopped: AtomicBool,
    changed: Notify,
    state: tokio::sync::Mutex<Operations>,
}
impl MemberClassOperations {
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
    async fn stopped(&self) {
        loop {
            let changed = self.changed.notified();
            if self.stopped.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }
    fn record(
        state: &mut Operations,
        result: Result<Result<(), HostLifetimeError>, tokio::task::JoinError>,
    ) {
        if !matches!(result, Ok(Ok(()))) {
            state.failure = Some(match result {
                Ok(Err(error)) => error,
                _ => HostLifetimeError::NotSettled,
            });
        }
    }
    pub(super) async fn join(&self) -> Result<(), HostLifetimeError> {
        self.stop();
        let mut state = self.state.lock().await;
        while let Some(result) = state.tasks.join_next().await {
            Self::record(&mut state, result);
        }
        state.failure.map_or(Ok(()), Err)
    }
}

impl ToolkitGatewayHost {
    /// Retire an original, still-unaccepted reservation through this host's
    /// existing bounded operation owner. This consumes the same one-shot right
    /// as Add, so an accepted Add or reconstructed handle cannot be cancelled
    /// here. No native construction is started.
    ///
    /// Dropping the observer does not drop entered retirement. Host close joins
    /// it, and the caller may release the associated physical resource only
    /// after observing successful retirement.
    pub async fn retire_unaccepted_member_class(
        self: &Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
    ) -> Result<(), HostLifetimeError> {
        self.retire_unaccepted_member_class_selected(admitted, None)
            .await
    }

    /// The same retirement operation retains and joins physical resource
    /// cleanup even when the caller drops its observation.
    pub async fn retire_unaccepted_member_class_with_cleanup(
        self: &Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
        cleanup: Arc<dyn MemberClassResourceCleanup>,
    ) -> Result<(), HostLifetimeError> {
        self.retire_unaccepted_member_class_selected(admitted, Some(cleanup))
            .await
    }

    async fn retire_unaccepted_member_class_selected(
        self: &Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
        cleanup: Option<Arc<dyn MemberClassResourceCleanup>>,
    ) -> Result<(), HostLifetimeError> {
        let originals = self
            .private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let policy = originals
            .service_tool_policy
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(policy, &admitted.owner) {
            return Err(HostLifetimeError::WrongOwner);
        }
        let mut operations = self.member_class_operations.state.lock().await;
        while let Some(result) = operations.tasks.try_join_next() {
            MemberClassOperations::record(&mut operations, result);
        }
        if self.member_class_operations.stopped.load(Ordering::Acquire)
            || operations.tasks.len() >= 16
        {
            return Err(HostLifetimeError::Unavailable);
        }
        admitted.claim_add()?;
        // No await or fallible step separates one-shot transfer from the same
        // host owner's retained task. Rejected Add and retirement observers
        // never obtain cleanup custody over another accepted operation.
        let host = self.clone();
        let (send, receive) = oneshot::channel();
        operations.tasks.spawn(async move {
            let retired = admitted.retire().await;
            let physical = resource_cleanup(
                cleanup.as_ref(),
                MemberClassCleanupObservation::UnacceptedRetirement(retired),
            )
            .await;
            let result = retired.and(physical);
            let _ = send.send(result);
            drop(host);
            result
        });
        drop(operations);
        receive.await.map_err(|_| HostLifetimeError::NotSettled)?
    }

    /// Add one admitted ordinary member using this host's actual native context.
    /// The finalizer runs once after held policy/route commitment, before native
    /// activation, so the application can publish its own desired roster there.
    /// New ordinary Adds retain the operator-owned peer-message and own-Recorder
    /// Write parents before activation. They do not select a delivery child or
    /// override the class tool grants. Helper/service grants remain separate.
    ///
    /// Once admitted to this host's bounded operation owner, dropping the caller
    /// is observation loss, not cancellation. Native still owns its deadline,
    /// activation and aborts. This host owns the eventual route/retirement work.
    pub async fn add_member_class<F, Fut>(
        self: &Arc<Self>,
        context: &PublicIdentityFirstContext,
        admitted: Arc<AdmittedMemberClass>,
        spec: DurableAgentSpec,
        route: CompiledNativeSourceRoute,
        controller: PreparedControllerIntent,
        deadline: meerkat_mob::HeldSpawnDeadline,
        finalizer: F,
    ) -> Result<MemberClassAddStart, MemberClassAddError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), HostLifetimeError>> + Send + 'static,
    {
        if !admitted.class().purpose.is_ordinary() || admitted.workflow().is_some() {
            return Err(MemberClassAddError::ReservationNotAccepted {
                error: HostLifetimeError::Denied,
            });
        }
        self.add_member_class_selected(
            context,
            admitted,
            spec,
            route,
            controller,
            deadline,
            move |_| finalizer(),
            None,
            None,
        )
        .await
    }
    pub(crate) async fn add_member_class_with_mcp<F, Fut>(
        self: &Arc<Self>,
        context: &PublicIdentityFirstContext,
        admitted: Arc<AdmittedMemberClass>,
        spec: DurableAgentSpec,
        route: CompiledNativeSourceRoute,
        controller: PreparedControllerIntent,
        deadline: meerkat_mob::HeldSpawnDeadline,
        finalizer: F,
        mcp: Option<(
            crate::installed_host::member_mcp_consumer::MemberMcpAddIntent,
            crate::private_host::ConsoleLoginHandle,
        )>,
    ) -> Result<MemberClassAddStart, MemberClassAddError>
    where
        F: FnOnce(meerkat_mob::HeldSpawnDeadline) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), HostLifetimeError>> + Send + 'static,
    {
        if !admitted.class().purpose.is_ordinary() || admitted.workflow().is_some() {
            return Err(MemberClassAddError::ReservationNotAccepted {
                error: HostLifetimeError::Denied,
            });
        }
        self.add_member_class_selected(
            context,
            admitted,
            spec,
            route,
            controller,
            deadline,
            finalizer,
            Some(mcp),
            None,
        )
        .await
    }
    pub(super) async fn add_member_class_selected<F, Fut>(
        self: &Arc<Self>,
        context: &PublicIdentityFirstContext,
        admitted: Arc<AdmittedMemberClass>,
        spec: DurableAgentSpec,
        route: CompiledNativeSourceRoute,
        controller: PreparedControllerIntent,
        deadline: meerkat_mob::HeldSpawnDeadline,
        finalizer: F,
        mcp: Option<
            Option<(
                crate::installed_host::member_mcp_consumer::MemberMcpAddIntent,
                crate::private_host::ConsoleLoginHandle,
            )>,
        >,
        cleanup: Option<Arc<dyn MemberClassResourceCleanup>>,
    ) -> Result<MemberClassAddStart, MemberClassAddError>
    where
        F: FnOnce(meerkat_mob::HeldSpawnDeadline) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), HostLifetimeError>> + Send + 'static,
    {
        let originals =
            self.private_originals
                .as_ref()
                .ok_or(MemberClassAddError::ReservationNotAccepted {
                    error: HostLifetimeError::Unavailable,
                })?;
        // A wrong host cannot take ownership merely to retire another owner's
        // reservation. Full current selection/preflight runs after transfer.
        let policy = originals.service_tool_policy.get().ok_or(
            MemberClassAddError::ReservationNotAccepted {
                error: HostLifetimeError::Unavailable,
            },
        )?;
        if !Arc::ptr_eq(policy, &admitted.owner) {
            return Err(MemberClassAddError::ReservationNotAccepted {
                error: HostLifetimeError::WrongOwner,
            });
        }
        let mut operations = self.member_class_operations.state.lock().await;
        while let Some(result) = operations.tasks.try_join_next() {
            MemberClassOperations::record(&mut operations, result);
        }
        // Same resource bound as private command admission. It grants no
        // authority; exact native and canonical owner checks remain mandatory.
        if self.member_class_operations.stopped.load(Ordering::Acquire)
            || operations.tasks.len() >= 16
        {
            return Err(MemberClassAddError::ReservationNotAccepted {
                error: HostLifetimeError::Unavailable,
            });
        }
        // Only the original, newly Reserved handle can transfer its one-shot
        // cleanup obligation. Same-id replay and reusing this Arc cannot retire
        // a Prepared/Active admission or another accepted Add.
        // The same source gate fences both input binding and Add acceptance.
        // No await or native callback separates its claim from retained spawn.
        // Retirement and already-entered completion do not use this gate.
        let receive = originals
            .with_new_work_admission(|| {
                admitted.claim_add()?;
                let host = self.clone();
                let context = context.clone();
                let (send, receive) = oneshot::channel();
                operations.tasks.spawn(async move {
                    host.run_member_class(
                        context, admitted, spec, route, controller, deadline, finalizer, mcp,
                        cleanup, send,
                    )
                    .await
                });
                Ok::<_, HostLifetimeError>(receive)
            })
            .map_err(|_| MemberClassAddError::ReservationNotAccepted {
                error: HostLifetimeError::Unavailable,
            })?
            .map_err(|error| MemberClassAddError::ReservationNotAccepted { error })?;
        drop(operations);
        receive
            .await
            .map_err(|_| MemberClassAddError::Host(HostLifetimeError::NotSettled))?
    }

    async fn run_member_class<F, Fut>(
        self: Arc<Self>,
        context: PublicIdentityFirstContext,
        admitted: Arc<AdmittedMemberClass>,
        spec: DurableAgentSpec,
        route: CompiledNativeSourceRoute,
        controller: PreparedControllerIntent,
        deadline: meerkat_mob::HeldSpawnDeadline,
        finalizer: F,
        mcp: Option<
            Option<(
                crate::installed_host::member_mcp_consumer::MemberMcpAddIntent,
                crate::private_host::ConsoleLoginHandle,
            )>,
        >,
        resource: Option<Arc<dyn MemberClassResourceCleanup>>,
        response: oneshot::Sender<Result<MemberClassAddStart, MemberClassAddError>>,
    ) -> Result<(), HostLifetimeError>
    where
        F: FnOnce(meerkat_mob::HeldSpawnDeadline) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), HostLifetimeError>> + Send + 'static,
    {
        if let Err(error) =
            self.member_class_preflight(&context, &admitted, &spec, &route, &controller)
        {
            let retired = admitted.retire().await;
            let physical = resource_cleanup(
                resource.as_ref(),
                MemberClassCleanupObservation::Preflight {
                    error,
                    cleanup: retired,
                },
            )
            .await;
            let cleanup = retired.and(physical);
            let _ = response.send(Err(MemberClassAddError::Preflight { error, cleanup }));
            return cleanup;
        }
        if admitted.workflow().is_some() {
            let result = match &self.private_originals {
                Some(source) => {
                    source
                        .select_workflow_class(admitted.clone(), &route, &spec)
                        .await
                }
                None => Err(HostLifetimeError::Unavailable),
            };
            if let Err(error) = result {
                let retired = admitted.retire().await;
                let physical = resource_cleanup(
                    resource.as_ref(),
                    MemberClassCleanupObservation::Preflight {
                        error,
                        cleanup: retired,
                    },
                )
                .await;
                let cleanup = retired.and(physical);
                let _ = response.send(Err(MemberClassAddError::Preflight { error, cleanup }));
                return cleanup;
            }
        }
        // This accepted host task owns review/retention and exact cleanup.
        // Native held construction starts only after the application ACK and
        // actual protected decisions, never while waiting for a human.
        let hold_budget = deadline.saturating_duration_since(tokio::time::Instant::now());
        let (member_mcp, deadline) = match mcp {
            None => (None, deadline),
            Some(request) => match self.prepare_member_mcp_add(&admitted, request).await {
                Ok(prepared) => (prepared, tokio::time::Instant::now() + hold_budget),
                Err(error) => {
                    let retired = admitted.retire().await;
                    let physical = resource_cleanup(
                        resource.as_ref(),
                        MemberClassCleanupObservation::Preflight {
                            error,
                            cleanup: retired,
                        },
                    )
                    .await;
                    let cleanup = retired.and(physical);
                    let _ = response.send(Err(MemberClassAddError::Preflight { error, cleanup }));
                    return cleanup;
                }
            },
        };
        let identity = spec.identity.clone();
        let callback_identity = identity.clone();
        let callback_admission = admitted.clone();
        let host = self.clone();
        let native = context
            .add_member_held(spec, deadline, move |mut members, completion| async move {
                let admitted = callback_admission;
                let entered = AtomicBool::new(false);
                let cancelled = AtomicBool::new(false);
                let complete = async {
                    host.check_member_class_owner(&admitted, &route, &controller)?;
                    if members.len() != 1 || members[0].identity != callback_identity {
                        return Err(HostLifetimeError::Denied);
                    }
                    let held = members.remove(0);
                    entered.store(true, Ordering::Release);
                    let route_for_check = route.clone();
                    host.private_originals
                        .as_ref()
                        .ok_or(HostLifetimeError::Unavailable)?
                        .complete_member_class(&admitted, &held, route, controller)
                        .await?;
                    host.owner.validate_current_selection()?;
                    if let Some(source) = &host.private_originals
                        && admitted.workflow().is_some()
                    {
                        source
                            .check_workflow_class_source(&admitted, &route_for_check)
                            .await?;
                    }
                    if tokio::time::Instant::now() >= deadline
                        || cancelled.load(Ordering::Acquire)
                        || host.private_closed.is_closed()
                        || host.member_class_operations.stopped.load(Ordering::Acquire)
                    {
                        return Err(HostLifetimeError::Denied);
                    }
                    // The application finalizer is inside native complete, after
                    // durable policy commitment and before Core can activate.
                    finalizer(deadline).await?;
                    host.owner.validate_current_selection()?;
                    if admitted.workflow().is_some() {
                        host.private_originals
                            .as_ref()
                            .ok_or(HostLifetimeError::Unavailable)?
                            .check_workflow_class_source(&admitted, &route_for_check)
                            .await?;
                    }
                    if tokio::time::Instant::now() >= deadline
                        || cancelled.load(Ordering::Acquire)
                        || host.private_closed.is_closed()
                        || host.member_class_operations.stopped.load(Ordering::Acquire)
                    {
                        return Err(HostLifetimeError::Denied);
                    }
                    Ok(())
                };
                tokio::pin!(complete);
                let result: Result<(), HostLifetimeError> = tokio::select! { biased;
                    _ = completion.aborted() => {
                        cancelled.store(true, Ordering::Release);
                        if entered.load(Ordering::Acquire) { let _ = complete.await; }
                        admitted.retire().await.and(Err(HostLifetimeError::Denied))
                    }
                    _ = host.member_class_operations.stopped() => {
                        cancelled.store(true, Ordering::Release);
                        if entered.load(Ordering::Acquire) { let _ = complete.await; }
                        admitted.retire().await.and(Err(HostLifetimeError::Denied))
                    }
                    result = &mut complete => result,
                };
                result.map_err(|error| error.to_string())
            })
            .await;
        match native {
            Ok(HeldBootstrapStart::Unsettled(settlement)) => {
                let (send, result) = oneshot::channel();
                let _ = response.send(Ok(MemberClassAddStart::Unsettled(MemberClassSettlement {
                    result,
                })));
                // This observer is always driven, even if neither caller nor
                // private Join remains. Native owns every entered setup step.
                let native = settlement.settled().await;
                let cleanup = admitted
                    .retire()
                    .await
                    .and(self.cleanup_member_mcp_bindings(member_mcp.as_ref()).await);
                let mut outcome = MemberClassAddOutcome {
                    native,
                    authorization: Err(HostLifetimeError::Denied),
                    cleanup,
                };
                let physical = resource_cleanup(
                    resource.as_ref(),
                    MemberClassCleanupObservation::Settled(&outcome),
                )
                .await;
                outcome.cleanup = outcome.cleanup.and(physical);
                let result = outcome.cleanup;
                let _ = send.send(outcome);
                result
            }
            other => {
                let native = match other {
                    Ok(HeldBootstrapStart::Activated(result)) => Ok(result),
                    Err(error) => Err(error),
                    _ => unreachable!(),
                };
                let authorization = self
                    .activate_created_member(&admitted, &identity, &native)
                    .await;
                let cleanup = if authorization.is_err() {
                    admitted
                        .retire()
                        .await
                        .and(self.cleanup_member_mcp_bindings(member_mcp.as_ref()).await)
                } else {
                    Ok(())
                };
                let mut outcome = MemberClassAddOutcome {
                    native,
                    authorization,
                    cleanup,
                };
                let physical = resource_cleanup(
                    resource.as_ref(),
                    MemberClassCleanupObservation::Settled(&outcome),
                )
                .await;
                outcome.cleanup = outcome.cleanup.and(physical);
                let result = outcome.cleanup;
                let _ = response.send(Ok(MemberClassAddStart::Settled(outcome)));
                result
            }
        }
    }

    fn member_class_preflight(
        &self,
        context: &PublicIdentityFirstContext,
        admitted: &AdmittedMemberClass,
        spec: &DurableAgentSpec,
        route: &CompiledNativeSourceRoute,
        controller: &PreparedControllerIntent,
    ) -> Result<(), HostLifetimeError> {
        self.check_member_class_owner(admitted, route, controller)?;
        if self.member_class_operations.stopped.load(Ordering::Acquire) {
            return Err(HostLifetimeError::Unavailable);
        }
        let bound = self
            .private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .bound_identity_runtime()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if !bound
            .owner()
            .handle()
            .actor_task_watch()
            .observes(&context.owner().handle())
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let instance = admitted.instance();
        if admitted.class().purpose.is_ordinary() {
            self.private_originals
                .as_ref()
                .ok_or(HostLifetimeError::Unavailable)?
                .check_operator_member_route(&instance.member, route, &spec.identity)?;
        }
        let alias =
            meerkat_mobkit::member_comms_id::roster_member_id_for_identity(spec.identity.as_str());
        if admitted.workflow().is_some()
            && (spec.addressability
                != meerkat_mobkit::identity_first::AgentAddressability::InternalOnly
                || spec.context.is_some()
                || spec.binding.is_some())
        {
            return Err(HostLifetimeError::Denied);
        }
        if alias.as_str() != instance.member.member.member
            || spec.profile.as_str() != instance.member.member.role
            || context.owner().handle().definition().id.as_str() != instance.member.member.mob_id
            || spec.initial_message.is_some()
            || spec.placement.is_some()
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    async fn activate_created_member(
        &self,
        admitted: &AdmittedMemberClass,
        identity: &AgentIdentity,
        native: &Result<RestoreFlowResult, IdentityRuntimeError>,
    ) -> Result<(), HostLifetimeError> {
        let result = native.as_ref().map_err(|_| HostLifetimeError::Denied)?;
        if result.outcomes.len() != 1 {
            return Err(HostLifetimeError::Denied);
        }
        let Some(RestoreOutcome::Created { binding, .. }) = result.outcomes.get(identity) else {
            return Err(HostLifetimeError::Denied);
        };
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed()
            || self.member_class_operations.stopped.load(Ordering::Acquire)
        {
            return Err(HostLifetimeError::Denied);
        }
        self.private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .activate_member_class(admitted, binding)
            .await?;
        // A close that won during the entered CAS narrows through the same
        // owner; the actual Created outcome is never rewritten as native Abort.
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed()
            || self.member_class_operations.stopped.load(Ordering::Acquire)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    fn check_member_class_owner(
        &self,
        admitted: &AdmittedMemberClass,
        route: &CompiledNativeSourceRoute,
        controller: &PreparedControllerIntent,
    ) -> Result<(), HostLifetimeError> {
        if self.private_closed.is_closed() {
            return Err(HostLifetimeError::Denied);
        }
        self.owner.validate_current_selection()?;
        let originals = self
            .private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        originals.check_member_class_owner(admitted)?;
        let native = self
            .owner
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        if native.routes.iter().any(|selected| selected.id == route.id)
            || native
                .members
                .iter()
                .any(|member| member.member.member == admitted.instance().member.member.member)
            || !native.member_classes.contains(admitted.class())
            || route.application_parent.is_some()
            || route.controller_parent.is_some()
            || controller.target
                != (PreparedControllerTarget::OriginalRoute {
                    route: route.id.clone(),
                })
            || !matches!(&route.origin, CompiledNativeSourceOrigin::LocalConsole { member, .. }
                if member == &admitted.instance().member.member)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
}
