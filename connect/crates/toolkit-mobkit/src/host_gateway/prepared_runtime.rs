//! Retain the real native construction across final application publication.
//!
//! This participant is called by the same gateway's held-bootstrap path. It
//! creates no machine, lifetime or WorkGraph owner. The final host adopts the
//! exact owners received from native construction, before the held pass may
//! publish members.

use super::*;
use crate::installed_host::{
    acquisition::{AcquisitionBootstrap, InstalledAcquisitionTools, InstalledNativeContinuation,
        InstalledServiceTaskSource},
    prepared_registration::PreparedMemberRegistration,
};
use meerkat_mobkit::{
    console_governed_ingress::ConsoleGovernedIngress,
    identity_first::{AgentIdentity, GovernedInputBinder},
    runtime::AuthenticatedConsoleHuman,
    storage_health::ResolvedStorageSummary,
};

pub struct PreparedGatewayRuntime {
    originals: Arc<PrivateOriginals>,
    registration: Arc<PreparedMemberRegistration>,
    registry: Arc<meerkat_core::ToolConsequencePolicyRegistry>,
    tools: InstalledAcquisitionTools,
    continuation: Arc<InstalledNativeContinuation>,
    construction: OnceLock<Construction>,
    acquisition: Arc<Mutex<AcquisitionBootstrap>>,
    console: Arc<PreparedConsoleIngress>,
    access: OnceLock<Option<meerkat_mobkit::access::AccessController>>,
}

struct Construction {
    machine: Arc<MeerkatMachine>,
    workgraph: Option<meerkat_workgraph::WorkGraphService>,
    storage: Option<ResolvedStorageSummary>,
}

#[derive(Default)]
struct PreparedConsoleIngress {
    installed: OnceLock<Arc<dyn ConsoleGovernedIngress>>,
}

impl ConsoleGovernedIngress for PreparedConsoleIngress {
    fn governs(&self, identity: &AgentIdentity) -> bool {
        self.installed.get().map_or(true, |ingress| ingress.governs(identity))
    }

    fn binder_for(&self, identity: &AgentIdentity, human: &AuthenticatedConsoleHuman)
        -> Result<Arc<dyn GovernedInputBinder>, String>
    {
        self.installed.get().ok_or("application preparation is not installed")?
            .binder_for(identity, human)
    }
}

// Failure-only local diagnostic. Fixed labels and unit variants contain no owner data.
// Diagnostic I/O cannot change the original refusal or its cleanup path.
fn adoption_refusal(stage: &'static str, error: HostLifetimeError) -> HostLifetimeError {
    use std::io::Write as _;
    let cause = match error {
        HostLifetimeError::InvalidSelection => "InvalidSelection",
        HostLifetimeError::Unavailable => "Unavailable",
        HostLifetimeError::Denied => "Denied",
        HostLifetimeError::WrongOwner => "WrongOwner",
        HostLifetimeError::NotSettled => "NotSettled",
    };
    let _ = writeln!(std::io::stderr().lock(),
        "{{\"event\":\"toolkit_application_adoption_refused\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}");
    error
}

impl PreparedGatewayRuntime {
    pub(crate) fn new(
        originals: Arc<PrivateOriginals>, registration: Arc<PreparedMemberRegistration>,
        registry: Arc<meerkat_core::ToolConsequencePolicyRegistry>,
        tools: InstalledAcquisitionTools, continuation: Arc<InstalledNativeContinuation>,
    ) -> Self {
        Self { originals, registration, registry, tools, continuation,
            construction: OnceLock::new(), acquisition: Arc::new(Mutex::new(AcquisitionBootstrap::default())),
            console: Arc::new(PreparedConsoleIngress::default()), access: OnceLock::new() }
    }

    /// The actual factory's account adapter keeps this same weak owner. It is
    /// unavailable while preparation has no installed acquisition.
    pub fn service_tasks(&self) -> InstalledServiceTaskSource {
        InstalledServiceTaskSource::new(&self.acquisition)
    }

