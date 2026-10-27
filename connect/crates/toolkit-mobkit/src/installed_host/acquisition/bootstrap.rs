//! Optional acquisition selection from the actual pre-input MobKit bootstrap.
//! This module opens no backend and supplies no native execution authority.
use super::{
    InstalledAcquisition, InstalledOriginalTaskOwner, InstalledSetupDefaults, InstalledSetupLimits,
    InstalledSetupSelection, InstalledAcquisitionTools,
};
use crate::private_originals::PrivateOriginals;
use meerkat_core::storage_durability::{DurabilityClass, DurabilityResolution};
use meerkat_mobkit::storage_health::ResolvedStorageSummary;
use meerkat_toolkit::{
    authority::Ceiling,
    host_lifetime::{control::HostLifetimeError, policy::CompiledNativeHost},
    request::ContextReferenceOwner,
    setup::{SetupWorkBackend, WorkAdmissionStepOptions},
};
use meerkat_workgraph::WorkGraphService;
use std::sync::Arc;
use toolkit_package::Id;
use toolkit_store::{SqliteStore, StoreScope};

/// Trusted application opt-in before MobBootstrapSpec exists. The existing
/// bootstrap supplies its selected service and unchanged provider-open census.
/// The application retains backend_generation across restart and changes it on
/// replacement unless continuity is established. This is not derived from a
/// path, provider label or service pointer, and resource preparation cannot
/// issue it. The original invocation owner supplies continuation at admission.
pub struct InstalledSetupBootstrap {
    pub helper: InstalledSetupSelection,
    pub proposal_ceiling: Ceiling,
    pub continuation: Arc<dyn InstalledOriginalTaskOwner>,
    pub backend_generation: Id,
    pub references: Option<Arc<dyn ContextReferenceOwner>>,
    pub limits: InstalledSetupLimits,
    pub work_step: WorkAdmissionStepOptions,
}

/// Observable optional-capability selection, never native execution readiness.
/// Configured means this host retained the acquisition composition selected at
/// bootstrap. Native registration, original admission and effects still use
/// their own owners and may refuse independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AcquisitionBootstrapStatus {
    NotConfigured,
    PendingBootstrap,
    Configured,
    Disabled { reason: AcquisitionDisabledReason },
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionDisabledReason {
    MissingService,
    MissingStorageSummary,
    MissingWorkGraphDeclaration,
    AmbiguousWorkGraphDeclaration,
    UnverifiedWorkGraphDeclaration,
    DegradedWorkGraph,
    NonPersistentWorkGraph,
    WorkGraphScopeMismatch,
    InvalidWorkGraphDeclaration,
}

/// The single retained state owns configuration, result and driver lifetime.
/// Status is a projection; no separate ready flag can drift from this owner.
#[derive(Default)]
pub(crate) struct AcquisitionBootstrap {
    state: BootstrapState,
}
#[derive(Default)]
enum BootstrapState {
    #[default]
    Unconfigured,
    Selected(Arc<InstalledAcquisition>),
    Deferred(Box<DeferredAcquisition>),
    Finished(FinishedAcquisition),
}
enum FinishedAcquisition {
    NotConfigured,
    Configured(Arc<InstalledAcquisition>),
    Disabled(AcquisitionDisabledReason),
    Failed,
}
struct DeferredAcquisition {
    store: Arc<SqliteStore>,
    source_scope: StoreScope,
    originals: Arc<PrivateOriginals>,
    native: CompiledNativeHost,
    defaults: InstalledSetupBootstrap,
    additional_helpers: Vec<InstalledSetupSelection>,
    tools: InstalledAcquisitionTools,
}

impl AcquisitionBootstrap {
    pub(crate) fn is_unconfigured(&self) -> bool {
        matches!(self.state, BootstrapState::Unconfigured)
    }

    pub(crate) fn selected(acquisition: Arc<InstalledAcquisition>) -> Self {
        Self {
            state: BootstrapState::Selected(acquisition),
        }
    }

