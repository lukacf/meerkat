//! Same-host wiring of existing acquisition ports and canonical storage.
//!
//! This composition neither opens another store/runtime nor creates an
//! authority. Native execution stays refused until the actual helper custody,
//! operation witness and original continuation producers are joined.

mod coordinator;
mod enrollment;
pub use enrollment::InstalledAcquisitionTools;
pub(crate) use enrollment::AcquisitionToolBinding;
mod task_approval;
pub use task_approval::InstalledServiceTaskSource;
mod results;
pub(crate) mod recovery;
pub use results::setup_report_definition;
mod bootstrap;
pub use bootstrap::{AcquisitionBootstrapStatus, AcquisitionDisabledReason, InstalledSetupBootstrap};
pub(crate) use bootstrap::AcquisitionBootstrap;
mod admission;
pub(crate) use admission::installed_invocation_correlation;
mod continuation;
pub use continuation::InstalledNativeContinuation;
#[cfg(test)]
pub(crate) use continuation::owner_tests::{ContextProbe, ProbeProvider};
pub use admission::{InstalledOriginalInvocation, InstalledOriginalTaskOwner, InstalledSetupDefaults, InstalledSetupLimits, InstalledSetupPreparer};
pub(crate) use coordinator::SetupWorkerObservation;

use crate::{
    FindAndInstallTool, HostToolAdmission, SetupToolHost, ToolAdmissionError,
    private_originals::{PrivateOriginalError, PrivateOriginals},
};
use async_trait::async_trait;
use meerkat_authorization_contracts::{
    grant::GrantLineageRef,
    work_association::{MAX_ASSOCIATION_REFERENCES, WorkAuthorityBasis},
};
use meerkat_core::{AgentToolDispatcher, MobMemberBinding, PrincipalRef, ToolDispatchContext};
use meerkat_toolkit::{
    continuation::{ContinuationStep, ContinuationStepOptions, SetupContinuationDelivery},
    host_lifetime::{
        control::HostLifetimeError,
        policy::{CompiledNativeHost, CompiledNativeMember},
    },
    request::ContextReferenceOwner,
    setup::{
        NativeWorkPreparation, SetupAccess, SetupError, SetupScope, SetupStore, SetupWorkBackend,
        SetupWorkPreparer, WorkAdmissionStepOptions,
    },
};
use std::{sync::Arc, time::Duration};
use toolkit_package::Id;
use toolkit_store::{SqliteStore, StoreScope, ToolkitStore};

/// Optional trusted application selection for the existing helper profile.
/// Console composition supplies defaults from its compiled member and independently
/// operator-issued grants. This is not public init or tool input. Native admission
/// resolves the grants again; configuration never declares them current.
///
/// The actual session target and controller model are deliberately absent: the
/// existing identity runtime registration and GovernedBindContext supply them.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledSetupSelection {
    /// Explicit catalog-only parent for current helper successors. The current
    /// native binder must derive a child before this supplies operation access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_capability: Option<ConfiguredHelperCatalogCapability>,
    /// Independently configured report parent. The actual helper session is
    /// selected later; this is never a permission or source-currentness claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_capability: Option<ConfiguredHelperReportCapability>,
    pub member: MobMemberBinding,
    pub ingress_actor: PrincipalRef,
    pub logical_executor: PrincipalRef,
    pub represented_subject: Option<PrincipalRef>,
    /// Independent operator-issued parent. The actual helper binder must issue
    /// or recover its child controller grant; this parent is not that child.
    pub controller_parent: Vec<GrantLineageRef>,
    /// Explicit helper operation selection, never borrowed from the caller.
    /// Native single-lineage and per-target grant selections retain their
    /// own meanings. Empty operation selection supplies no operation access.
    pub operation_basis: WorkAuthorityBasis,
}

mod report;
pub use report::ConfiguredHelperReportCapability;
mod catalog;
pub use catalog::ConfiguredHelperCatalogCapability;
mod helpers;
use helpers::InstalledHelpers;

