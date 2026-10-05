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
    DetachedJobService, JobAwaitCoordinator, JobAwaitDeliverySink, JobDeliveryApplication,
    JobDeliverySink, RuntimeDeliveryHost, RuntimeDeliveryOwner, RuntimeDeliveryOwnerHandle,
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
    async fn apply(&self, application: JobDeliveryApplication) -> Result<(), String> {
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
                .map_err(|error| error.to_string()),
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
                .map_err(|error| error.to_string()),
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
        }
    }
}

#[async_trait::async_trait]
impl<S> RuntimeDeliveryHost for SessionServiceDeliveryHost<S>
where
    S: SessionServiceControlExt + Send + Sync + 'static,
{
    async fn delivery_sink(&self, session_id: &SessionId) -> Option<Arc<dyn JobDeliverySink>> {
        let service = self.service.upgrade()?;
        let runtime = self.runtime.upgrade()?;
        let operations = match &self.realm_id {
            Some(_) => runtime.ops_lifecycle_registry(session_id).await,
            None => None,
        };
        let base: Arc<dyn JobDeliverySink> =
            Arc::new(SessionServiceDeliverySink { service, runtime });
        Some(match (operations, &self.realm_id) {
            (Some(operations), Some(realm_id)) => Arc::new(JobAwaitDeliverySink::new(
                JobAwaitCoordinator::new(realm_id.clone(), self.jobs.clone(), operations),
                base,
            )),
            _ => base,
        })
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
    let host = Arc::new(SessionServiceDeliveryHost::new(
        service, runtime, jobs, realm_id,
    ));
    match owner.arm(host) {
        Ok(handle) => RuntimeDeliveryOwnerHandle::detach(handle),
        Err(RuntimeDeliveryOwnerAlreadyArmed) => {
            tracing::debug!("the runtime delivery inbox already has a delivery owner; keeping it")
        }
    }
}