    pub(crate) fn deferred_with_tools(
        store: Arc<SqliteStore>, source_scope: StoreScope, originals: Arc<PrivateOriginals>,
        native: CompiledNativeHost, defaults: InstalledSetupBootstrap, tools: InstalledAcquisitionTools,
        additional_helpers: Vec<InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        tools.require_unbound(std::time::Duration::from_millis(defaults.limits.admission_timeout_ms))?;
        // Validate trusted selections before a missing optional backend can be
        // interpreted as a normal disabled capability. These are the existing
        // configuration owners, not copied policy or current grant checks.
        super::InstalledHelpers::new(defaults.helper.clone(), additional_helpers.clone(), &native)?;
        defaults.limits.validate()?;
        super::admission::validate_proposal_ceiling(&native, &defaults.proposal_ceiling)?;
        defaults
            .work_step
            .validate()
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        source_scope
            .validate()
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        if source_scope.realm != native.realm.as_str()
            || Id::new(source_scope.realm.clone()).is_err()
            || Id::new(source_scope.owner.clone()).is_err()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(Self {
            state: BootstrapState::Deferred(Box::new(DeferredAcquisition {
                store,
                source_scope,
                originals,
                native,
                defaults,
                additional_helpers,
                tools,
            })),
        })
    }


    pub(crate) fn status(&self) -> AcquisitionBootstrapStatus {
        match &self.state {
            BootstrapState::Unconfigured
            | BootstrapState::Finished(FinishedAcquisition::NotConfigured) => {
                AcquisitionBootstrapStatus::NotConfigured
            }
            BootstrapState::Selected(_) | BootstrapState::Deferred(_) => {
                AcquisitionBootstrapStatus::PendingBootstrap
            }
            BootstrapState::Finished(FinishedAcquisition::Configured(acquisition)) => {
                // Retain the same strong driver owner after tool registration;
                // its tool's weak back-reference cannot outlive this selection.
                let _retained_owner = acquisition;
                AcquisitionBootstrapStatus::Configured
            }
            BootstrapState::Finished(FinishedAcquisition::Disabled(reason)) => {
                AcquisitionBootstrapStatus::Disabled { reason: *reason }
            }
            BootstrapState::Finished(FinishedAcquisition::Failed) => {
                AcquisitionBootstrapStatus::Failed
            }
        }
    }

