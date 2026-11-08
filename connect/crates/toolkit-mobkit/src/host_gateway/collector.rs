//! One collector occurrence, dispatched by Core through an owned MCP router.
//! The registry below owns physical cleanup only. Enrollment and every call's
//! authority remain with the canonical source, native grants and ServiceCall.

use super::*;
use crate::installed_host::operator_setup::CollectorSetupHost;
use crate::private_originals::collector_setup::{CollectorSetupError, ResolvedCollectorSetup};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_core::{
    AgentToolDispatcher, McpOperationObservation, OperationAuthorizationError, ToolCallView,
    ToolCatalogEntry, ToolExecutionResolutionContext, mcp_config::McpTransportConfig,
};
use meerkat_mcp::{
    HttpEgressProfile, McpRouterAdapter, McpRouterShutdownObservation, McpRouterShutdownObserver,
    McpStdioLaunchProfile,
};
use meerkat_runtime::{
    ServiceCallAuditKey,
    input_authority::{ServiceCallRequest, ServiceOccurrence},
    meerkat_machine::{ServiceCall, ServiceCallOutcome},
};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum CollectorOpenRefusal {
    #[error(transparent)]
    Setup(#[from] CollectorSetupError),
    #[error("collector admission is closed")]
    Closed,
    #[error("collector launch configuration is unsupported")]
    UnsupportedConfiguration,
    #[error("collector connection is unavailable")]
    ConnectionUnavailable,
    #[error("collector operations are not ready")]
    NotReady,
    #[error("native collector admission refused")]
    Native(OperationAuthorizationError),
}

/// Opening failure with the actual native cleanup observation, when a router
/// was registered. No physical connection is implied by an absent observation.
#[derive(Debug, thiserror::Error)]
#[error("{refusal}")]
pub struct CollectorOpenError {
    pub refusal: CollectorOpenRefusal,
    pub cleanup: Option<McpRouterShutdownObservation>,
}

#[derive(Debug, thiserror::Error)]
pub enum CollectorReadError {
    #[error(transparent)]
    Setup(#[from] CollectorSetupError),
    #[error("collector admission is closed")]
    Closed,
}

#[derive(Default)]
struct Adapters {
    stopped: bool,
    adapters: Vec<Arc<McpRouterAdapter>>,
}

/// Real router custody, never a permission, occurrence or audit registry.
#[derive(Default)]
pub(super) struct CollectorAdapters(Mutex<Adapters>);

fn settled(observation: &McpRouterShutdownObservation) -> bool {
    matches!(observation, McpRouterShutdownObservation::Reported(report)
        if report.is_settled() && report.guards_completed())
}

impl CollectorAdapters {
    fn register(
        &self,
        adapter: &Arc<McpRouterAdapter>,
        check: impl FnOnce() -> Result<(), CollectorOpenRefusal>,
    ) -> Result<(), CollectorOpenRefusal> {
        let mut state = self.0.lock().map_err(|_| CollectorOpenRefusal::Closed)?;
        if state.stopped {
            return Err(CollectorOpenRefusal::Closed);
        }
        // The same lock linearizes registration against stop/close. Recheck
        // the actual host here, not only before awaited setup resolution.
        check()?;
        state.adapters.retain(|adapter| {
            !adapter
                .shutdown_observer()
                .current()
                .as_ref()
                .is_some_and(settled)
        });
        state.adapters.push(adapter.clone());
        Ok(())
    }

    fn open(&self) -> bool {
        self.0.lock().is_ok_and(|state| !state.stopped)
    }

    pub(super) fn stop(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stopped = true;
    }

    pub(super) fn begin_close(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stopped = true;
        for adapter in &state.adapters {
            drop(adapter.begin_close());
        }
    }

    pub(super) async fn join(&self) -> Result<(), GatewayHostError> {
        self.begin_close();
        let adapters = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .adapters
            .clone();
        // Do not stop at the first incomplete report. Core retains each actual
        // shutdown task if this observer is cancelled or times out.
        let observations = futures::future::join_all(
            adapters
                .iter()
                .map(|adapter| adapter.shutdown_with_report()),
        )
        .await;
        if observations.iter().all(settled) {
            Ok(())
        } else {
            Err(GatewayHostError::SettlementIncomplete)
        }
    }
}

impl Drop for CollectorAdapters {
    fn drop(&mut self) {
        let state = self
            .0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for adapter in &state.adapters {
            drop(adapter.begin_close());
        }
    }
}

struct AdapterLease(Arc<McpRouterAdapter>);
impl Drop for AdapterLease {
    fn drop(&mut self) {
        // Native close starts synchronously even if opening/read observation
        // was cancelled. The host retains the adapter for an explicit join.
        drop(self.0.begin_close());
    }
}

/// A process-local occurrence. This handle is not Clone and creates no input,
/// scheduler admission or provider delivery. Core owns its one ServiceCall.
/// Drop starts physical close; `close` or host `join_close` observes completion.
pub struct CollectorOccurrence {
    gateway: Arc<ToolkitGatewayHost>,
    host: Arc<CollectorSetupHost>,
    selected: ResolvedCollectorSetup,
    service: ServiceCall,
    adapter: AdapterLease,
}

impl ToolkitGatewayHost {
    /// Open the exact enrolled installation for one trusted-host occurrence.
    /// The occurrence identifiers are native audit data, not proof that a
    /// schedule fired. The host explicitly selects both execution profiles and
    /// a finite positive connection observation budget. Missing credentials
    /// remain a native refusal; this method starts no login or browser.
    ///
    /// This first slice refuses stdio `secret_env`: the factory offer does not
    /// expose the realm's launch-secret resolver. No ambient credential fallback
    /// or copied token/configuration store is installed.
    pub async fn open_collector_occurrence(
        self: &Arc<Self>,
        host: Arc<CollectorSetupHost>,
        setup: EvidenceId,
        occurrence: ServiceOccurrence,
        launch: McpStdioLaunchProfile,
        egress: HttpEgressProfile,
        ready_timeout: Duration,
    ) -> Result<CollectorOccurrence, CollectorOpenError> {
        let before_router = async {
            self.check_collector_open()?;
            let originals = self
                .private_originals
                .as_ref()
                .ok_or(CollectorSetupError::Unavailable)?;
            let selected = originals.resolve_collector_setup(&setup, &host).await?;
            check_launch(selected.config(), ready_timeout)?;
            let machine = originals
                .machine()
                .map_err(|_| CollectorSetupError::Unavailable)?;
            if !self
                .machine
                .get()
                .is_some_and(|actual| Arc::ptr_eq(actual, &machine))
            {
                return Err(CollectorSetupError::Denied.into());
            }
            // Core constructs the RuntimeStore audit sink. This uses the
            // native audit-owner API from Core0768728, not the older selected
            // Core0a interface; no compatibility sink belongs in Connect.
            let request =
                ServiceCallRequest::from_trusted_host(selected.association()?, occurrence)
                    .map_err(|refusal| {
                        CollectorOpenRefusal::Native(OperationAuthorizationError::Refused(refusal))
                    })?;
            let service = machine
                .prepare_service_call(request)
                .map_err(CollectorOpenRefusal::Native)?;
            let router = host.collector_router(launch, egress)?;
            Ok::<_, CollectorOpenRefusal>((selected, service, router))
        }
        .await
        .map_err(|refusal| CollectorOpenError {
            refusal,
            cleanup: None,
        })?;
        let (selected, service, router) = before_router;
        let adapter = AdapterLease(Arc::new(McpRouterAdapter::new(router)));
        let opened = async {
            // Nothing has been staged or spawned before this registration.
            self.collector_adapters.register(&adapter.0, || {
                if self.private_closed.is_closed() {
                    return Err(CollectorOpenRefusal::Closed);
                }
                self.owner
                    .validate_current_selection()
                    .map_err(|_| CollectorOpenRefusal::Closed)
            })?;
            connect_selected(
                &adapter.0,
                selected.config(),
                selected.operations(),
                ready_timeout,
            )
            .await?;
            self.check_collector_open()?;
            let current = self
                .private_originals
                .as_ref()
                .ok_or(CollectorSetupError::Unavailable)?
                .resolve_collector_setup(selected.id(), &host)
                .await?;
            check_same_selection(&selected, &current)?;
            self.check_collector_open()?;
            Ok::<_, CollectorOpenRefusal>(())
        }
        .await;
        match opened {
            Ok(()) => Ok(CollectorOccurrence {
                gateway: self.clone(),
                host,
                selected,
                service,
                adapter,
            }),
            Err(refusal) => {
                let cleanup = adapter.0.shutdown_with_report().await;
                Err(CollectorOpenError {
                    refusal,
                    cleanup: Some(cleanup),
                })
            }
        }
    }

    fn check_collector_open(&self) -> Result<(), CollectorOpenRefusal> {
        if !self.collector_adapters.open() || self.private_closed.is_closed() {
            return Err(CollectorOpenRefusal::Closed);
        }
        self.owner
            .validate_current_selection()
            .map_err(|_| CollectorOpenRefusal::Closed)
    }
}

fn check_same_selection(
    expected: &ResolvedCollectorSetup,
    current: &ResolvedCollectorSetup,
) -> Result<(), CollectorSetupError> {
    if expected.config() != current.config() || expected.association()? != current.association()? {
        return Err(CollectorSetupError::InstallationChanged);
    }
    Ok(())
}

fn check_launch(
    config: &meerkat_core::McpServerConfig,
    timeout: Duration,
) -> Result<(), CollectorOpenRefusal> {
    if timeout.is_zero()
        || tokio::time::Instant::now().checked_add(timeout).is_none()
        || matches!(&config.transport, McpTransportConfig::Stdio(stdio) if !stdio.secret_env.is_empty())
    {
        return Err(CollectorOpenRefusal::UnsupportedConfiguration);
    }
    Ok(())
}

/// Physical connection only. Its caller has already resolved the canonical
/// selection and registered custody; this helper supplies no call authority.
async fn connect_selected(
    adapter: &McpRouterAdapter,
    config: &meerkat_core::McpServerConfig,
    operations: &std::collections::BTreeSet<String>,
    ready_timeout: Duration,
) -> Result<(), CollectorOpenRefusal> {
    adapter
        .stage_add(config.clone())
        .await
        .map_err(|_| CollectorOpenRefusal::ConnectionUnavailable)?;
    adapter
        .apply_staged()
        .await
        .map_err(|_| CollectorOpenRefusal::ConnectionUnavailable)?;
    adapter
        .wait_until_ready(ready_timeout)
        .await
        .map_err(|_| CollectorOpenRefusal::NotReady)?;
    let binding = config
        .connector
        .as_ref()
        .ok_or(CollectorSetupError::Denied)?;
    for operation in operations {
        match adapter.observe_operation(&config.name, operation).await {
            McpOperationObservation::Callable {
                connector: Some(actual),
                ..
            } if actual.realm_id == binding.realm_id
                && actual.slot_id == binding.slot_id
                && actual.issuer == binding.issuer
                && actual.client == binding.client
                && actual.account == binding.account => {}
            _ => return Err(CollectorOpenRefusal::NotReady),
        }
    }
    Ok(())
}

impl CollectorOccurrence {
    /// Native catalog projection for constructing genuine calls. This is not
    /// permission: the canonical enrollment and Core gate decide each read.
    pub fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.adapter.0.tool_catalog()
    }

    /// Resolve the current setup before entry, then retain Core's complete
    /// result and audit disposition. A failed outcome append does not turn a
    /// completed provider call into a refusal or authorize an automatic retry.
    pub async fn read(
        &mut self,
        call: ToolCallView<'_>,
        resolution: ToolExecutionResolutionContext,
    ) -> Result<ServiceCallOutcome, CollectorReadError> {
        self.gateway
            .check_collector_open()
            .map_err(|_| CollectorReadError::Closed)?;
        let current = self
            .gateway
            .private_originals
            .as_ref()
            .ok_or(CollectorSetupError::Unavailable)?
            .resolve_collector_setup(self.selected.id(), &self.host)
            .await?;
        check_same_selection(&self.selected, &current)?;
        self.gateway
            .check_collector_open()
            .map_err(|_| CollectorReadError::Closed)?;
        // The adapter supplies the real target, connection and account. Core
        // rechecks source/grants/operation at entry. Never replace its returned
        // outcome with a later configuration or currentness observation.
        Ok(self
            .service
            .dispatch(&self.adapter.0, call, resolution)
            .await)
    }

    /// The native store key for this occurrence's append-only audit rows.
    pub fn audit_key(&self) -> &ServiceCallAuditKey {
        self.service.audit_key()
    }

    pub fn shutdown_observer(&self) -> McpRouterShutdownObserver {
        self.adapter.0.shutdown_observer()
    }

    pub async fn close(&self) -> McpRouterShutdownObservation {
        self.adapter.0.shutdown_with_report().await
    }
}

#[cfg(test)]
pub(crate) mod tests;