    pub fn tools(&self) -> InstalledAcquisitionTools { self.tools.clone() }

    pub fn continuation(&self) -> Arc<InstalledNativeContinuation> { self.continuation.clone() }

    pub fn prepare_bootstrap(&self, spec: MobBootstrapSpec)
        -> Result<MobBootstrapSpec, GatewayHostError>
    {
        let machine = spec.session_service.acquire_runtime_adapter(spec.runtime_adapter.clone())
            .map_err(|_| GatewayHostError::Unavailable)?.ok_or(GatewayHostError::Unavailable)?;
        if spec.runtime_adapter.as_ref().is_some_and(|selected| !Arc::ptr_eq(selected, &machine)) {
            return Err(GatewayHostError::Denied);
        }
        self.originals.bind_machine(machine.clone()).map_err(|_| GatewayHostError::Denied)?;
        self.construction.set(Construction {
            machine, workgraph: spec.workgraph_service(), storage: spec.resolved_storage.clone(),
        }).map_err(|_| GatewayHostError::Denied)?;
        let source = meerkat_mob_mcp::HostJobSource::new(
            [self.continuation.namespace().to_owned()], self.continuation.clone(),
        ).map_err(|_| GatewayHostError::Denied)?;
        let spec = spec.with_host_job_source(source).map_err(|_| GatewayHostError::Unavailable)?;
        Ok(spec.with_member_runtime_registration_hook(self.registration.clone())
            .with_tool_consequence_policy_registry(self.registry.clone())
            .register_tool_bundle(crate::FIND_AND_INSTALL_BUNDLE, self.tools.acquisition_tool())
            .register_tool_bundle(meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE, self.tools.result_tool()))
    }

    pub fn bind_runtime(&self, runtime: &UnifiedRuntime) -> Result<(), GatewayHostError> {
        let expected = self.construction.get().ok_or(GatewayHostError::Unavailable)?;
        let actual = runtime.mob_runtime().session_service().ok_or(GatewayHostError::Unavailable)?
            .acquire_runtime_adapter(Some(expected.machine.clone()))
            .map_err(|_| GatewayHostError::Unavailable)?.ok_or(GatewayHostError::Unavailable)?;
        if !Arc::ptr_eq(&expected.machine, &actual) { return Err(GatewayHostError::Denied); }
        self.continuation.bind_native(runtime.mob_runtime()).map_err(host_error)
    }

    // Observe only this Toolkit-owned object in empty-roster hook controls.
    // This is not a native ingress getter or a production readiness surface.
    #[cfg(test)]
    pub(crate) fn console_reference_count_for_test(&self) -> usize {
        Arc::strong_count(&self.console)
    }