    pub(crate) fn configured(&self) -> Option<Arc<InstalledAcquisition>> {
        match &self.state {
            BootstrapState::Finished(FinishedAcquisition::Configured(owner)) => Some(owner.clone()),
            _ => None,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        service: Option<WorkGraphService>,
        storage: Option<&ResolvedStorageSummary>,
    ) -> Result<Option<Arc<InstalledAcquisition>>, HostLifetimeError> {
        // A replay cannot recreate a driver or register a second tool bundle.
        // Preserve the retained completed owner when refusing that replay.
        if matches!(self.state, BootstrapState::Finished(_)) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let selected = std::mem::replace(
            &mut self.state,
            BootstrapState::Finished(FinishedAcquisition::Failed),
        );
        let acquisition = match selected {
            BootstrapState::Unconfigured => {
                self.state = BootstrapState::Finished(FinishedAcquisition::NotConfigured);
                return Ok(None);
            }
            BootstrapState::Selected(acquisition) => acquisition,
            BootstrapState::Deferred(deferred) => {
                let backend = match select_backend(
                    service,
                    storage,
                    &deferred.source_scope.realm,
                    deferred.defaults.backend_generation.clone(),
                ) {
                    Ok(backend) => backend,
                    Err(reason) => {
                        self.state =
                            BootstrapState::Finished(FinishedAcquisition::Disabled(reason));
                        return Ok(None);
                    }
                };
                let defaults = deferred.defaults;
                let acquisition = Arc::new(InstalledAcquisition::from_defaults_with_tools_and_helpers(
                    deferred.store,
                    deferred.source_scope,
                    deferred.originals,
                    &deferred.native,
                    InstalledSetupDefaults {
                        helper: defaults.helper,
                        proposal_ceiling: defaults.proposal_ceiling,
                        continuation: defaults.continuation,
                        work_backend: backend,
                        references: defaults.references,
                        limits: defaults.limits,
                        work_step: defaults.work_step,
                    },
                    deferred.tools,
                    deferred.additional_helpers,
                )?);
                acquisition.bind_tool_driver()?;
                acquisition
            }
            BootstrapState::Finished(_) => unreachable!("completed state checked above"),
        };
        self.state = BootstrapState::Finished(FinishedAcquisition::Configured(acquisition.clone()));
        Ok(Some(acquisition))
    }
}

fn select_backend(
    service: Option<WorkGraphService>,
    storage: Option<&ResolvedStorageSummary>,
    expected_realm: &str,
    backend_generation: Id,
) -> Result<SetupWorkBackend, AcquisitionDisabledReason> {
    use AcquisitionDisabledReason as Disabled;
    let service = service.ok_or(Disabled::MissingService)?;
    let storage = storage.ok_or(Disabled::MissingStorageSummary)?;
    let mut slots = storage
        .slots
        .iter()
        .filter(|slot| slot.durability.domain() == "workgraph");
    let slot = slots.next().ok_or(Disabled::MissingWorkGraphDeclaration)?;
    if slots.next().is_some() {
        return Err(Disabled::AmbiguousWorkGraphDeclaration);
    }
    if slot.degraded {
        return Err(Disabled::DegradedWorkGraph);
    }
    let declaration = slot
        .durability
        .declaration()
        .ok_or(Disabled::UnverifiedWorkGraphDeclaration)?;
    if declaration.class != DurabilityClass::Durable
        || declaration.resolution != DurabilityResolution::Persistent
    {
        return Err(Disabled::NonPersistentWorkGraph);
    }
    if service.namespace_grant().realm_id != expected_realm {
        return Err(Disabled::WorkGraphScopeMismatch);
    }
    // Backend is only the original census label; it does not establish
    // persistence. Keep the exact provider declaration and native namespace.
    SetupWorkBackend::new(
        service,
        slot.backend.clone(),
        backend_generation,
        std::slice::from_ref(declaration),
    )
    .map_err(|_| Disabled::InvalidWorkGraphDeclaration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{
        service::WorkGraphNamespaceGrant, storage_durability::DurabilityDeclaration,
    };
    use meerkat_mobkit::storage_health::{BlobDurability, StorageSlotSummary};
    use meerkat_workgraph::SqliteWorkGraphStore;

    fn service(path: &std::path::Path) -> WorkGraphService {
        WorkGraphService::with_namespace_grant(
            Arc::new(SqliteWorkGraphStore::open(path).unwrap()),
            WorkGraphNamespaceGrant::new("realm", "selected-namespace").unwrap(),
        )
        .unwrap()
    }
    fn summary(slots: Vec<StorageSlotSummary>) -> ResolvedStorageSummary {
        let mut summary = ResolvedStorageSummary::new(BlobDurability::DeclaredEphemeral, None);
        summary.slots = slots;
        summary
    }
    fn persistent() -> StorageSlotSummary {
        StorageSlotSummary {
            durability: meerkat_mobkit::storage_health::StorageSlotDurability::Declared(
                DurabilityDeclaration::durable("workgraph", DurabilityResolution::Persistent),
            ),
            backend: "selected-provider".into(),
            detail: Some("original provider note".into()),
            degraded: false,
        }
    }
    fn generation() -> Id {
        Id::new("retained-app-generation").unwrap()
    }

    #[test]
    fn backend_preserves_exact_selected_service_scope_declaration_and_generation() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("workgraph.sqlite"));
        let storage = summary(vec![persistent()]);
        let backend =
            select_backend(Some(service.clone()), Some(&storage), "realm", generation()).unwrap();
        assert!(Arc::ptr_eq(service.store(), backend.service().store()));
        assert_eq!(backend.pin().realm().as_str(), "realm");
        assert_eq!(backend.pin().namespace().as_str(), "selected-namespace");
        assert_eq!(backend.pin().backend_generation(), &generation());
        assert_eq!(backend.pin().provider(), storage.slots[0].backend);
        assert_eq!(
            backend.pin().durability(),
            storage.slots[0].durability.declaration().unwrap()
        );
    }

