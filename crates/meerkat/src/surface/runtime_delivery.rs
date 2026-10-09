//! Default durable job delivery for facade-built surfaces.
//!
//! [`build_runtime_backed_service_with_default_reconfigure_host`] arms the
//! bundle's [`RuntimeDeliveryOwner`] with [`SessionServiceDeliveryHost`], so
//! every surface built through that composition applies job deliveries.
//!
//! [`build_runtime_backed_service_with_default_reconfigure_host`]:
//!     super::build_runtime_backed_service_with_default_reconfigure_host

use std::sync::{Arc, Weak};

use meerkat_core::SessionId;
use meerkat_core::service::SessionServiceControlExt;
use meerkat_runtime::{MeerkatMachine, RuntimeDeliveryOwnerAlreadyArmed};

use crate::{
    DeliveryRoute, DetachedJobService, JobAwaitCoordinator, JobAwaitDeliverySink,
    JobDeliveryApplication, JobDeliverySink, RuntimeDeliveryHost, RuntimeDeliveryOwner,
    RuntimeDeliveryOwnerHandle,
};

/// Applies job deliveries through a session service and its runtime.
///
/// `Notification` appends an ordered System message through the service,
/// which reaches live and persisted sessions. `Event` admits a durable
/// external event through the runtime's waking path, so an idle registered
/// session starts a turn; a session the runtime has not registered refuses it,
/// and the row stays pending until an attachment commit retries it.
pub struct SessionServiceDeliverySink<S> {
    service: Arc<S>,
    runtime: Arc<MeerkatMachine>,
}

impl<S> SessionServiceDeliverySink<S> {
    pub fn new(service: Arc<S>, runtime: Arc<MeerkatMachine>) -> Self {
        Self { service, runtime }
    }
}

#[async_trait::async_trait]
impl<S> JobDeliverySink for SessionServiceDeliverySink<S>
where
    S: SessionServiceControlExt + Send + Sync + 'static,
{
    async fn apply(
        &self,
        application: JobDeliveryApplication,
    ) -> Result<(), crate::JobDeliveryApplyError> {
        match application {
            JobDeliveryApplication::Record { .. } => Ok(()),
            JobDeliveryApplication::Notification {
                job_id,
                delivery_sequence,
                subscription,
                content,
            } => self
                .service
                .append_system_context(
                    subscription.session_id(),
                    crate::job_delivery_notification_request(
                        &job_id,
                        delivery_sequence,
                        &subscription,
                        &content,
                    ),
                )
                .await
                .map(|_| ())
                .map_err(|error| match error {
                    meerkat_core::SessionControlError::Authorization(error) => {
                        crate::JobDeliveryApplyError::Authorization(error)
                    }
                    meerkat_core::SessionControlError::Review(error) => {
                        crate::JobDeliveryApplyError::Review(error)
                    }
                    // Another runtime owner hosts the recipient (#1813): its
                    // store-only write was refused; a skip, never a block.
                    meerkat_core::SessionControlError::Session(
                        meerkat_core::SessionError::ServedElsewhere { id },
                    ) => crate::JobDeliveryApplyError::ServedElsewhere { session_id: id },
                    // Its claim is unavailable: nothing was written unclaimed.
                    meerkat_core::SessionControlError::Session(
                        meerkat_core::SessionError::HostingUnavailable { id },
                    ) => crate::JobDeliveryApplyError::HostingUnavailable { session_id: id },
                    other => crate::JobDeliveryApplyError::Infrastructure(other.to_string()),
                }),
            JobDeliveryApplication::Event {
                job_id,
                delivery_sequence,
                subscription,
                interaction_lineage_id,
                handling_mode,
                content,
            } => self
                .runtime
                .accept_input_with_completion(
                    subscription.session_id(),
                    crate::job_delivery_event_input(
                        &job_id,
                        delivery_sequence,
                        &subscription,
                        &interaction_lineage_id,
                        handling_mode,
                        &content,
                    ),
                )
                .await
                .map(|_| ())
                .map_err(|error| match error {
                    meerkat_runtime::RuntimeDriverError::InputRefused { refusal } => {
                        crate::JobDeliveryApplyError::Authorization(refusal.into())
                    }
                    meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                        ..
                    } => crate::JobDeliveryApplyError::Authorization(
                        meerkat_core::OperationAuthorizationError::Unavailable,
                    ),
                    // Another runtime owner hosts the recipient (#1813): its
                    // registration was refused; a skip, never a block.
                    meerkat_runtime::RuntimeDriverError::ServedElsewhere { session_id } => {
                        crate::JobDeliveryApplyError::ServedElsewhere { session_id }
                    }
                    meerkat_runtime::RuntimeDriverError::HostingUnavailable { session_id } => {
                        crate::JobDeliveryApplyError::HostingUnavailable { session_id }
                    }
                    other => crate::JobDeliveryApplyError::Infrastructure(other.to_string()),
                }),
        }
    }
}

