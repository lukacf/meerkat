//! Operator-only setup for the application's actual native owner/store pair.
//!
//! The trusted local gateway can share its existing owner with this setup view;
//! a protected remote view takes its operator only from installed selection.
//! Standalone initialization still
//! requires exclusive ownership. Both paths require the matched native source
//! to supply meerkat::governed_realm::stock, present in the retained d34 reference
//! but absent from the older b0a compile tuple.

use super::*;
use meerkat::governed_realm::stock::StockPrincipals;
use meerkat_authorization_contracts::{evidence::EvidenceId, grant::GrantLineageRef};
use meerkat_core::{ApprovalDecision, ApprovalRecord, ApprovalRequest, ApprovalStatus};
use serde::{Deserialize, Serialize};
use toolkit_package::Id;
use toolkit_store::{
    AdmissionOutcome, AdmissionRequest, RecordData, StoreKey, StoredRecord, ToolkitStore,
    Transition,
};

pub(super) mod configured_setup;
pub use configured_setup::AgentSetupCapabilityRequest;
pub(super) mod configured_report;
pub use configured_report::HelperReportCapabilityRequest;
mod service_mcp;
mod service_batch;
pub use service_batch::{MAX_PUBLIC_SERVICE_BATCH, ServiceMcpParentBatchReview};
pub(crate) use service_batch::ReviewedServiceCatalogParent;
pub(crate) use service_batch::member_decision_id as service_batch_member_id;
pub(crate) mod service_recovery;
mod standing_service;
pub(crate) use standing_service::{ServiceParentRuleEvidence, StandingServiceParentProposal};
mod service_review;
mod setup_request;
pub(crate) mod setup_targets;
pub use setup_targets::SetupTargetRequest;
mod setup_recovery;
pub(crate) use setup_request::{validate_setup_selection, validate_retained_setup_selection};
pub use service_mcp::{ReviewedServiceMcpParent, ServiceMcpParentOutcome, ServiceMcpParentReview};
pub(crate) use service_mcp::ServiceMcpParentCompensation;

const SCHEMA: &str = "toolkit.operator-application-parent";
const RECORD_VERSION: u32 = 2;

/// Explicit fresh setup, never a fallback after an unsuccessful restore. The
/// root principal is deliberately absent: the trusted constructor selects it
/// from the native OS facade or the installed application bootstrap.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperatorOwnerStartup {
    Restore,
    Initialize {
        namespace: EvidenceId,
        generation: u64,
    },
}