    #[test]
    fn incomplete_or_ambiguous_census_does_not_select_a_backend() {
        use AcquisitionDisabledReason as Disabled;
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("workgraph.sqlite"));
        let persistent = summary(vec![persistent()]);
        assert_eq!(
            select_backend(None, Some(&persistent), "realm", generation()).err(),
            Some(Disabled::MissingService)
        );
        assert_eq!(
            select_backend(Some(service.clone()), None, "realm", generation()).err(),
            Some(Disabled::MissingStorageSummary)
        );
        assert_eq!(
            select_backend(
                Some(service.clone()),
                Some(&summary(vec![])),
                "realm",
                generation()
            )
            .err(),
            Some(Disabled::MissingWorkGraphDeclaration)
        );
        let duplicate = summary(vec![self::persistent(), self::persistent()]);
        assert_eq!(
            select_backend(Some(service), Some(&duplicate), "realm", generation()).err(),
            Some(Disabled::AmbiguousWorkGraphDeclaration)
        );
    }

    #[test]
    fn provider_label_never_upgrades_unverified_ephemeral_or_degraded_storage() {
        use AcquisitionDisabledReason as Disabled;
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("workgraph.sqlite"));
        let cases = [
            (
                StorageSlotSummary::unverified("workgraph", DurabilityClass::Durable, "SQLite"),
                Disabled::UnverifiedWorkGraphDeclaration,
            ),
            (
                StorageSlotSummary::declared_ephemeral(
                    "workgraph",
                    "SQLite",
                    "explicitly ephemeral",
                ),
                Disabled::NonPersistentWorkGraph,
            ),
            (
                StorageSlotSummary::degraded("workgraph", "provider could not open"),
                Disabled::DegradedWorkGraph,
            ),
        ];
        for (slot, reason) in cases {
            assert_eq!(
                select_backend(
                    Some(service.clone()),
                    Some(&summary(vec![slot])),
                    "realm",
                    generation()
                )
                .err(),
                Some(reason)
            );
        }
        assert_eq!(
            select_backend(
                Some(service.clone()),
                Some(&summary(vec![persistent()])),
                "foreign-realm",
                generation()
            )
            .err(),
            Some(Disabled::WorkGraphScopeMismatch)
        );
        let mut invalid = persistent();
        invalid.backend = " invalid label ".into();
        assert_eq!(
            select_backend(
                Some(service),
                Some(&summary(vec![invalid])),
                "realm",
                generation()
            )
            .err(),
            Some(Disabled::InvalidWorkGraphDeclaration)
        );
    }

    #[test]
    fn bootstrap_selection_is_one_shot_even_when_not_configured() {
        let mut selection = AcquisitionBootstrap::default();
        assert_eq!(
            selection.status(),
            AcquisitionBootstrapStatus::NotConfigured
        );
        assert!(selection.prepare(None, None).unwrap().is_none());
        assert!(!selection.is_unconfigured());
        assert!(matches!(
            selection.prepare(None, None),
            Err(HostLifetimeError::InvalidSelection)
        ));
        assert_eq!(
            selection.status(),
            AcquisitionBootstrapStatus::NotConfigured
        );
    }

    #[test]
    fn disabled_selection_stays_disabled_on_replay_without_a_driver() {
        let reason = AcquisitionDisabledReason::MissingService;
        let mut selection = AcquisitionBootstrap {
            state: BootstrapState::Finished(FinishedAcquisition::Disabled(reason)),
        };
        assert!(matches!(
            selection.prepare(None, None),
            Err(HostLifetimeError::InvalidSelection)
        ));
        assert_eq!(
            selection.status(),
            AcquisitionBootstrapStatus::Disabled { reason }
        );
    }

    #[test]
    fn work_step_bootstrap_validation_uses_dispatch_owner_bounds() {
        let mut options = WorkAdmissionStepOptions {
            worker: Id::new("worker").unwrap(),
            lease_ms: 25,
            owner_timeout_ms: 10,
            settlement_budget_ms: 5,
        };
        assert!(options.validate().is_ok());
        options.lease_ms = 24;
        assert!(options.validate().is_err());
    }
}