impl InstalledSetupSelection {
    pub(crate) fn selected_member(
        &self,
        native: &CompiledNativeHost,
    ) -> Result<CompiledNativeMember, HostLifetimeError> {
        let member = native
            .members
            .iter()
            .find(|entry| entry.member == self.member)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        if native
            .members
            .iter()
            .filter(|entry| entry.member == self.member)
            .count()
            != 1
            || [&self.ingress_actor, &self.logical_executor]
                .iter()
                .any(|principal| principal.validate_qualified().is_err())
            || self
                .represented_subject
                .as_ref()
                .is_some_and(|principal| principal.validate_qualified().is_err())
            || self.controller_parent.is_empty()
            || self.controller_parent.len() >= MAX_ASSOCIATION_REFERENCES
            || self
                .controller_parent
                .iter()
                .any(|reference| reference.validate().is_err())
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if !matches!(
            self.operation_basis,
            WorkAuthorityBasis::GrantLineage { .. }
                | WorkAuthorityBasis::TargetedGrantLineages { .. }
        ) || self.operation_basis.grant_lineages().any(|lineage| {
            lineage.is_empty()
                || lineage.len() > MAX_ASSOCIATION_REFERENCES
                || lineage
                    .iter()
                    .any(|reference| reference.validate().is_err())
        }) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if let Some(report) = &self.report_capability {
            report.validate(&self.operation_basis)?;
        }
        if let Some(catalog) = &self.catalog_capability {
            catalog.validate(&self.operation_basis)?;
        }
        Ok(member.clone())
    }
}

/// Actual application owners selected by the private bootstrap factory.
/// These are existing ports, not names decoded from public configuration.
/// Supplying them is composition; it does not declare native execution ready.
pub struct InstalledAcquisitionOwners {
    /// This local adapter uses the native realm as its conditional-store scope.
    pub tenant: Id,
    pub helper: InstalledSetupSelection,
    pub admission: Arc<dyn SetupToolHost>,
    pub references: Arc<dyn ContextReferenceOwner>,
    pub preparer: Arc<dyn SetupWorkPreparer>,
    /// Actual selected WorkGraph service and its provider-open declaration.
    pub work_backend: SetupWorkBackend,
    pub continuation: Arc<dyn SetupContinuationDelivery>,
    pub admission_timeout: Duration,
    /// Bounded effect settings selected by the existing host worker owner.
    pub work_step: WorkAdmissionStepOptions,
}

pub(crate) struct InstalledAcquisition {
    helpers: InstalledHelpers,
    grant_caller: PrincipalRef,
    store: Arc<SqliteStore>,
    scope: SetupScope,
    host: Arc<CanonicalSetupHost>,
    references: Arc<dyn ContextReferenceOwner>,
    preparer: Arc<dyn SetupWorkPreparer>,
    work_backend: SetupWorkBackend,
    continuation: Arc<dyn SetupContinuationDelivery>,
    work_step: WorkAdmissionStepOptions,
    tools: InstalledAcquisitionTools,
}

impl InstalledAcquisition {
    pub(super) fn new(
        store: Arc<SqliteStore>,
        source_scope: StoreScope,
        originals: Arc<PrivateOriginals>,
        native: &CompiledNativeHost,
        owners: InstalledAcquisitionOwners,
        additional_helpers: Vec<InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        let tools = InstalledAcquisitionTools::new(owners.admission_timeout)?;
        Self::with_tools(store, source_scope, originals, native, owners, tools, additional_helpers)
    }

