//! Existing fixture ports may be prepared before their enrolled leaves bind.
//! The report-only variant waits for the actual helper session before selecting
//! its native operation grant; it never submits an input while unbound.
use super::*;
use acquisition::{
    InstalledAcquisition, InstalledNativeContinuation, InstalledSetupDefaults,
    InstalledSetupLimits,
};
use meerkat_core::{
    service::WorkGraphNamespaceGrant,
    storage_durability::{DurabilityDeclaration, DurabilityResolution},
};
use meerkat_toolkit::{
    authority::Ceiling,
    setup::{SetupWorkBackend, WorkAdmissionStepOptions},
};
use meerkat_workgraph::{SqliteWorkGraphStore, WorkGraphService};

pub(crate) struct ApprovedAcquisitionPorts {
    pub continuation: Arc<acquisition::InstalledNativeContinuation>,
    pub workgraph: meerkat_workgraph::WorkGraphService,
    work_backend: meerkat_toolkit::setup::SetupWorkBackend,
    store: Arc<SqliteStore>,
    scope: StoreScope,
    originals: Arc<PrivateOriginals>,
    native: meerkat_toolkit::host_lifetime::policy::CompiledNativeHost,
}

pub(crate) fn prepare_approved_acquisition(host: &Host) -> ApprovedAcquisitionPorts {
    let id = |value: &str| toolkit_package::Id::new(value).unwrap();
    let native = host
        .lifetime
        .selected_policy()
        .native_host
        .as_ref()
        .unwrap();
    let continuation = InstalledNativeContinuation::new(
        host.store.clone(),
        host.source_scope.clone(),
        host.originals.clone(),
        id("approved-acquisition-turn"),
        [id("person")].into(),
    )
    .unwrap();
    let workgraph = WorkGraphService::with_namespace_grant(
        Arc::new(
            SqliteWorkGraphStore::open(
                &host.source_path.with_file_name("approved-workgraph.sqlite"),
            )
            .unwrap(),
        ),
        WorkGraphNamespaceGrant::new(native.realm.as_str(), "setup").unwrap(),
    )
    .unwrap();
    let work_backend = SetupWorkBackend::new(
        workgraph.clone(),
        "fixture".into(),
        id("generation"),
        &[DurabilityDeclaration::durable(
            "workgraph",
            DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    ApprovedAcquisitionPorts {
        continuation, workgraph, work_backend, store: host.store.clone(),
        scope: host.source_scope.clone(), originals: host.originals.clone(), native: native.clone(),
    }
}

impl ApprovedAcquisitionPorts {
    pub fn bind(
        &self,
        tools: acquisition::InstalledAcquisitionTools,
        helper: acquisition::InstalledSetupSelection,
    ) -> Arc<acquisition::InstalledAcquisition> {
        self.bind_with_limits(tools, helper, InstalledSetupLimits::default())
    }
    pub fn bind_with_limits(
        &self,
        tools: acquisition::InstalledAcquisitionTools,
        helper: acquisition::InstalledSetupSelection,
        limits: InstalledSetupLimits,
    ) -> Arc<acquisition::InstalledAcquisition> {
    let id = |value: &str| toolkit_package::Id::new(value).unwrap();
    let owner = Arc::new(
        InstalledAcquisition::from_defaults_with_tools(
            self.store.clone(),
            self.scope.clone(),
            self.originals.clone(),
            &self.native,
            InstalledSetupDefaults {
                helper,
                proposal_ceiling: Ceiling {
                    actions: [
                        meerkat_toolkit::authority::ActionPermission {
                            action: meerkat_toolkit::authority::Action::FindAndInstall,
                            resource: id("agent"),
                        },
                        meerkat_toolkit::authority::ActionPermission {
                            action: meerkat_toolkit::authority::Action::Delegate,
                            resource: id("agent"),
                        },
                    ]
                    .into(),
                    services: Default::default(),
                    frontends: Default::default(),
                    output_audiences: [id("person")].into(),
                },
                continuation: self.continuation.clone(),
                work_backend: self.work_backend.clone(),
                references: None,
                limits,
                work_step: WorkAdmissionStepOptions {
                    worker: id("worker"),
                    lease_ms: 15_000,
                    owner_timeout_ms: 5_000,
                    settlement_budget_ms: 5_000,
                },
            },
            tools,
        )
        .unwrap(),
    );
    owner.bind_tool_driver().unwrap();
    owner
    }
}

pub(crate) fn bind_approved_acquisition(
    host: &Host,
    tools: acquisition::InstalledAcquisitionTools,
) -> (
    Arc<acquisition::InstalledAcquisition>,
    Arc<acquisition::InstalledNativeContinuation>,
    meerkat_workgraph::WorkGraphService,
) {
    let ports = prepare_approved_acquisition(host);
    let owner = ports.bind(tools, host.acquisition_helper.clone().unwrap());
    (owner, ports.continuation, ports.workgraph)
}