    pub fn bind_identity_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        let identities = runtime.identity_runtime().ok_or(GatewayHostError::Unavailable)?;
        if let Some(access) = runtime.access_controller() {
            access.join_authorization_publication(self.originals.authorization_publication())
                .map_err(|_| GatewayHostError::Denied)?;
        }
        self.originals.bind_identity_runtime(&identities).map_err(|_| GatewayHostError::Denied)?;
        self.originals.start_custody_recovery(runtime.mob_runtime().continuation_source_announcer())
            .map_err(|_| GatewayHostError::Unavailable)?;
        self.access.set(runtime.access_controller().cloned()).map_err(|_| GatewayHostError::Denied)?;
        // The native router retains this same participant. Until final owner
        // adoption it refuses; it cannot select an ungoverned Console path.
        runtime.set_console_governed_ingress(self.console.clone());
        Ok(())
    }

    /// Stop the same source-owned recovery task even if final host adoption
    /// never succeeds. Stop confers no native cancellation acknowledgement.
    pub fn begin_custody_close(&self) { self.originals.stop_custody_recovery(); }

    /// Retain the entered task across cancellation of this join. Adoption and
    /// prepared failure paths share the same idempotent joined result.
    pub async fn join_custody_close(&self) -> Result<(), GatewayHostError> {
        self.originals.join_custody_recovery().await.map_err(|_| GatewayHostError::Unavailable)
    }

    /// Adopt the already bound native construction after exact compiler output
    /// and final selection validation. This does not run bootstrap twice.
    pub async fn adopt(&self, mut host: ToolkitGatewayHost) -> Result<ToolkitGatewayHost, HostLifetimeError> {
        let construction = self.construction.get()
            .ok_or_else(|| adoption_refusal("runtime.construction", HostLifetimeError::Unavailable))?;
        if host.machine.get().is_some() {
            return Err(adoption_refusal("runtime.machine_unbound", HostLifetimeError::InvalidSelection));
        }
        if !host.private_originals.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &self.originals)) {
            return Err(adoption_refusal("runtime.same_originals", HostLifetimeError::InvalidSelection));
        }
        if !host.installed_tool_policy.as_ref().is_some_and(|registry| Arc::ptr_eq(registry, &self.registry)) {
            return Err(adoption_refusal("runtime.same_registry", HostLifetimeError::InvalidSelection));
        }
        if !host.native_continuation.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &self.continuation)) {
            return Err(adoption_refusal("runtime.same_continuation", HostLifetimeError::InvalidSelection));
        }
        host.owner.validate_current_selection()
            .map_err(|error| adoption_refusal("runtime.current_before", error))?;
        self.originals.bind_host_lifetime(&host.owner)
            .map_err(|_| adoption_refusal("runtime.bind_host_lifetime", HostLifetimeError::InvalidSelection))?;
        self.originals.bound_identity_runtime()
            .map_err(|_| adoption_refusal("runtime.bound_identity_runtime", HostLifetimeError::Unavailable))?;
        {
            let mut selected = host.acquisition.lock()
                .map_err(|_| adoption_refusal("runtime.selected_acquisition_lock", HostLifetimeError::Unavailable))?;
            let mut retained = self.acquisition.lock()
                .map_err(|_| adoption_refusal("runtime.retained_acquisition_lock", HostLifetimeError::Unavailable))?;
            if !retained.is_unconfigured() {
                return Err(adoption_refusal("runtime.acquisition_unconfigured", HostLifetimeError::InvalidSelection));
            }
            *retained = std::mem::take(&mut *selected);
            retained.prepare(construction.workgraph.clone(), construction.storage.as_ref())
                .map_err(|error| adoption_refusal("runtime.prepare_acquisition", error))?;
        }
        host.acquisition = self.acquisition.clone();
        host.machine.set(construction.machine.clone())
            .map_err(|_| adoption_refusal("runtime.install_machine", HostLifetimeError::InvalidSelection))?;
        host.prepared_application_selected.set(())
            .map_err(|_| adoption_refusal("runtime.select_prepared_application", HostLifetimeError::InvalidSelection))?;
        let logins = host.console_logins.as_ref()
            .ok_or_else(|| adoption_refusal("runtime.console_logins", HostLifetimeError::InvalidSelection))?;
        let access = self.access.get()
            .ok_or_else(|| adoption_refusal("runtime.access_controller", HostLifetimeError::Unavailable))?.clone();
        let ingress = self.originals.console_ingress(logins.clone(), access).await
            .map_err(|_| adoption_refusal("runtime.compose_console_ingress", HostLifetimeError::Unavailable))?;
        host.owner.validate_current_selection()
            .map_err(|error| adoption_refusal("runtime.current_after", error))?;
        if host.private_closed.is_closed() {
            return Err(adoption_refusal("runtime.private_open", HostLifetimeError::Denied));
        }
        self.console.installed.set(ingress)
            .map_err(|_| adoption_refusal("runtime.install_console_ingress", HostLifetimeError::InvalidSelection))?;
        Ok(host)
    }
}