    pub(super) fn with_tools(
        store: Arc<SqliteStore>,
        source_scope: StoreScope,
        originals: Arc<PrivateOriginals>,
        native: &CompiledNativeHost,
        owners: InstalledAcquisitionOwners,
        tools: InstalledAcquisitionTools,
        additional_helpers: Vec<InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        tools.require_unbound(owners.admission_timeout)?;
        store.capabilities().require_durable_contract()
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        // Conditional publication can participate only in this physical
        // realm. A tenant alias must be resolved by the application compiler.
        if owners.tenant.as_str() != source_scope.realm {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let helpers = InstalledHelpers::new(owners.helper, additional_helpers, native)?;
        if owners.work_backend.pin().realm() != &owners.tenant {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let deployment =
            Id::new(source_scope.owner).map_err(|_| HostLifetimeError::InvalidSelection)?;
        let scope = SetupScope::new(owners.tenant.clone(), deployment.clone());
        let host = Arc::new(CanonicalSetupHost {
            originals,
            inner: owners.admission,
            tenant: owners.tenant,
            deployment,
            #[cfg(test)]
            recheck_pause: std::sync::Mutex::new(None),
        });
        Ok(Self {
            helpers,
            grant_caller: native.grant_administrator.clone(),
            store,
            scope,
            host,
            references: owners.references,
            preparer: owners.preparer,
            work_backend: owners.work_backend,
            continuation: owners.continuation,
            work_step: owners.work_step,
            tools,
        })
    }

    /// Historical installed helper selection for the existing native binder.
    /// It confers no input custody, controller child or operation permission.
    pub(crate) fn helper(&self) -> (&InstalledSetupSelection, &CompiledNativeMember) {
        self.helpers.get(None).expect("validated default helper")
    }

    pub(crate) fn helper_selections(&self) -> Vec<InstalledSetupSelection> {
        self.helpers.all().map(|(helper, _)| helper.clone()).collect()
    }

    fn selected_helper(&self, request: &meerkat_toolkit::request::ValidatedSetupRequest)
        -> Result<crate::original_ingress::helper_originals::HelperAdmissionSelection, SetupError>
    {
        let (helper, member) = self.helpers.get(request.host().setup_helper.as_ref())
            .map_err(|_| SetupError::Denied)?;
        Ok(crate::original_ingress::helper_originals::HelperAdmissionSelection {
            helper: helper.clone(), member: member.clone(), grant_caller: self.grant_caller.clone(),
        })
    }

    pub(super) fn bind_tool_driver(self: &Arc<Self>) -> Result<(), HostLifetimeError> {
        self.tools.bind(self)
    }

    pub(crate) fn tool_admission(&self) -> crate::AdmissionDependencies {
        crate::AdmissionDependencies {
            store: self.store.clone(), conditional_store: Some(self.store.clone()),
            scope: self.scope.clone(), host: self.host.clone(),
            references: self.references.clone(),
        }
    }

    pub(crate) fn tool(&self) -> Arc<dyn AgentToolDispatcher> {
        self.tools.acquisition_tool()
    }

    pub(crate) fn result_tool(self: &Arc<Self>) -> Arc<dyn AgentToolDispatcher> {
        self.tools.result_tool()
    }

    /// Freeze immutable input through the selected ports and the same SQLite
    /// instance that owns source rows. No helper WorkGraph admission or native
    /// runtime materialization occurs during this step.
    pub(crate) async fn prepare(
        &self,
        operation: &Id,
        call_id: &str,
        context: &ToolDispatchContext,
    ) -> Result<NativeWorkPreparation, SetupError> {
        let admission = self
            .host
            .admit_invocation(call_id, context)
            .await
            .map_err(host_admission_error)?;
        let authorization = context.work_authorization().ok_or(SetupError::Denied)?;
        let current = admission.context.borrowed(authorization);
        let store = SetupStore::new_conditional(self.store.as_ref(), self.scope.clone())?;
        let snapshot = store
            .load(
                operation,
                &admission.evidence,
                &current,
                self.references.as_ref(),
            )
            .await?;
        self.prepare_bound_work(
            &store,
            &snapshot,
            SetupAccess {
                evidence: &admission.evidence,
                context: &current,
                references: self.references.as_ref(),
            },
            context,
        )
        .await
    }

    /// The existing continuation owner checks the native receipt and current
    /// destination. This does not derive a task or receipt from a source row.
    pub(crate) async fn continue_original(
        &self,
        operation: &Id,
        options: &ContinuationStepOptions,
    ) -> Result<ContinuationStep, SetupError> {
        SetupStore::new_conditional(self.store.as_ref(), self.scope.clone())?
            .step_continuation(operation, self.continuation.as_ref(), options)
            .await
    }
}

fn host_admission_error(error: ToolAdmissionError) -> SetupError {
    match error {
        ToolAdmissionError::Denied => SetupError::Denied,
        ToolAdmissionError::Unavailable => SetupError::NativeExecutionUnavailable,
    }
}

fn source_admission_error(error: PrivateOriginalError) -> ToolAdmissionError {
    match error {
        PrivateOriginalError::Unavailable => ToolAdmissionError::Unavailable,
        PrivateOriginalError::Denied | PrivateOriginalError::Conflict => ToolAdmissionError::Denied,
    }
}

struct CanonicalSetupHost {
    originals: Arc<PrivateOriginals>,
    inner: Arc<dyn SetupToolHost>,
    tenant: Id,
    deployment: Id,
    #[cfg(test)]
    recheck_pause: std::sync::Mutex<Option<Arc<AdmissionRecheckPause>>>,
}

// A one-shot test observation of this host's actual successful inner admission.
// The real registered invocation retains its admission/context while paused;
// no source or publication lock is held across the permit await.
#[cfg(test)]
pub(crate) struct AdmissionRecheckPause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    finished: tokio::sync::Notify,
    result: std::sync::Mutex<Option<Result<(), ToolAdmissionError>>>,
}
#[cfg(test)]
impl AdmissionRecheckPause {
    pub(crate) async fn entered(&self) { self.entered.notified().await; }
    pub(crate) fn release(&self) { self.release.add_permits(1); }
    pub(crate) async fn outcome(&self) -> Result<(), ToolAdmissionError> {
        self.finished.notified().await;
        self.result.lock().unwrap().take().expect("actual admission returned")
    }
}
#[cfg(test)]
impl InstalledAcquisition {
    pub(crate) fn hold_admission_recheck_for_test(&self) -> Arc<AdmissionRecheckPause> {
        let pause = Arc::new(AdmissionRecheckPause {
            entered: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0),
            finished: tokio::sync::Notify::new(), result: std::sync::Mutex::new(None),
        });
        assert!(self.host.recheck_pause.lock().unwrap().replace(pause.clone()).is_none());
        pause
    }
}