impl OperatorOwnerStartup {
    pub fn fresh(namespace: &str, generation: u64) -> Result<Self, HostLifetimeError> {
        if generation == 0 {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(Self::Initialize {
            namespace: EvidenceId::new(namespace)
                .map_err(|_| HostLifetimeError::InvalidSelection)?,
            generation,
        })
    }
}

pub struct OperatorSetup {
    lifetime: Arc<NativeHostLifetime>,
    native: CompiledNativeHost,
    operator: meerkat_core::PrincipalRef,
    originals: Arc<dyn ToolkitStore>,
    source: Arc<ToolkitOriginalOwner>,
    helper_selection: Option<crate::installed_host::acquisition::InstalledSetupSelection>,
    additional_helpers: Vec<crate::installed_host::acquisition::InstalledSetupSelection>,
}

/// Held by the actual operator setup invocation across its existing presenter.
/// This is not deserializable, and the UI body is never decoded as authority.
#[derive(Clone)]
pub struct OperatorParentProposal {
    route: Id,
    request: ApprovalRequest,
    restrictions: ExecutionRestrictions,
    purpose: OperatorParentPurpose,
    setup_request: Option<SetupRequestIntent>,
    batch_member: Option<service_batch::ServiceBatchMemberIdentity>,
    creating_source: std::sync::Weak<ToolkitOriginalOwner>,
    creating_lifetime: std::sync::Weak<NativeHostLifetime>,
}
impl OperatorParentProposal {
    pub fn request(&self) -> &ApprovalRequest {
        &self.request
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorDecision {
    route: Id,
    purpose: OperatorParentPurpose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    setup_request: Option<SetupRequestIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    batch_member: Option<service_batch::ServiceBatchMemberIdentity>,
    operator: meerkat_core::PrincipalRef,
    application: meerkat_core::PrincipalRef,
    represented_subject: Option<meerkat_core::PrincipalRef>,
    approval: ApprovalRecord,
    requested: ExecutionRestrictions,
    issued: ParentIssuance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    target_issuance: Vec<setup_targets::TargetIssuance>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ParentIssuance {
    Pending,
    Denied,
    NativeRefused,
    Issued { lineage: Vec<GrantLineageRef> },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupRequestIntent {
    session_id: meerkat_core::SessionId,
    // The legacy tuple-level review cannot select a fresh source original.
    original: Option<crate::private_originals::PrivateOriginalRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<crate::original_ingress::helper_originals::HelperSetupOrigin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    targets: Vec<setup_targets::SetupTargetIntent>,
}

/// The reviewed purpose distinguishes controller, service and setup access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorParentPurpose {
    Operations,
    Controller,
    SetupRequest,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperatorParentOutcome {
    /// The existing source owner retained this explicit setup-only selection
    /// on its still-unbound original. No native input or service effect entered.
    SetupRequestSelected { selection: crate::private_originals::PrivateSetupRequestSelection },
    Denied,
    Unavailable,
    PendingReconciliation,
    /// Historical native selection for the trusted application's compiler.
    /// The purpose chooses exactly one field on this route; it is not a union.
    /// Publish a new compiled policy and close/reopen through the existing host
    /// lifetime. This does not mutate the currently selected policy. If compiler
    /// publication fails, retry this exact result, never reissue the parent.
    Selected {
        route: Id,
        purpose: OperatorParentPurpose,
        lineage: Vec<GrantLineageRef>,
    },
}

fn open_owner_pair(
    selection: &HostBootstrapSelection,
    native: &CompiledNativeHost,
    operator: &meerkat_core::PrincipalRef,
    startup: OperatorOwnerStartup,
) -> Result<(Arc<ToolkitOriginalOwner>, SqliteStore), HostLifetimeError> {
    let create = matches!(startup, OperatorOwnerStartup::Initialize { .. });
    let directory = state_directory(
        selection,
        native,
        if create {
            NativeOwnerStartup::Initialize
        } else {
            NativeOwnerStartup::Restore
        },
    )?;
    open_owner_pair_in(&directory, &native.ingress_domain, Some(operator), startup, false)
}

/// The stopped application's preparation uses the same canonical owner pair.
/// The directory was resolved by state_directory_in and is not a new store.
pub(super) fn open_owner_pair_in(
    directory: &Path,
    ingress_domain: &ResourceDomain,
    operator: Option<&meerkat_core::PrincipalRef>,
    startup: OperatorOwnerStartup,
    installed_publication: bool,
) -> Result<(Arc<ToolkitOriginalOwner>, SqliteStore), HostLifetimeError> {
    let create = matches!(startup, OperatorOwnerStartup::Initialize { .. });
    let grants = directory.join("grants.jsonl");
    let original_path = directory.join("originals.sqlite3");
    // A partial pair is not an invitation to overwrite, reinitialize or
    // infer that lost historical original evidence was an empty store.
    match (grants.try_exists(), original_path.try_exists()) {
        (Ok(false), Ok(false)) if create => {}
        (Ok(true), Ok(true)) if !create => {}
        _ => return Err(HostLifetimeError::Unavailable),
    }
    if grants.is_symlink() || original_path.is_symlink() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let store = Arc::new(FileGrantOwnerStore::new(grants));
    let source = match startup {
        OperatorOwnerStartup::Initialize {
            namespace,
            generation,
        } => {
            if generation == 0 {
                return Err(HostLifetimeError::InvalidSelection);
            }
            ToolkitOriginalOwner::initialize(
                LocalGrantConfiguration {
                    root: operator.ok_or(HostLifetimeError::InvalidSelection)?.clone(),
                    namespace,
                    generation,
                },
                store,
                Arc::new(HostAuthorizationClock),
                ingress_domain.clone(),
            )
        }
        OperatorOwnerStartup::Restore => ToolkitOriginalOwner::restore(
            store,
            Arc::new(HostAuthorizationClock),
            ingress_domain.clone(),
        ),
    }
    .map_err(|_| HostLifetimeError::Unavailable)?;
    // The actual native grant lease stays held while the source-store
    // pair is opened, and until this entire setup owner is dropped.
    let source = Arc::new(source);
    let originals = match (create, installed_publication) {
        (true, false) => SqliteStore::open(&original_path),
        (false, false) => SqliteStore::open_existing(&original_path),
        (true, true) => SqliteStore::open_published(&original_path, source.clone()),
        (false, true) => SqliteStore::open_existing_published(&original_path, source.clone()),
    }
    .map_err(|_| HostLifetimeError::Unavailable)?;
    Ok((source, originals))
}

impl OperatorSetup {
    /// First-use only, called by the trusted installer's stopped-host transaction.
    /// Candidate selection need not be current. Both canonical stores must be
    /// absent; an occupied, partial or uncertain pair is never repaired/replaced.
    /// The native owner lease covers both stores and is released before success.
    /// No runtime lifetime, grant, session or activation publication is created.
    pub fn provision_application_candidate(
        selection: &HostBootstrapSelection,
        namespace: &str,
        generation: u64,
    ) -> Result<(), HostLifetimeError> {
        let startup = OperatorOwnerStartup::fresh(namespace, generation)?;
        NativeHostLifetime::with_candidate_installation(selection, |policy| {
            let native = policy
                .native_host
                .as_ref()
                .ok_or(HostLifetimeError::Unavailable)?;
            let operator = native
                .remote_operator
                .as_ref()
                .ok_or(HostLifetimeError::Unavailable)?;
            let _validated = validate_installed_artifacts(native, |reference| {
                read_bundle_artifact(selection, reference)
            })?;
            let (source, originals) = open_owner_pair(selection, native, operator, startup)?;
            // No asynchronous store operation has been issued. Close the source
            // store first, then release the actual native grant-store lease.
            drop(originals);
            drop(source);
            Ok(())
        })
    }

    pub fn open(
        selection: HostBootstrapSelection,
        startup: OperatorOwnerStartup,
    ) -> Result<Self, HostLifetimeError> {
        Self::open_selected(selection, startup, false)
    }

    /// Trusted first-use installer entry, never ordinary application runtime
    /// bootstrap or a review command. The installed selection owns this root;
    /// Initialize refuses any existing or partial canonical owner/store pair.
    pub fn open_for_application_operator(
        selection: HostBootstrapSelection,
        startup: OperatorOwnerStartup,
    ) -> Result<Self, HostLifetimeError> {
        Self::open_selected(selection, startup, true)
    }

    fn open_selected(
        selection: HostBootstrapSelection,
        startup: OperatorOwnerStartup,
        application: bool,
    ) -> Result<Self, HostLifetimeError> {
        let mut lifetime = NativeHostLifetime::bootstrap(selection.clone())?;
        let native = lifetime
            .selected_policy()
            .native_host
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        let operator = if application {
            native
                .remote_operator
                .clone()
                .ok_or(HostLifetimeError::Unavailable)?
        } else {
            StockPrincipals::for_local_operator(&native.realm)
                .map_err(|_| HostLifetimeError::Unavailable)?
                .operator
        };
        let (source, originals) = lifetime.with_current_setup(|| {
            // An exact digest is not executable validity. Use the same actual
            // native artifact validators as ordinary startup before touching
            // either store or creating the native owner directory.
            let _validated = validate_installed_artifacts(&native, |reference| {
                read_bundle_artifact(&selection, reference)
            })?;
            open_owner_pair(&selection, &native, &operator, startup)
        })?;
        lifetime.bind_native_authorization_publication(source.publication())?;
        Ok(Self {
            lifetime: Arc::new(lifetime),
            native,
            operator,
            originals: Arc::new(originals),
            source,
            helper_selection: None,
            additional_helpers: vec![],
        })
    }

    /// Trusted local Rust composition only. The gateway supplies its exact
    /// existing source/store/lifetime together; this never opens another store
    /// or accepts an application-supplied operator principal. The protected
    /// local presenter still has to return the retained exact approval.
    pub(crate) fn from_local_gateway(
        lifetime: Arc<NativeHostLifetime>,
        originals: Arc<dyn ToolkitStore>,
        source: Arc<ToolkitOriginalOwner>,
    ) -> Result<Self, HostLifetimeError> {
        lifetime.validate_current_selection()?;
        lifetime.validate_native_entry(None)?;
        let native = lifetime
            .selected_policy()
            .native_host
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        if source.ingress_domain() != &native.ingress_domain {
            return Err(HostLifetimeError::WrongOwner);
        }
        let operator = StockPrincipals::for_local_operator(&native.realm)
            .map_err(|_| HostLifetimeError::Unavailable)?
            .operator;
        Ok(Self {
            lifetime,
            native,
            operator,
            originals,
            source,
            helper_selection: None,
            additional_helpers: vec![],
        })
    }

    /// Same-owner application view. The private parent authenticates and
    /// claims the protected answer; its JSON cannot select this native caller.
    pub(crate) fn from_application_gateway(
        lifetime: Arc<NativeHostLifetime>,
        originals: Arc<dyn ToolkitStore>,
        source: Arc<ToolkitOriginalOwner>,
    ) -> Result<Self, HostLifetimeError> {
        lifetime.validate_current_selection()?;
        lifetime.validate_native_entry(None)?;
        let native = lifetime
            .selected_policy()
            .native_host
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        if source.ingress_domain() != &native.ingress_domain {
            return Err(HostLifetimeError::WrongOwner);
        }
        let operator = native
            .remote_operator
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        Ok(Self {
            lifetime,
            native,
            operator,
            originals,
            source,
            helper_selection: None,
            additional_helpers: vec![],
        })
    }

    pub(crate) fn with_helper_selection(mut self,
        helper: Option<crate::installed_host::acquisition::InstalledSetupSelection>) -> Result<Self, HostLifetimeError>
    {
        self.lifetime.validate_current_selection()?;
        if let Some(helper) = &helper { helper.selected_member(&self.native)?; }
        self.helper_selection = helper;
        Ok(self)
    }

    pub(crate) fn with_helper_selections(mut self,
        helpers: Vec<crate::installed_host::acquisition::InstalledSetupSelection>) -> Result<Self, HostLifetimeError>
    {
        self.lifetime.validate_current_selection()?;
        if helpers.len() > 8 { return Err(HostLifetimeError::InvalidSelection); }
        let mut selected = Vec::new();
        for helper in helpers {
            helper.selected_member(&self.native)?;
            if selected.iter().any(|old: &crate::installed_host::acquisition::InstalledSetupSelection| old.member == helper.member) {
                return Err(HostLifetimeError::InvalidSelection);
            }
            selected.push(helper);
        }
        self.helper_selection = if selected.is_empty() { None } else { Some(selected.remove(0)) };
        self.additional_helpers = selected;
        Ok(self)
    }

    /// Close and join the exclusive installer before starting the ordinary
    /// restore-only gateway. This neither issues a grant nor changes selection.
    pub async fn finish(self) -> Result<(), HostLifetimeError> {
        use meerkat_toolkit::host_lifetime::control::{
            HostCloseReason, HostCloseState, PrivateHostLifetimeControl,
        };
        let close = self
            .lifetime
            .begin_close_activation(HostCloseReason::Shutdown)?;
        let settled = self.lifetime.join_close(&close.handle).await?;
        if settled.state != HostCloseState::Settled {
            return Err(HostLifetimeError::NotSettled);
        }
        Ok(())
    }

    /// Avoid a new prompt when the actual selected controller parent already
    /// covers this exact native route. Serialized selection alone is never
    /// sufficient: resolve the full current native lineage and its bounds.
    pub fn selected_controller_parent(
        &self,
        route: &Id,
        facts: &meerkat_core::ControllerModelFacts,
    ) -> Result<Option<OperatorParentOutcome>, HostLifetimeError> {
        use meerkat_authorization::clock::LocalAuthorizationClock;
        use meerkat_authorization_contracts::constraints::OperationRestrictionValues;
        self.lifetime.validate_current_selection()?;
        let selected = self.route(route)?;
        let Some(parent) = selected.controller_parent.as_ref() else {
            return Ok(None);
        };
        let (binding, _, _) = selected.source_binding(self.lifetime.selected_policy())?;
        let member = self
            .native
            .members
            .iter()
            .find(|member| &member.member == binding)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        let resolved = match self.source.grants().resolve_controller_lineage(
            parent,
            &self.native.grant_administrator,
            selected.represented_subject.as_ref(),
        ) {
            Ok(resolved) => resolved,
            Err(meerkat_authorization::grants::GrantRefusal::Denied) => return Ok(None),
            Err(_) => return Err(HostLifetimeError::Unavailable),
        };
        let values = application_operations::controller(
            &self.native.grant_administrator,
            &selected.logical_executor,
            &member.audience,
            &facts.route_key(),
            application_operations::ControllerAction::Inference,
        );
        let child = match resolved.restrictions().for_child(&ExecutionRestrictions {
            delegation_depth:
                meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0),
            ..ExecutionRestrictions::unrestricted()
        }) {
            Ok(child) => child,
            Err(_) => return Ok(None),
        };
        if child
            .check_bounds(OperationRestrictionValues {
                action: &values.action,
                resource_domain: &values.resource_domain,
                processor: &values.processor,
                audience: &values.audience,
                now_ms: HostAuthorizationClock
                    .now()
                    .map_err(|_| HostLifetimeError::Unavailable)?
                    .unix_ms,
            })
            .is_err()
        {
            return Ok(None);
        }
        resolved
            .check_current()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        self.lifetime.validate_current_selection()?;
        Ok(Some(OperatorParentOutcome::Selected {
            route: route.clone(),
            purpose: OperatorParentPurpose::Controller,
            lineage: parent.clone(),
        }))
    }

    /// Build an exact controller parent review from the actual selected
    /// client's request-free facts. This uses the invocation owner's vocabulary;
    /// it cannot turn a selected credential or model label into permission.
    /// The ordinary approval/issuance path below still owns the decision.
    pub fn prepare_controller_parent_review(
        &self,
        route: Id,
        facts: &meerkat_core::ControllerModelFacts,
        request: ApprovalRequest,
    ) -> Result<OperatorParentProposal, HostLifetimeError> {
        self.lifetime.validate_current_selection()?;
        let selected = self.route(&route)?;
        let (binding, _, _) = selected.source_binding(self.lifetime.selected_policy())?;
        let member = self
            .native
            .members
            .iter()
            .find(|member| &member.member == binding)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        if facts.endpoint().is_empty() || facts.wire_model().is_empty() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let values = application_operations::controller(
            &self.native.grant_administrator,
            &selected.logical_executor,
            &member.audience,
            &facts.route_key(),
            application_operations::ControllerAction::Inference,
        );
        let mut proposal = self.prepare_parent_review_for(
            route,
            application_operations::parent_restrictions(values),
            request,
            OperatorParentPurpose::Controller,
        )?;
        // Display the actual request-free native route, not only its opaque
        // resource key. These are nonsecret review facts, never a new resolver.
        proposal
            .request
            .proposed_action
            .body
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
            .ok_or(HostLifetimeError::InvalidSelection)?
            .insert(
                "controller".into(),
                serde_json::json!({
                    "selection": facts.selection(), "endpoint": facts.endpoint(),
                    "wire_model": facts.wire_model(), "route_key": facts.route_key().as_str(),
                }),
            );
        Ok(proposal)
    }

    /// Review access to the registered setup-request operation for one actual
    /// native agent session selected by trusted application composition. The
    /// same tuple is used at invocation; this proposal does not enable provider
    /// operations or bypass helper admission and later protected installation.
    pub fn prepare_setup_parent_review(
        &self,
        route: Id,
        session: &meerkat_core::SessionId,
        request: ApprovalRequest,
    ) -> Result<OperatorParentProposal, HostLifetimeError> {
        self.lifetime.validate_current_selection()?;
        let selected = self.route(&route)?;
        let (binding, _, _) = selected.source_binding(self.lifetime.selected_policy())?;
        let member = self
            .native
            .members
            .iter()
            .find(|member| &member.member == binding)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        let values = application_operations::find_and_install(
            &self.native.grant_administrator,
            &selected.logical_executor,
            &member.audience,
            session,
        );
        let mut proposal = self.prepare_parent_review_for(
            route,
            application_operations::parent_restrictions(values),
            request,
            OperatorParentPurpose::SetupRequest,
        )?;
        proposal.setup_request = Some(SetupRequestIntent { session_id: session.clone(), original: None, origin: None, targets: vec![] });
        proposal
            .request
            .proposed_action
            .body
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
            .ok_or(HostLifetimeError::InvalidSelection)?
            .insert(
                "setup_request".into(),
                serde_json::json!({
                    "bundle": crate::FIND_AND_INSTALL_BUNDLE,
                    "operation": crate::FIND_AND_INSTALL,
                    "session": session.to_string(),
                }),
            );
        Ok(proposal)
    }

    /// The existing invocation supplies its native approval envelope/session.
    /// This owner fills the exact reviewed access body from retained typed facts.
    /// The caller passes request() to the existing review_access presenter.
    pub fn prepare_parent_review(
        &self,
        route: Id,
        restrictions: ExecutionRestrictions,
        request: ApprovalRequest,
    ) -> Result<OperatorParentProposal, HostLifetimeError> {
        self.prepare_parent_review_for(
            route,
            restrictions,
            request,
            OperatorParentPurpose::Operations,
        )
    }

    fn prepare_parent_review_for(
        &self,
        route: Id,
        restrictions: ExecutionRestrictions,
        mut request: ApprovalRequest,
        purpose: OperatorParentPurpose,
    ) -> Result<OperatorParentProposal, HostLifetimeError> {
        self.lifetime.validate_current_selection()?;
        let selected = self.route(&route)?;
        // The parent must support a direct depth0 application-to-agent child.
        restrictions
            .for_child(&ExecutionRestrictions {
                delegation_depth:
                    meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0),
                ..ExecutionRestrictions::unrestricted()
            })
            .map_err(|_| HostLifetimeError::Denied)?;
        request.proposed_action.summary = match purpose {
            OperatorParentPurpose::Controller => "Allow this application to use the selected model",
            OperatorParentPurpose::SetupRequest => "Allow this original task to request connector setup",
            OperatorParentPurpose::Operations => {
                "Allow this application to delegate the reviewed access"
            }
        }
        .into();
        request.proposed_action.body = Some(serde_json::json!({
            "operator": self.operator,
            "application": self.native.grant_administrator,
            "realm": self.native.realm,
            "route": route,
            "purpose": purpose,
            "represented_subject": selected.represented_subject,
            "restrictions": restrictions,
        }));
        Ok(OperatorParentProposal {
            route,
            request,
            restrictions,
            purpose,
            setup_request: None,
            batch_member: None,
            creating_source: Arc::downgrade(&self.source),
            creating_lifetime: Arc::downgrade(&self.lifetime),
        })
    }

    fn check_parent_proposal_owner(&self, proposal: &OperatorParentProposal) -> Result<(), HostLifetimeError> {
        let source = proposal.creating_source.upgrade().ok_or(HostLifetimeError::Unavailable)?;
        let lifetime = proposal.creating_lifetime.upgrade().ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&source, &self.source) || !Arc::ptr_eq(&lifetime, &self.lifetime) {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.lifetime.validate_current_selection()
    }

    /// Apply only the actual record returned by that retained operator review.
    /// The record is durable evidence, not a second authority or an account
    /// permission inferred from software installation. Native issue_root checks
    /// the actual OS-authenticated operator against this exact restored owner.
    pub async fn apply_parent_decision(
        &self,
        proposal: OperatorParentProposal,
        approval: ApprovalRecord,
    ) -> Result<OperatorParentOutcome, HostLifetimeError> {
        self.apply_parent_decision_current(proposal, approval, Arc::new(|| true)).await
    }

    fn parent_decision_record(&self, proposal: &OperatorParentProposal, approval: ApprovalRecord)
        -> Result<OperatorDecision, HostLifetimeError>
    {
        // An actual approval is applied only by the source and lifetime that
        // created its opaque proposal. Re-prepare after a legitimate restore.
        self.check_parent_proposal_owner(&proposal)?;
        let route = self.route(&proposal.route)?;
        if !same_request(&proposal.request, &approval) {
            return Err(HostLifetimeError::Denied);
        }
        let approved = match (&approval.decision, approval.status) {
            (Some(decision), ApprovalStatus::Approved)
                if decision.decision == ApprovalDecision::Approve =>
            {
                true
            }
            (Some(decision), ApprovalStatus::Denied)
                if decision.decision == ApprovalDecision::Deny =>
            {
                false
            }
            _ => return Err(HostLifetimeError::Denied),
        };
        let record = OperatorDecision {
            route: proposal.route.clone(),
            purpose: proposal.purpose,
            target_issuance: proposal.setup_request.as_ref().map(|intent|
                vec![setup_targets::TargetIssuance::NotStarted; intent.targets.len()]).unwrap_or_default(),
            setup_request: proposal.setup_request.clone(),
            batch_member: proposal.batch_member.clone(),
            operator: self.operator.clone(),
            application: self.native.grant_administrator.clone(),
            represented_subject: route.represented_subject.clone(),
            approval,
            requested: proposal.restrictions.clone(),
            issued: if approved {
                ParentIssuance::Pending
            } else {
                ParentIssuance::Denied
            },
        };
        service_batch::validate_member(&record)?;
        Ok(record)
    }

    async fn apply_parent_decision_current(
        &self, proposal: OperatorParentProposal, approval: ApprovalRecord,
        presented: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<OperatorParentOutcome, HostLifetimeError> {
        let record = self.parent_decision_record(&proposal, approval)?;
        let approved = matches!(&record.issued, ParentIssuance::Pending);
        let key = self.parent_decision_key(&record)?;
        // Store admission compares the whole immutable decision. An exact retry
        // reads the saved result even if the review deadline has since elapsed.
        if let Some(saved) = self
            .originals
            .get(&key)
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?
        {
            let existing = decode(&saved, &key)?;
            if decision_body(&existing)? != decision_body(&record)? {
                return Err(HostLifetimeError::Denied);
            }
            if approved && record.purpose == OperatorParentPurpose::SetupRequest
                && record.setup_request.as_ref().is_some_and(|intent| intent.original.is_some()) {
                return self.observe_setup_decision(saved, existing).await;
            }
            return self.observe(&existing);
        }
        let now = std::time::SystemTime::now();
        if record
            .approval
            .expires_at
            .is_some_and(|at| std::time::SystemTime::from(at) <= now)
        {
            return Err(HostLifetimeError::Denied);
        }
        self.lifetime.validate_current_selection()?;
        let admitted = self
            .originals
            .admit(AdmissionRequest {
                key: key.clone(),
                admission_key: key.id.clone(),
                data: encode(&record)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let saved = admitted.record();
        let mut record = decode(saved, &key)?;
        if !matches!(admitted, AdmissionOutcome::Created(_)) || !approved {
            return self.observe(&record);
        }
        let id =
            EvidenceId::new(key.id.clone()).map_err(|_| HostLifetimeError::InvalidSelection)?;
        let issued = self.lifetime.with_current_setup(|| {
            if !presented() { return Ok(Err(meerkat_authorization::grants::GrantRefusal::Denied)); }
            Ok(self.source.grants().issue_root(
                &self.operator,
                id,
                record.application.clone(),
                record.represented_subject.clone(),
                record.requested.clone(),
            ))
        })?;
        record.issued = match issued {
            Ok(parent) => ParentIssuance::Issued {
                lineage: vec![parent],
            },
            Err(meerkat_authorization::grants::GrantRefusal::Denied) => {
                ParentIssuance::NativeRefused
            }
            Err(_) => return Ok(OperatorParentOutcome::PendingReconciliation),
        };
        let saved = match self
            .originals
            .compare_and_swap(Transition {
                key,
                expected_revision: saved.revision,
                expected_digest: Some(saved.content_digest.clone()),
                claim: None,
                data: encode(&record)?,
                events: vec![],
                effects: vec![],
            })
            .await
        {
            Ok(saved) => saved,
            Err(_) => return Ok(OperatorParentOutcome::PendingReconciliation),
        };
        self.complete_setup_targets(saved, record, presented).await
    }

    fn route(
        &self,
        id: &Id,
    ) -> Result<&meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute, HostLifetimeError>
    {
        self.native
            .routes
            .iter()
            .find(|route| &route.id == id)
            .ok_or(HostLifetimeError::Denied)
    }
    fn observe(
        &self,
        record: &OperatorDecision,
    ) -> Result<OperatorParentOutcome, HostLifetimeError> {
        if matches!(record.issued, ParentIssuance::Issued { .. }) {
            if let Some(outcome) = self.observe_target_progress(record)? { return Ok(outcome); }
        }
        match &record.issued {
            ParentIssuance::Pending => Ok(OperatorParentOutcome::PendingReconciliation),
            ParentIssuance::Denied => Ok(OperatorParentOutcome::Denied),
            ParentIssuance::NativeRefused => Ok(OperatorParentOutcome::Unavailable),
            ParentIssuance::Issued { lineage } => {
                // A still-current grant cannot revive a closed or replaced
                // application selection through historical decision replay.
                self.lifetime.validate_current_selection()?;
                let resolved = self.source.grants().resolve_lineage(
                    lineage,
                    &record.application,
                    record.represented_subject.as_ref(),
                );
                match resolved {
                    Ok(resolved)
                        if resolved.check_current().is_ok()
                            && resolved.restrictions() == &record.requested =>
                    {
                        Ok(OperatorParentOutcome::Selected {
                            route: record.route.clone(),
                            purpose: record.purpose,
                            lineage: lineage.clone(),
                        })
                    }
                    _ => Ok(OperatorParentOutcome::Unavailable),
                }
            }
        }
    }
}

fn same_request(a: &ApprovalRequest, b: &ApprovalRecord) -> bool {
    a.requester == b.requester
        && a.owner == b.owner
        && a.resource == b.resource
        && a.proposed_action == b.proposed_action
        && a.risk == b.risk
        && a.request_body == b.request_body
        && a.allowed_decisions == b.allowed_decisions
        && a.expires_at == b.expires_at
        && a.metadata == b.metadata
        && a.request_provenance == b.request_provenance
}
fn decision_body(record: &OperatorDecision) -> Result<Vec<u8>, HostLifetimeError> {
    if let Some(member) = &record.batch_member {
        return canonical_json(&(
            "toolkit.public-service-batch-parent.v1", member, &record.route,
            record.purpose, &record.setup_request, &record.operator,
            &record.application, &record.represented_subject, &record.approval,
            &record.requested,
        )).map_err(|_| HostLifetimeError::InvalidSelection);
    }
    // The historical singleton comparison bytes remain exactly unchanged.
    canonical_json(&(
        &record.route,
        record.purpose,
        &record.setup_request,
        &record.operator,
        &record.application,
        &record.represented_subject,
        &record.approval,
        &record.requested,
    ))
    .map_err(|_| HostLifetimeError::InvalidSelection)
}
fn encode(record: &OperatorDecision) -> Result<RecordData, HostLifetimeError> {
    Ok(RecordData {
        schema: SCHEMA.into(),
        version: RECORD_VERSION,
        body: serde_json::to_value(record).map_err(|_| HostLifetimeError::InvalidSelection)?,
    })
}
fn decode(stored: &StoredRecord, key: &StoreKey) -> Result<OperatorDecision, HostLifetimeError> {
    if &stored.key != key
        || stored.data.schema != SCHEMA
        || stored.data.version != RECORD_VERSION
        || stored.data.digest().ok().as_deref() != Some(stored.content_digest.as_str())
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let record: OperatorDecision = serde_json::from_value(stored.data.body.clone())
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    if canonical_json(&record).map_err(|_| HostLifetimeError::InvalidSelection)?
        != canonical_json(&stored.data.body).map_err(|_| HostLifetimeError::InvalidSelection)?
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    if let ParentIssuance::Issued { lineage } = &record.issued {
        if lineage.len() != 1 || lineage[0].grant_id.as_str() != key.id {
            return Err(HostLifetimeError::InvalidSelection);
        }
    }
    setup_targets::validate_progress(&record, key)?;
    service_batch::validate_member(&record)?;
    Ok(record)
}

/// A real open host phase for isolated grant/source component controls. The
/// production path obtains this weak lifetime only from ReviewedServiceMcpParent.
#[cfg(test)]
pub(crate) fn test_service_lifetime() -> (tempfile::TempDir, Arc<NativeHostLifetime>) {
    let (directory, selection) = super::tests::fixture();
    (directory, Arc::new(NativeHostLifetime::bootstrap(selection).unwrap()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    mod owner_tests;
    mod target_tests;
    mod report_parent_tests;
    use super::*;
    use meerkat_authorization_contracts::constraints::{
        ActionRef, DelegationDepth, ExactRestriction,
    };
    use meerkat_core::{
        ApprovalActionKind, ApprovalOwnerRef, ApprovalPrincipalId, ApprovalProposedAction,
        ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk,
        ApprovalService, SessionId,
    };

    use meerkat_toolkit::host_lifetime::control::{
        HostCloseReason, PrivateHostLifetimeControl,
    };

    fn startup() -> OperatorOwnerStartup {
        OperatorOwnerStartup::Initialize {
            namespace: EvidenceId::new("operator-owner-test").unwrap(),
            generation: 1,
        }
    }
    pub(super) fn request() -> ApprovalRequest {
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("setup-invocation").unwrap(),
            owner: ApprovalOwnerRef::Session {
                session_id: SessionId::new(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: ApprovalResourceId::new("selected-application"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "host fills actual access".into(),
                body: None,
            },
            risk: ApprovalRisk::High,
            request_body: None,
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect(),
            expires_at: None,
            metadata: Default::default(),
            request_provenance: None,
        }
    }
    fn bounds() -> ExecutionRestrictions {
        ExecutionRestrictions {
            actions: ExactRestriction::exact([ActionRef {
                feature: "mcp".into(),
                action: "read".into(),
            }]),
            delegation_depth: DelegationDepth::remaining(1),
            ..ExecutionRestrictions::unrestricted()
        }
    }
    fn reviewed(proposal: &OperatorParentProposal, choice: ApprovalDecision) -> ApprovalRecord {
        // Actual native approval service and records. This fixture does not
        // impersonate the CLI's protected presenter or claim its UX was run.
        let approvals = ApprovalService::new();
        let pending = approvals.request(proposal.request().clone()).unwrap();
        approvals
            .decide(
                &pending.approval_id,
                choice,
                ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(),
                None,
                None,
            )
            .unwrap()
    }

    fn controller_facts() -> meerkat_core::ControllerModelFacts {
        use meerkat_core::connection::{
            AuthCredentialIdentity, CredentialAccountId, CredentialAccountRef,
        };
        use meerkat_core::{ControllerModelSelection, Provider, SessionLlmIdentity};
        meerkat_core::ControllerModelFacts::new(
            ControllerModelSelection::new(
                SessionLlmIdentity {
                    model: "reviewed-model".into(),
                    provider: Provider::OpenAI,
                    self_hosted_server_id: None,
                    auth_binding: None,
                    provider_params: None,
                },
                AuthCredentialIdentity::Account(CredentialAccountRef {
                    realm: meerkat_core::mob_realm_id("factory").unwrap(),
                    account: CredentialAccountId::parse("reviewed-account").unwrap(),
                }),
                "reviewed-profile".into(),
                "reviewed-backend".into(),
            ),
            "https://reviewed.invalid/v1/responses".into(),
            "wire-model".into(),
        )
    }

    // This source-owner gate fixture never asks a model or simulates native
    // pin preparation. A controller request is an immediate test failure.
    struct UnusedController;
    #[async_trait::async_trait]
    impl meerkat_core::AgentLlmClient for UnusedController {
        async fn stream_response(
            &self,
            _: &[meerkat_core::Message],
            _: &[Arc<meerkat_core::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            panic!("the source-owner gate must not make a controller request")
        }
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }
        fn model(&self) -> &str {
            "unused-owner-gate-fixture"
        }
    }

    #[tokio::test]
    async fn prepared_controller_rejects_another_gateway_before_operator_review() {
        use crate::private_host::PrivateHostClosed;
        use crate::private_originals::{LocalControllerSetup, PrivateOriginalRef};
        let (_a_dir, a_selection) = super::super::tests::fixture();
        let (_b_dir, b_selection) = super::super::tests::fixture();
        for selection in [&a_selection, &b_selection] {
            drop(OperatorSetup::open(selection.clone(), startup()).unwrap());
        }
        let (a_lifetime, a_installed) =
            InstalledToolkitHost::open(a_selection.clone(), NativeOwnerStartup::Restore)
                .await
                .unwrap();
        let (b_lifetime, b_installed) =
            InstalledToolkitHost::open(b_selection.clone(), NativeOwnerStartup::Restore)
                .await
                .unwrap();
        // Both fixtures intentionally have identical serialized route,
        // deployment, principal and native namespace values.
        let a_source = a_installed.originals();
        let (a_closed, a_receiver) = tokio::sync::watch::channel(false);
        let (_b_closed, b_receiver) = tokio::sync::watch::channel(false);
        let a_host = a_installed.compose(Arc::new(a_lifetime), PrivateHostClosed(a_receiver));
        let b_host = b_installed.compose(Arc::new(b_lifetime), PrivateHostClosed(b_receiver));
        let native_session = SessionId::new();
        let original = PrivateOriginalRef {
            route: Id::new("console").unwrap(),
            original: EvidenceId::new("owner-gate").unwrap(),
            event_sha256: "a".repeat(64),
        };
        let prepared = LocalControllerSetup::owner_test_fixture(
            &a_source,
            original.clone(),
            native_session.clone(),
            meerkat_core::ControllerModelClient::new(
                controller_facts().selection().clone(),
                Arc::new(UnusedController),
            ),
        );
        assert_eq!(prepared.session_id(), &native_session);
        assert!(prepared.original() == &original);
        let b_native = b_host.local_operator_setup().unwrap();
        let b_grants = state_directory(&b_selection, &b_native.native, NativeOwnerStartup::Restore)
            .unwrap()
            .join("grants.jsonl");
        let before = std::fs::read(&b_grants).unwrap();
        drop(b_native);
        assert!(matches!(
            b_host.local_operator_setup_for(&prepared),
            Err(HostLifetimeError::WrongOwner)
        ));
        assert_eq!(std::fs::read(&b_grants).unwrap(), before);
        let correct = a_host.local_operator_setup_for(&prepared).unwrap();
        drop(correct);
        a_closed.send(true).unwrap();
        assert!(matches!(
            a_host.local_operator_setup_for(&prepared),
            Err(HostLifetimeError::Denied)
        ));
        drop(a_host);
        drop(a_source);
        assert!(matches!(
            b_host.local_operator_setup_for(&prepared),
            Err(HostLifetimeError::Unavailable)
        ));
    }

    #[test]
    fn first_use_rejects_correct_hash_invalid_artifacts_before_creating_native_pair() {
        for case in super::super::tests::INVALID_EXECUTABLE_CASES {
            let (_directory, mut selection) = super::super::tests::fixture();
            let mut policy: meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy =
                serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
            policy.native_host.as_mut().unwrap().remote_operator =
                Some(super::super::tests::principal("selected-operator"));
            super::super::tests::replace_local(&mut selection, &policy);
            super::super::tests::invalid_executable_artifact(&mut selection, case);
            // The source selection/hash envelope itself is valid.
            drop(NativeHostLifetime::bootstrap(selection.clone()).unwrap());
            assert!(
                matches!(
                    OperatorSetup::open_for_application_operator(selection.clone(), startup()),
                    Err(HostLifetimeError::InvalidSelection)
                ),
                "{case}"
            );
            assert!(
                !selection
                    .deployment_root
                    .join("shared/data/toolkit-native")
                    .exists(),
                "{case}"
            );
        }
    }

    fn application_candidate() -> (tempfile::TempDir, HostBootstrapSelection) {
        let (directory, mut selection) = super::super::tests::fixture();
        let mut policy: meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        policy.native_host.as_mut().unwrap().remote_operator =
            Some(super::super::tests::principal("selected-operator"));
        super::super::tests::replace_local(&mut selection, &policy);
        (directory, selection)
    }

    #[tokio::test]
    async fn candidate_initialization_does_not_publish_or_authorize_candidate_runtime() {
        let (_directory, selection) = application_candidate();
        let current = selection.deployment_root.join("current");
        let previous = selection
            .deployment_root
            .join("shared/activations/previous.json");
        fs::write(&previous, b"{\"previous\":true}").unwrap();
        fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(&previous, &current).unwrap();
        let previous_link = fs::read_link(&current).unwrap();
        validate_application_candidate(&selection).unwrap();
        assert!(matches!(
            NativeHostLifetime::bootstrap(selection.clone()),
            Err(HostLifetimeError::Denied)
        ));
        OperatorSetup::provision_application_candidate(&selection, "candidate-owner", 1).unwrap();
        assert_eq!(fs::read_link(&current).unwrap(), previous_link);
        assert_eq!(fs::read(&previous).unwrap(), b"{\"previous\":true}");
        assert!(matches!(
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore).await,
            Err(HostLifetimeError::Denied)
        ));
        let (policy, _) =
            meerkat_toolkit::host_lifetime::policy::validate_candidate_selection(&selection)
                .unwrap();
        let native = policy.native_host.unwrap();
        let directory = state_directory(&selection, &native, NativeOwnerStartup::Restore).unwrap();
        let grants = fs::read(directory.join("grants.jsonl")).unwrap();
        let originals = fs::read(directory.join("originals.sqlite3")).unwrap();
        // An aborted activation retains this exact authority pair. No automatic
        // second Initialize, even with a new namespace/generation.
        assert!(
            OperatorSetup::provision_application_candidate(&selection, "replacement", 2).is_err()
        );
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), grants);
        assert_eq!(
            fs::read(directory.join("originals.sqlite3")).unwrap(),
            originals
        );
        // Only the application's actual stopped-host publication makes ordinary
        // Restore eligible. The released native lease permits this reopen.
        fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(&selection.activation.path, &current).unwrap();
        let restored =
            OperatorSetup::open_for_application_operator(selection, OperatorOwnerStartup::Restore)
                .unwrap();
        assert_eq!(restored.operator, native.remote_operator.unwrap());
        restored.finish().await.unwrap();
    }

    #[test]
    fn candidate_provisioning_refuses_occupied_entry_fence_and_partial_pairs() {
        let (_directory, selection) = application_candidate();
        // Use the same actual OS fence as ordinary host entry.
        drop(NativeHostLifetime::bootstrap(selection.clone()).unwrap());
        let fence =
            fs::File::open(selection.deployment_root.join("shared/control/deny.lock")).unwrap();
        fence.try_lock_shared().unwrap();
        assert!(
            OperatorSetup::provision_application_candidate(&selection, "candidate-owner", 1)
                .is_err()
        );
        assert!(
            !selection
                .deployment_root
                .join("shared/data/toolkit-native")
                .exists()
        );
        drop(fence);
        for present in ["grants.jsonl", "originals.sqlite3"] {
            let (_directory, selection) = application_candidate();
            let (policy, _) =
                meerkat_toolkit::host_lifetime::policy::validate_candidate_selection(&selection)
                    .unwrap();
            let directory = state_directory(
                &selection,
                policy.native_host.as_ref().unwrap(),
                NativeOwnerStartup::Initialize,
            )
            .unwrap();
            fs::write(directory.join(present), b"existing-unknown-owner-data").unwrap();
            assert!(
                OperatorSetup::provision_application_candidate(&selection, "candidate-owner", 1)
                    .is_err()
            );
            assert_eq!(
                fs::read(directory.join(present)).unwrap(),
                b"existing-unknown-owner-data"
            );
            assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        }
    }

    #[test]
    fn candidate_validation_preserves_app_metadata_and_refuses_bad_native_artifacts() {
        let (_directory, mut selection) = application_candidate();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection.policy_manifest.path).unwrap()).unwrap();
        manifest["seq"] = serde_json::json!(42);
        manifest["created"] = serde_json::json!("owned-by-application");
        manifest["inputs"] = serde_json::json!({"source":"application"});
        manifest["gating"] = serde_json::json!({"read_only":true});
        manifest["surfaces"] = serde_json::json!(["application-owned-surface"]);
        let bytes = canonical_json(&manifest).unwrap();
        fs::write(&selection.policy_manifest.path, &bytes).unwrap();
        selection.policy_manifest.sha256 = digest(&bytes);
        let mut activation: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection.activation.path).unwrap()).unwrap();
        activation["policy_bundle"]["policy_digest"] = serde_json::json!(format!(
            "hc-policy-v1:sha256:{}",
            selection.policy_manifest.sha256
        ));
        activation["application_metadata"] = serde_json::json!({"preserve":true});
        let activation_bytes = canonical_json(&activation).unwrap();
        fs::write(&selection.activation.path, &activation_bytes).unwrap();
        selection.activation.sha256 = digest(&activation_bytes);
        fs::remove_file(selection.deployment_root.join("current")).unwrap();
        validate_application_candidate(&selection).unwrap();
        assert_eq!(fs::read(&selection.policy_manifest.path).unwrap(), bytes);
        assert_eq!(
            fs::read(&selection.activation.path).unwrap(),
            activation_bytes
        );
        assert!(
            !selection
                .deployment_root
                .join("shared/data/toolkit-native")
                .exists()
        );
        assert!(
            !selection
                .deployment_root
                .join("shared/control/deny.lock")
                .exists()
        );
        for case in super::super::tests::INVALID_EXECUTABLE_CASES {
            let (_directory, mut selection) = application_candidate();
            super::super::tests::invalid_executable_artifact(&mut selection, case);
            fs::remove_file(selection.deployment_root.join("current")).unwrap();
            assert!(
                matches!(
                    validate_application_candidate(&selection),
                    Err(HostLifetimeError::InvalidSelection)
                ),
                "{case}"
            );
            assert!(
                matches!(
                    OperatorSetup::provision_application_candidate(
                        &selection,
                        "candidate-owner",
                        1
                    ),
                    Err(HostLifetimeError::InvalidSelection)
                ),
                "{case}"
            );
            assert!(
                !selection
                    .deployment_root
                    .join("shared/data/toolkit-native")
                    .exists(),
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn application_root_initialization_uses_same_pair_and_native_caller_equality() {
        let (_directory, mut selection) = super::super::tests::fixture();
        assert!(
            OperatorSetup::open_for_application_operator(selection.clone(), startup()).is_err()
        );
        let mut policy: meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        let selected = meerkat_core::PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::Human,
            "parent-1",
            meerkat_core::TrustDomainId::new("factory-test").unwrap(),
        )
        .unwrap();
        policy.native_host.as_mut().unwrap().remote_operator = Some(selected.clone());
        super::super::tests::replace_local(&mut selection, &policy);
        let setup =
            OperatorSetup::open_for_application_operator(selection.clone(), startup()).unwrap();
        assert_eq!(setup.operator, selected);
        assert!(
            OperatorSetup::open_for_application_operator(selection.clone(), startup()).is_err()
        );
        let view = OperatorSetup::from_application_gateway(
            setup.lifetime.clone(),
            setup.originals.clone(),
            setup.source.clone(),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&view.source, &setup.source));
        assert!(Arc::ptr_eq(&view.originals, &setup.originals));
        assert!(Arc::ptr_eq(&view.lifetime, &setup.lifetime));
        let proposal = view
            .prepare_controller_parent_review(
                Id::new("console").unwrap(),
                &controller_facts(),
                request(),
            )
            .unwrap();
        let approval = reviewed(&proposal, ApprovalDecision::Approve);
        assert!(matches!(
            view.apply_parent_decision(proposal, approval)
                .await
                .unwrap(),
            OperatorParentOutcome::Selected {
                purpose: OperatorParentPurpose::Controller,
                ..
            }
        ));
        drop(view);
        setup.finish().await.unwrap();
        // A changed bootstrap caller does not replace the restored root.
        policy.native_host.as_mut().unwrap().remote_operator =
            Some(super::super::tests::principal("different-operator"));
        super::super::tests::replace_local(&mut selection, &policy);
        let restored =
            OperatorSetup::open_for_application_operator(selection, OperatorOwnerStartup::Restore)
                .unwrap();
        let proposal = restored
            .prepare_controller_parent_review(
                Id::new("console").unwrap(),
                &controller_facts(),
                request(),
            )
            .unwrap();
        let approval = reviewed(&proposal, ApprovalDecision::Approve);
        assert!(matches!(
            restored
                .apply_parent_decision(proposal, approval)
                .await
                .unwrap(),
            OperatorParentOutcome::Unavailable
        ));
        restored.finish().await.unwrap();
    }

    #[tokio::test]
    async fn local_view_shares_owner_and_requires_published_controller_selection() {
        let (_directory, mut selection) = super::super::tests::fixture();
        let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
        let view = OperatorSetup::from_local_gateway(
            setup.lifetime.clone(),
            setup.originals.clone(),
            setup.source.clone(),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&setup.lifetime, &view.lifetime));
        assert!(Arc::ptr_eq(&setup.originals, &view.originals));
        assert!(Arc::ptr_eq(&setup.source, &view.source));
        let route = Id::new("console").unwrap();
        let facts = controller_facts();
        assert!(
            view.selected_controller_parent(&route, &facts)
                .unwrap()
                .is_none()
        );
        let proposal = view
            .prepare_controller_parent_review(route.clone(), &facts, request())
            .unwrap();
        assert_eq!(
            proposal.request().proposed_action.summary,
            "Allow this application to use the selected model"
        );
        let body = proposal.request().proposed_action.body.as_ref().unwrap();
        assert_eq!(body["controller"]["endpoint"], facts.endpoint());
        assert_eq!(body["controller"]["wire_model"], facts.wire_model());
        assert_eq!(body["controller"]["route_key"], facts.route_key().as_str());
        let approval = reviewed(&proposal, ApprovalDecision::Approve);
        let OperatorParentOutcome::Selected {
            lineage, purpose, ..
        } = view
            .apply_parent_decision(proposal, approval)
            .await
            .unwrap()
        else {
            panic!("expected exact controller parent")
        };
        assert_eq!(purpose, OperatorParentPurpose::Controller);
        // Issuance alone does not rewrite the selected policy or pick the
        // latest native grant. The old host is still missing the selection.
        assert!(
            view.selected_controller_parent(&route, &facts)
                .unwrap()
                .is_none()
        );
        let mut policy = view.lifetime.selected_policy().clone();
        policy.native_host.as_mut().unwrap().routes[0].controller_parent = Some(lineage.clone());
        super::super::tests::replace_local(&mut selection, &policy);
        assert!(view.selected_controller_parent(&route, &facts).is_err());
        drop(view);
        drop(setup);
        let restored =
            OperatorSetup::open(selection.clone(), OperatorOwnerStartup::Restore).unwrap();
        assert!(
            matches!(restored.selected_controller_parent(&route, &facts).unwrap(),
            Some(OperatorParentOutcome::Selected { lineage: ref selected, purpose: OperatorParentPurpose::Controller, .. }) if selected == &lineage)
        );
        let changed = meerkat_core::ControllerModelFacts::new(
            facts.selection().clone(),
            "https://changed.invalid".into(),
            facts.wire_model().into(),
        );
        assert!(
            restored
                .selected_controller_parent(&route, &changed)
                .unwrap()
                .is_none()
        );
        let view = OperatorSetup::from_local_gateway(
            restored.lifetime.clone(),
            restored.originals.clone(),
            restored.source.clone(),
        )
        .unwrap();
        restored
            .lifetime
            .begin_close_activation(HostCloseReason::Shutdown)
            .unwrap();
        assert!(view.selected_controller_parent(&route, &facts).is_err());
        assert!(
            view.prepare_controller_parent_review(route, &facts, request())
                .is_err()
        );
    }

    #[tokio::test]
    async fn operator_pair_is_exclusive_and_exact_decision_replays_after_restore() {
        let (_directory, selection) = super::super::tests::fixture();
        assert!(OperatorSetup::open(selection.clone(), OperatorOwnerStartup::Restore).is_err());
        let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
        let directory =
            state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
        assert!(directory.join("grants.jsonl").is_file());
        assert!(directory.join("originals.sqlite3").is_file());
        assert!(OperatorSetup::open(selection.clone(), OperatorOwnerStartup::Restore).is_err());
        assert!(OperatorSetup::open(selection.clone(), startup()).is_err());
        let proposal = setup
            .prepare_parent_review(Id::new("console").unwrap(), bounds(), request())
            .unwrap();
        let approval = reviewed(&proposal, ApprovalDecision::Approve);
        let first = setup
            .apply_parent_decision(proposal.clone(), approval.clone())
            .await
            .unwrap();
        let OperatorParentOutcome::Selected {
            route,
            purpose,
            lineage,
        } = first
        else {
            panic!("expected native parent")
        };
        assert_eq!(route, Id::new("console").unwrap());
        assert_eq!(purpose, OperatorParentPurpose::Operations);
        let resolved = setup
            .source
            .grants()
            .resolve_lineage(&lineage, &setup.native.grant_administrator, None)
            .unwrap();
        assert_eq!(resolved.restrictions(), &bounds());
        assert!(resolved.check_current().is_ok());
        drop(resolved);
        let bytes = fs::read(directory.join("grants.jsonl")).unwrap();
        let mut repurposed = proposal.clone();
        repurposed.purpose = OperatorParentPurpose::Controller;
        assert!(matches!(
            setup
                .apply_parent_decision(repurposed, approval.clone())
                .await,
            Err(HostLifetimeError::Denied)
        ));
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);

        // A predecessor decision lacks the explicit reviewed purpose. Even a
        // valid content digest cannot upgrade its record version into selection.
        let mut saved = setup
            .originals
            .list_records(
                &StoreScope {
                    realm: setup.native.realm.to_string(),
                    owner: setup.lifetime.selected_policy().deployment.to_string(),
                },
                None,
                2,
            )
            .await
            .unwrap();
        assert_eq!(saved.len(), 1);
        let mut previous = saved.pop().unwrap();
        assert_eq!(previous.data.version, RECORD_VERSION);
        previous.data.version = 1;
        previous
            .data
            .body
            .as_object_mut()
            .unwrap()
            .remove("purpose");
        previous.content_digest = previous.data.digest().unwrap();
        assert!(matches!(
            decode(&previous, &previous.key),
            Err(HostLifetimeError::InvalidSelection)
        ));
        drop(setup);
        let setup = OperatorSetup::open(selection.clone(), OperatorOwnerStartup::Restore).unwrap();
        assert!(setup.apply_parent_decision(proposal.clone(), approval.clone()).await.is_err());
        // The same durable request/answer is recoverable. Only the process-local
        // proposal custody is refreshed through this restored owner's constructor.
        let proposal = setup.prepare_parent_review_for(
            proposal.route, proposal.restrictions, proposal.request, proposal.purpose,
        ).unwrap();
        let replay = setup
            .apply_parent_decision(proposal, approval)
            .await
            .unwrap();
        assert!(
            matches!(replay, OperatorParentOutcome::Selected { lineage: ref saved, purpose: OperatorParentPurpose::Operations, .. } if saved == &lineage)
        );
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);
        drop(setup);
        // Initialization is fail-if-present after complete shutdown too.
        assert!(OperatorSetup::open(selection, startup()).is_err());
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);
    }

    #[tokio::test]
    async fn operator_denial_close_and_replaced_selection_never_issue() {
        let (_directory, selection) = super::super::tests::fixture();
        let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
        let directory =
            state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
        let before = fs::read(directory.join("grants.jsonl")).unwrap();
        let proposal = setup
            .prepare_parent_review(Id::new("console").unwrap(), bounds(), request())
            .unwrap();
        let denial = reviewed(&proposal, ApprovalDecision::Deny);
        assert!(matches!(
            setup.apply_parent_decision(proposal, denial).await.unwrap(),
            OperatorParentOutcome::Denied
        ));
        let proposal = setup
            .prepare_parent_review(Id::new("console").unwrap(), bounds(), request())
            .unwrap();
        let approval = reviewed(&proposal, ApprovalDecision::Approve);
        setup
            .lifetime
            .begin_close_activation(HostCloseReason::Shutdown)
            .unwrap();
        assert!(
            setup
                .apply_parent_decision(proposal, approval)
                .await
                .is_err()
        );
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), before);
        drop(setup);

        let setup = OperatorSetup::open(selection.clone(), OperatorOwnerStartup::Restore).unwrap();
        let proposal = setup
            .prepare_parent_review(Id::new("console").unwrap(), bounds(), request())
            .unwrap();
        let approval = reviewed(&proposal, ApprovalDecision::Approve);
        fs::write(&selection.local_policy.path, b"replaced").unwrap();
        assert!(
            setup
                .apply_parent_decision(proposal, approval)
                .await
                .is_err()
        );
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), before);
    }

    #[tokio::test]
    async fn saved_success_cannot_reselect_after_close_or_policy_replacement() {
        for replace in [false, true] {
            let (_directory, selection) = super::super::tests::fixture();
            let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
            let directory =
                state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
            let proposal = setup
                .prepare_parent_review(Id::new("console").unwrap(), bounds(), request())
                .unwrap();
            let approval = reviewed(&proposal, ApprovalDecision::Approve);
            assert!(matches!(
                setup
                    .apply_parent_decision(proposal.clone(), approval.clone())
                    .await
                    .unwrap(),
                OperatorParentOutcome::Selected { .. }
            ));
            let before = fs::read(directory.join("grants.jsonl")).unwrap();
            if replace {
                fs::write(&selection.local_policy.path, b"replaced selected policy").unwrap();
            } else {
                setup
                    .lifetime
                    .begin_close_activation(
                        HostCloseReason::Shutdown,
                    )
                    .unwrap();
            }
            // Same exact approved decision and native grant, not a new review.
            assert!(
                setup
                    .apply_parent_decision(proposal, approval)
                    .await
                    .is_err()
            );
            assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), before);
        }
    }

    #[test]
    fn partial_pair_is_not_repaired_or_reinitialized() {
        for missing in ["grants.jsonl", "originals.sqlite3"] {
            let (_directory, selection) = super::super::tests::fixture();
            let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
            let directory =
                state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
            drop(setup);
            fs::remove_file(directory.join(missing)).unwrap();
            let remaining = if missing == "grants.jsonl" {
                "originals.sqlite3"
            } else {
                "grants.jsonl"
            };
            let before = fs::read(directory.join(remaining)).unwrap();
            assert!(OperatorSetup::open(selection.clone(), OperatorOwnerStartup::Restore).is_err());
            assert!(OperatorSetup::open(selection, startup()).is_err());
            assert!(!directory.join(missing).exists());
            assert_eq!(fs::read(directory.join(remaining)).unwrap(), before);
        }
    }

    pub(super) fn service_proposal(setup: &OperatorSetup) -> (
        crate::private_originals::PrivateServiceMcpAccessDecision,
        crate::private_originals::ServiceMcpSelection,
        ApprovalRequest,
    ) {
        use crate::private_originals::{PrivateAccessChoice, PrivateServiceMcpAccessDecision,
            PrivateServiceMcpPermission, PrivateSourceEvidence, ServiceMcpSelection};
        use meerkat_toolkit::authority::{LifetimeScope, RoleLifetime};
        let route = setup.service_mcp_route(&Id::new("console").unwrap()).unwrap();
        let mut config = meerkat_core::mcp_config::McpServerConfig::stdio(
            "mail", "not-executed", vec![], Default::default());
        let operation = meerkat_core::mcp_config::McpConnectorOperation {
            required_scopes: ["mail.read".into()].into(),
            destination_origin: "https://mail.invalid".into(), read_only: true,
        };
        config.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
            realm_id: setup.native.realm.to_string(), slot_id: "slot".into(),
            issuer: "https://issuer.invalid".into(), client: "client".into(),
            resource: "https://mail.invalid".into(), strategy_id: "oauth".into(), account: "account-one".into(),
            operations: [("read".into(), operation.clone()), ("list".into(), operation)].into(),
        });
        let selection = ServiceMcpSelection::connector(
            &meerkat::ConnectorMcpInstallation::from_config(&config).unwrap().unwrap());
        let decision = PrivateServiceMcpAccessDecision {
            catalog: None,
            live_attachment: None,
            route: route.id, decision: Id::new("service-b").unwrap(),
            evidence: PrivateSourceEvidence { resource_id: EvidenceId::new("private-review").unwrap(),
                revision: EvidenceId::new("1").unwrap(), sha256: "a".repeat(64) },
            choice: PrivateAccessChoice::Approve,
            permission: PrivateServiceMcpPermission { target: route.grant_target,
                connection: Id::new("mail").unwrap(), operations: [Id::new("read").unwrap(), Id::new("list").unwrap()].into(),
                resource: Id::new("mail").unwrap(), lifetime: RoleLifetime { scope: LifetimeScope::Task { task: Id::new("fresh-b").unwrap() }, expires_at_ms: None } },
            connection_revision: toolkit_package::Revision::new(digest(&canonical_json(&config).unwrap())).unwrap(),
        };
        let mut request = request();
        request.proposed_action.body = Some(serde_json::json!({
            "instruction": "Read the reviewed inbox", "task": "fresh-b", "correlation": "old-a"
        }));
        (decision, selection, request)
    }
    fn service_approval(review: &ServiceMcpParentReview, choice: ApprovalDecision) -> ApprovalRecord {
        let approvals = ApprovalService::new();
        let pending = approvals.request(review.request().clone()).unwrap();
        approvals.decide(&pending.approval_id, choice,
            ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(), None, None).unwrap()
    }
    async fn service_adapter(setup: &OperatorSetup) -> crate::private_originals::PrivateOriginals {
        crate::private_originals::PrivateOriginals::new(StoreScope {
            realm: setup.native.realm.to_string(), owner: setup.lifetime.selected_policy().deployment.to_string(),
        }, setup.originals.clone(), setup.source.clone(),
            vec![setup.service_mcp_route(&Id::new("console").unwrap()).unwrap()]).await.unwrap()
    }

    #[tokio::test]
    async fn protected_service_parent_covers_whole_set_without_reselecting_route() {
        use crate::private_originals::{PrivateOriginalError, PrivateServiceGrantState};
        let (_directory, selection) = super::super::tests::fixture();
        let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
        let directory = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
        let (decision, mcp, request) = service_proposal(&setup);
        let review = setup.prepare_service_mcp_parent_review(decision.clone(), mcp.clone(), request).unwrap();
        let body = review.request().proposed_action.body.as_ref().unwrap();
        assert_eq!(body["access_request"]["instruction"], "Read the reviewed inbox");
        assert_eq!(body["service_access"]["permission"]["operations"], serde_json::json!(["list", "read"]));
        assert_eq!(body["connector_selection"]["connector"]["account"], "account-one");
        assert_eq!(body["purpose"], "operations");
        let approval = service_approval(&review, ApprovalDecision::Approve);
        let ServiceMcpParentOutcome::Approved(approved) = setup.apply_service_mcp_parent_decision(review, approval).await.unwrap()
            else { panic!("expected actual reviewed native parent") };
        assert!(approved.decision() == &decision);
        assert!(approved.selection() == &mcp);
        assert!(setup.route(&decision.route).unwrap().application_parent.is_none());
        let adapter = service_adapter(&setup).await;
        let observed = adapter.apply_mcp_service_access_decision(approved.clone()).await.unwrap();
        assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: setup.route(&decision.route).unwrap().source_authority.clone(), work: EvidenceId::new("fresh-b").unwrap(),
        };
        let lineage = adapter.service_grant_lineage(&observed.grant, &original).await.unwrap();
        assert_eq!(lineage.len(), 2);
        let route = setup.service_mcp_route(&decision.route).unwrap();
        let resolved = setup.source.grants().resolve_lineage(&lineage, &route.logical_executor, None).unwrap();
        let expected = mcp.restrictions(&route, &decision, setup.source.clock().now().unwrap().unix_ms).unwrap();
        assert_eq!(resolved.restrictions(), &expected);
        drop(resolved);
        let bytes = fs::read(directory.join("grants.jsonl")).unwrap();
        assert_eq!(adapter.apply_mcp_service_access_decision(approved.clone()).await.unwrap().state, PrivateServiceGrantState::GrantIssued);
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);
        let (_other_directory, other_selection) = super::super::tests::fixture();
        let other = OperatorSetup::open(other_selection, startup()).unwrap();
        assert!(matches!(approved.clone().into_parts(&other.source), Err(PrivateOriginalError::Denied)));
        other.finish().await.unwrap();
        setup.finish().await.unwrap();
        assert!(matches!(adapter.apply_mcp_service_access_decision(approved).await,
            Err(PrivateOriginalError::Denied | PrivateOriginalError::Unavailable)));
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);
    }

    #[tokio::test]
    async fn service_review_rejects_changed_set_context_and_denial_before_native_issue() {
        let (_directory, selection) = super::super::tests::fixture();
        let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
        let directory = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
        let (decision, mcp, request) = service_proposal(&setup);
        let original = setup.prepare_service_mcp_parent_review(decision.clone(), mcp.clone(), request.clone()).unwrap();
        let approval = service_approval(&original, ApprovalDecision::Approve);
        let bytes = fs::read(directory.join("grants.jsonl")).unwrap();
        let mut subset = decision.clone(); subset.permission.operations.remove(&Id::new("list").unwrap());
        let changed = setup.prepare_service_mcp_parent_review(subset, mcp.clone(), request.clone()).unwrap();
        assert!(matches!(setup.apply_service_mcp_parent_decision(changed, approval.clone()).await, Err(HostLifetimeError::Denied)));
        let mut changed_request = request.clone();
        changed_request.proposed_action.body.as_mut().unwrap()["instruction"] = "Send a message instead".into();
        let changed = setup.prepare_service_mcp_parent_review(decision.clone(), mcp.clone(), changed_request).unwrap();
        assert!(matches!(setup.apply_service_mcp_parent_decision(changed, approval).await, Err(HostLifetimeError::Denied)));
        let review = setup.prepare_service_mcp_parent_review(decision, mcp, request).unwrap();
        let denial = service_approval(&review, ApprovalDecision::Deny);
        assert!(matches!(setup.apply_service_mcp_parent_decision(review, denial).await.unwrap(), ServiceMcpParentOutcome::Denied));
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);
        setup.finish().await.unwrap();
    }

    #[tokio::test]
    async fn protected_live_effect_is_explicit_and_cannot_reuse_a_tool_only_answer() {
        use crate::private_originals::{PrivateServiceGrantState, PrivateServiceMcpLiveAttachment};
        use meerkat_authorization_contracts::work_association::NativeWorkTarget;
        use meerkat_authorization_contracts::constraints::OperationRestrictionValues;
        let (_directory, selection) = super::super::tests::fixture();
        let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
        let directory = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
        let (mut decision, mcp, request) = service_proposal(&setup);
        let tool_only = setup.prepare_service_mcp_parent_review(decision.clone(), mcp.clone(), request.clone()).unwrap();
        assert!(tool_only.request().proposed_action.body.as_ref().unwrap()["service_access"].get("live_attachment").is_none());
        let tool_only_answer = service_approval(&tool_only, ApprovalDecision::Approve);
        let member = &setup.native.members[0];
        let session = meerkat_core::SessionId::new();
        let destination = NativeWorkTarget {
            logical_owner: member.logical_owner.clone(),
            logical_runtime: EvidenceId::new(meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()).unwrap(),
            context: EvidenceId::new(session.to_string()).unwrap(),
            context_generation: member.context_generation, audience: member.audience.clone(),
        };
        decision.live_attachment = Some(PrivateServiceMcpLiveAttachment { destination: destination.clone() });
        decision.permission.lifetime.expires_at_ms = Some(u64::MAX);
        let live = setup.prepare_service_mcp_parent_review(decision.clone(), mcp.clone(), request.clone()).unwrap();
        assert_eq!(live.request().proposed_action.body.as_ref().unwrap()["service_access"]["live_attachment"]["destination"],
            serde_json::to_value(&destination).unwrap());
        let bytes = fs::read(directory.join("grants.jsonl")).unwrap();
        assert!(matches!(setup.apply_service_mcp_parent_decision(live, tool_only_answer).await,
            Err(HostLifetimeError::Denied)));
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);
        let mut wrong = decision.clone();
        wrong.live_attachment.as_mut().unwrap().destination.context_generation += 1;
        assert!(matches!(setup.prepare_service_mcp_parent_review(wrong, mcp.clone(), request.clone()), Err(HostLifetimeError::Denied)));
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), bytes);
        let live = setup.prepare_service_mcp_parent_review(decision.clone(), mcp.clone(), request).unwrap();
        let answer = service_approval(&live, ApprovalDecision::Approve);
        let ServiceMcpParentOutcome::Approved(approved) = setup.apply_service_mcp_parent_decision(live, answer).await.unwrap()
            else { panic!("expected real protected parent issuance") };
        let adapter = service_adapter(&setup).await;
        let observed = adapter.apply_mcp_service_access_decision(approved).await.unwrap();
        assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
        let route = setup.service_mcp_route(&decision.route).unwrap();
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(), work: EvidenceId::new("fresh-b").unwrap(),
        };
        let lineage = adapter.service_grant_lineage(&observed.grant, &original).await.unwrap();
        let resolved = setup.source.grants().resolve_lineage(&lineage, &route.logical_executor, None).unwrap();
        let exact = digest(&canonical_json(&("mail", &decision.connection_revision, &destination)).unwrap());
        let values = super::super::application_operations::mcp_live_attachment(
            &route.grant_caller, &route.logical_executor, &route.grant_audience, &exact);
        resolved.restrictions().check_bounds(OperationRestrictionValues {
            action: &values.action, resource_domain: &values.resource_domain,
            processor: &values.processor, audience: &values.audience,
            now_ms: setup.source.clock().now().unwrap().unix_ms,
        }).unwrap();
        drop(resolved);
        setup.finish().await.unwrap();
    }


    #[tokio::test]
    async fn retained_service_review_uses_exact_existing_parent_without_writes() {
        use crate::private_originals::PrivateSourceEvidence;
        let (_directory, selection) = super::super::tests::fixture();
        let setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
        let (decision, mcp, request) = service_proposal(&setup);
        let review = setup.prepare_service_mcp_parent_review(decision.clone(), mcp, request).unwrap();
        let approval = service_approval(&review, ApprovalDecision::Approve);
        let expected = approval.clone();
        let ServiceMcpParentOutcome::Approved(approved) = setup.apply_service_mcp_parent_decision(review, approval).await.unwrap()
            else { panic!("real reviewed parent required") };
        let (_, _, parent, _) = approved.into_parts(&setup.source).unwrap();
        let evidence = PrivateSourceEvidence {
            resource_id: EvidenceId::new(expected.approval_id.to_string()).unwrap(),
            revision: EvidenceId::new("settled-review-v1").unwrap(),
            sha256: digest(&canonical_json(&expected).unwrap()),
        };
        let scope = toolkit_store::StoreScope { realm: setup.native.realm.to_string(),
            owner: setup.lifetime.selected_policy().deployment.to_string() };
        let before = setup.originals.list_records(&scope, None, 256).await.unwrap();
        let directory = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
        let grants = std::fs::read(directory.join("grants.jsonl")).unwrap();
        assert_eq!(setup.retained_service_review(&decision.route, &evidence, &parent).await.unwrap(), expected);
        for change in ["digest", "evidence", "legacy", "parent", "missing"] {
            let mut evidence = evidence.clone();
            let mut parent = parent.clone();
            match change {
                "digest" => evidence.sha256 = "0".repeat(64),
                "evidence" => evidence.resource_id = EvidenceId::new("other-review").unwrap(),
                "legacy" => evidence.revision = EvidenceId::new("proposal-v1").unwrap(),
                "parent" => parent[0].grant_id = EvidenceId::new("other-parent").unwrap(),
                "missing" => parent.clear(),
                _ => unreachable!(),
            }
            assert!(setup.retained_service_review(&decision.route, &evidence, &parent).await.is_err(), "{change}");
        }
        assert_eq!(setup.originals.list_records(&scope, None, 256).await.unwrap(), before);
        assert_eq!(std::fs::read(directory.join("grants.jsonl")).unwrap(), grants);
    }

}