/// Delivery host over a session service and its runtime. Holds both weakly:
/// the owner stops once either is gone.
pub struct SessionServiceDeliveryHost<S> {
    service: Weak<S>,
    runtime: Weak<MeerkatMachine>,
    jobs: DetachedJobService,
    realm_id: Option<String>,
    continuations: Arc<crate::ContinuationHostBindings>,
}

impl<S> SessionServiceDeliveryHost<S> {
    /// `realm_id` scopes job-await closure; without it deliveries still apply
    /// but no job-await operation is closed by them.
    pub fn new(
        service: &Arc<S>,
        runtime: &Arc<MeerkatMachine>,
        jobs: DetachedJobService,
        realm_id: Option<String>,
    ) -> Self {
        Self {
            service: Arc::downgrade(service),
            runtime: Arc::downgrade(runtime),
            jobs,
            realm_id,
            continuations: Arc::default(),
        }
    }

    /// Resolve member addresses and confirm retained completions through
    /// `continuations`, bound by the host once its mob runtime exists.
    #[must_use]
    pub fn with_continuation_bindings(
        mut self,
        continuations: Arc<crate::ContinuationHostBindings>,
    ) -> Self {
        self.continuations = continuations;
        self
    }
}

#[async_trait::async_trait]
impl<S> RuntimeDeliveryHost for SessionServiceDeliveryHost<S>
where
    S: SessionServiceControlExt + Send + Sync + 'static,
{
    async fn continuation_sink(
        &self,
        _session_id: &SessionId,
    ) -> Option<Arc<dyn crate::ContinuationDeliverySink>> {
        let runtime = self.runtime.upgrade()?;
        Some(Arc::new(crate::MachineContinuationSink::new(runtime)))
    }

    async fn resolve_address(
        &self,
        address: &meerkat_runtime::LogicalRuntimeId,
    ) -> crate::AddressResolution {
        self.continuations.resolve_address(address).await
    }

    fn retained_job_source(&self) -> Option<Arc<dyn crate::RetainedJobSource>> {
        self.continuations.job_source()
    }

    async fn delivery_route(&self, session_id: &SessionId) -> Option<DeliveryRoute> {
        let service = self.service.upgrade()?;
        let runtime = self.runtime.upgrade()?;
        // #1813: routed from this runtime owner's claim registry only. A
        // session another runtime owner of this process hosts is that
        // owner's to deliver.
        let serving = runtime.session_serving(session_id);
        if serving == meerkat_runtime::SessionServing::HeldByAnotherLocalOwner {
            return Some(DeliveryRoute::ServedElsewhere);
        }
        let operations = match &self.realm_id {
            Some(_) => runtime.ops_lifecycle_registry(session_id).await,
            None => None,
        };
        let base: Arc<dyn JobDeliverySink> =
            Arc::new(SessionServiceDeliverySink { service, runtime });
        let sink: Arc<dyn JobDeliverySink> = match (operations, &self.realm_id) {
            (Some(operations), Some(realm_id)) => Arc::new(JobAwaitDeliverySink::new(
                JobAwaitCoordinator::new(realm_id.clone(), self.jobs.clone(), operations),
                base,
            )),
            _ => base,
        };
        Some(match serving {
            meerkat_runtime::SessionServing::HeldHere => DeliveryRoute::ServedHere(sink),
            meerkat_runtime::SessionServing::NotHeldInThisProcess => DeliveryRoute::Unserved(sink),
            meerkat_runtime::SessionServing::HeldByAnotherLocalOwner => {
                DeliveryRoute::ServedElsewhere
            }
        })
    }

    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
        Some(self.runtime.upgrade()?.grant_session_hosting(session_id))
    }
}

/// Arm `owner` for a facade-built service and detach it: the owner runs until
/// the service or its runtime is gone. Outside a tokio runtime nothing is
/// armed, and a second arming over the same inbox is refused.
pub(crate) fn arm_default_runtime_delivery<S>(
    owner: RuntimeDeliveryOwner,
    service: &Arc<S>,
    runtime: &Arc<MeerkatMachine>,
    realm_id: Option<String>,
    continuations: Arc<crate::ContinuationHostBindings>,
) where
    S: SessionServiceControlExt + Send + Sync + 'static,
{
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::warn!(
            "no tokio runtime while building the service; durable job delivery not armed"
        );
        return;
    }
    let jobs = DetachedJobService::new(owner.job_store());
    let host = Arc::new(
        SessionServiceDeliveryHost::new(service, runtime, jobs, realm_id)
            .with_continuation_bindings(continuations),
    );
    match owner.arm(host) {
        Ok(handle) => RuntimeDeliveryOwnerHandle::detach(handle),
        Err(RuntimeDeliveryOwnerAlreadyArmed) => {
            tracing::debug!("the runtime delivery inbox already has a delivery owner; keeping it");
        }
    }
}