#[async_trait]
impl SetupToolHost for CanonicalSetupHost {
    async fn admit_invocation(
        &self,
        call_id: &str,
        context: &ToolDispatchContext,
    ) -> Result<HostToolAdmission, ToolAdmissionError> {
        let original = self
            .originals
            .acquisition_original(context)
            .await
            .map_err(source_admission_error)?;
        let admission = self.inner.admit_invocation(call_id, context).await?;
        #[cfg(test)]
        let pause = self.recheck_pause.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = &pause {
            pause.entered.notify_one();
            pause.release.acquire().await.unwrap().forget();
        }
        let result = self.recheck_admission(context, &original, admission).await;
        #[cfg(test)]
        if let Some(pause) = pause {
            *pause.result.lock().unwrap() = Some(match &result {
                Ok(_) => Ok(()),
                Err(ToolAdmissionError::Denied) => Err(ToolAdmissionError::Denied),
                Err(ToolAdmissionError::Unavailable) => Err(ToolAdmissionError::Unavailable),
            });
            pause.finished.notify_one();
        }
        result
    }
}

impl CanonicalSetupHost {
    async fn recheck_admission(
        &self,
        context: &ToolDispatchContext,
        original: &crate::private_originals::acquisition::AcquisitionOriginal,
        admission: HostToolAdmission,
    ) -> Result<HostToolAdmission, ToolAdmissionError> {
        let claims = original.original.association().candidate();
        if admission.evidence.association() != original.original.association()
            || admission.context.tenant != self.tenant
            || admission.context.deployment != self.deployment
            || admission.context.realm != claims.ingress_namespace.realm
            || admission.context.target != claims.target
        {
            return Err(ToolAdmissionError::Denied);
        }
        // The asynchronous application lookup may overlap cancellation or a
        // source change. Re-open the exact native-held original and canonical
        // row; neither a self-consistent digest nor a route alias is custody.
        let current = self
            .originals
            .acquisition_original(context)
            .await
            .map_err(source_admission_error)?;
        if !Arc::ptr_eq(&current.original, &original.original)
            || !self.originals.same_acquisition_source(
                &original.reference, &original.source_record, &current.source_record,
            ).map_err(source_admission_error)?
            || current.contributors.len() != original.contributors.len()
            || current
                .contributors
                .iter()
                .zip(&original.contributors)
                .any(|(left, right)| !Arc::ptr_eq(left, right))
        {
            return Err(ToolAdmissionError::Denied);
        }
        Ok(admission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_ingress::tests::{initialized, principal};
    use meerkat_authorization_contracts::{
        constraints::ExecutionRestrictions, evidence::EvidenceId,
    };
    use meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy;

    fn selection() -> (
        tempfile::TempDir,
        CompiledNativeHost,
        InstalledSetupSelection,
    ) {
        let (directory, bootstrap) = super::super::tests::fixture();
        let policy: CompiledLocalPolicy =
            serde_json::from_slice(&std::fs::read(bootstrap.local_policy.path).unwrap()).unwrap();
        let native = policy.native_host.unwrap();
        let owner = initialized(&directory.path().join("setup-selection-grants"));
        // Real owner-issued references in a fixture. Shape selection below is
        // deliberately not a claim of their current usability by this host.
        let controller = owner
            .grants()
            .issue_root(
                &principal("admin"),
                EvidenceId::new("setup-controller").unwrap(),
                principal("setup-agent"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        let operation = owner
            .grants()
            .issue_root(
                &principal("admin"),
                EvidenceId::new("setup-operation").unwrap(),
                principal("setup-agent"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        let selected = InstalledSetupSelection {
            catalog_capability: None,
            report_capability: None,
            member: native.members[0].member.clone(),
            ingress_actor: principal("setup-owner"),
            logical_executor: principal("setup-agent"),
            represented_subject: None,
            controller_parent: vec![controller],
            operation_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![operation],
            },
        };
        (directory, native, selected)
    }

    #[test]
    fn unavailable_source_and_host_remain_unavailable_not_denied() {
        assert!(matches!(
            source_admission_error(PrivateOriginalError::Unavailable),
            ToolAdmissionError::Unavailable
        ));
        assert!(matches!(
            source_admission_error(PrivateOriginalError::Denied),
            ToolAdmissionError::Denied
        ));
        assert!(matches!(
            host_admission_error(ToolAdmissionError::Unavailable),
            SetupError::NativeExecutionUnavailable
        ));
        assert!(matches!(
            host_admission_error(ToolAdmissionError::Denied),
            SetupError::Denied
        ));
    }

    #[test]
    fn setup_helper_selection_requires_exact_compiled_member() {
        let (_directory, native, mut selected) = selection();
        assert_eq!(
            selected.selected_member(&native).unwrap(),
            native.members[0]
        );
        selected.member.role = "other-role".into();
        assert!(selected.selected_member(&native).is_err());
    }

    #[test]
    fn setup_helper_selection_preserves_targeted_base_and_empty_operation_selection() {
        let (_directory, native, mut selected) = selection();
        let WorkAuthorityBasis::GrantLineage { lineage } = selected.operation_basis.clone() else {
            panic!("fixture requires one explicit operation lineage");
        };
        selected.operation_basis = WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(lineage),
            targets: vec![],
        };
        assert!(selected.selected_member(&native).is_ok());
        selected.operation_basis = WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![],
        };
        // Shape selection accepts the native controller-only representation;
        // this gives the helper no operation access.
        assert!(selected.selected_member(&native).is_ok());
        assert_eq!(selected.operation_basis.grant_lineages().count(), 0);
        selected.operation_basis = WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(vec![]),
            targets: vec![],
        };
        assert!(selected.selected_member(&native).is_err());
    }

    #[test]
    fn setup_helper_selection_requires_separate_explicit_grant_selections() {
        let (_directory, native, selected) = selection();
        let mut missing_controller = selected.clone();
        missing_controller.controller_parent.clear();
        assert!(missing_controller.selected_member(&native).is_err());
        let mut missing_operation = selected;
        missing_operation.operation_basis = WorkAuthorityBasis::GrantLineage { lineage: vec![] };
        assert!(missing_operation.selected_member(&native).is_err());
    }
}
